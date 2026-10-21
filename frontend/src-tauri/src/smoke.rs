//! Дымовой прогон упакованного приложения: `nivora-launcher --smoke-test`.
//!
//! Прогоняет операции через тот же мост `backend_call`, что использует UI:
//! нативные, воркерные и транзитные маршруты, параллельные вызовы,
//! операции из нового функционала. Отчёт — JSON в диагностический каталог
//! (`smoke-test.json`), код выхода: 0 — успех, 3 — шаг провален,
//! 4 — прогон не уложился в бюджет времени (watchdog).
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::{backend_call_sync, diagnostic_dir, startup_trace};

pub const ARGUMENT: &str = "--smoke-test";
const WATCHDOG_BUDGET: Duration = Duration::from_secs(240);
const PARALLEL_CALLS: usize = 20;
const SMOKE_INSTANCE: &str = "nivora-smoke-missing";

/// Признак провала именно моста: эти строки ищет фронтенд и поддержка.
fn bridge_failure(error: &str) -> bool {
    error.contains("Операция GUI не разрешена desktop bridge")
        || error.contains("GUI-запрос не содержит operation")
}

struct Outcome {
    ok: bool,
    ms: u128,
    error: Option<String>,
}

impl Outcome {
    fn ok(ms: u128) -> Self {
        Self { ok: true, ms, error: None }
    }
    fn fail(ms: u128, error: String) -> Self {
        Self { ok: false, ms, error: Some(error) }
    }
}

fn timed(app: &AppHandle, request: Value) -> Outcome {
    let started = Instant::now();
    let result = backend_call_sync(app.clone(), request);
    let ms = started.elapsed().as_millis();
    match result {
        Ok(_) => Outcome::ok(ms),
        Err(error) => Outcome::fail(ms, error),
    }
}

/// Шаг, где допустима (и обязательна) доменная ошибка — например, запрос
/// несуществующего экземпляра. Провелом считается только ошибка моста.
fn timed_expect_domain_error(app: &AppHandle, request: Value) -> Outcome {
    let started = Instant::now();
    let result = backend_call_sync(app.clone(), request);
    let ms = started.elapsed().as_millis();
    match result {
        Ok(value) => Outcome::fail(
            ms,
            format!("ожидалась предсказуемая ошибка, получен успех: {value}"),
        ),
        Err(error) if bridge_failure(&error) => Outcome::fail(ms, error),
        Err(error) => Outcome::ok(ms).with_error(error),
    }
}

impl Outcome {
    fn with_error(mut self, error: String) -> Self {
        self.error = Some(error);
        self
    }
}

/// N параллельных вызовов одного запроса: все должны завершиться (успехом
/// или ожидаемой доменной ошибкой) без провалов моста и паник.
fn parallel(app: &AppHandle, request: Value, expect_ok: bool) -> Outcome {
    let started = Instant::now();
    let results = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(PARALLEL_CALLS);
        for _ in 0..PARALLEL_CALLS {
            let handle = app.clone();
            let request = request.clone();
            handles.push(scope.spawn(move || backend_call_sync(handle, request)));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| Err("поток вызова упал".to_owned())))
            .collect::<Vec<_>>()
    });
    let ms = started.elapsed().as_millis();
    let failures: Vec<String> = results
        .into_iter()
        .filter_map(|result| match result {
            Ok(_) if expect_ok => None,
            Ok(_) => Some("ожидалась предсказуемая ошибка, получен успех".to_owned()),
            Err(error) if bridge_failure(&error) => Some(error),
            Err(_) => None,
        })
        .collect();
    if failures.is_empty() {
        Outcome::ok(ms)
    } else {
        Outcome::fail(ms, failures.join("; "))
    }
}

pub fn requested() -> bool {
    std::env::args().any(|argument| argument == ARGUMENT)
}

pub fn run(app: AppHandle) {
    let started = Instant::now();
    startup_trace("smoke: начало");
    // Страховка от зависшего шага (например, воркер не отвечает):
    // без этого CI ждал бы таймаута задачи целиком.
    let watchdog = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(WATCHDOG_BUDGET);
        startup_trace("smoke: превышен бюджет времени — принудительный выход");
        watchdog.exit(4);
    });

    let mut steps: Vec<(&'static str, Outcome)> = Vec::new();

    // Нативные операции.
    steps.push(("engine_info", timed(&app, json!({ "operation": "engine_info" }))));
    let bootstrap_started = Instant::now();
    let bootstrap_result = backend_call_sync(app.clone(), json!({ "operation": "bootstrap" }));
    let bootstrap_ms = bootstrap_started.elapsed().as_millis();
    // Настройки из того же ответа: save_settings ниже пишет их обратно
    // без изменений (roundtrip), пользовательские данные не затрагиваются.
    let bootstrap_settings = bootstrap_result
        .as_ref()
        .ok()
        .and_then(|value| value.get("settings").cloned())
        .unwrap_or_else(|| json!({}));
    steps.push((
        "bootstrap",
        match bootstrap_result {
            Ok(_) => Outcome::ok(bootstrap_ms),
            Err(error) => Outcome::fail(bootstrap_ms, error),
        },
    ));
    steps.push((
        "native_parallel_bootstrap",
        parallel(&app, json!({ "operation": "bootstrap" }), true),
    ));

    // Воркер: обычный маршрут (постоянный воркер) и транзитный (своя копия
    // на вызов) на заведомо несуществующем экземпляре — предсказуемая
    // доменная ошибка вместо провала моста.
    steps.push((
        "worker_parallel_instance_log",
        parallel(&app, json!({ "operation": "instance_log", "name": SMOKE_INSTANCE }), false),
    ));
    steps.push((
        "transient_worker_launch_missing",
        timed_expect_domain_error(&app, json!({ "operation": "launch_instance", "name": SMOKE_INSTANCE })),
    ));

    // Новый функционал: ярлыки и экспорт списка модов.
    steps.push((
        "shortcut_status_missing",
        timed_expect_domain_error(&app, json!({ "operation": "instance_shortcut_status", "name": SMOKE_INSTANCE })),
    ));
    steps.push((
        "mod_list_export_missing",
        timed_expect_domain_error(&app, json!({ "operation": "instance_mod_list_export", "name": SMOKE_INSTANCE })),
    ));
    steps.push((
        "shortcut_take_launch_request",
        timed(&app, json!({ "operation": "shortcut_take_launch_request" })),
    ));
    steps.push((
        "save_settings_roundtrip",
        timed(&app, json!({ "operation": "save_settings", "values": bootstrap_settings })),
    ));

    let ok = steps.iter().all(|(_, outcome)| outcome.ok);
    let report = json!({
        "ok": ok,
        "parallel_calls": PARALLEL_CALLS,
        "duration_ms": started.elapsed().as_millis(),
        "steps": steps
            .iter()
            .map(|(name, outcome)| json!({
                "name": name,
                "ok": outcome.ok,
                "ms": outcome.ms,
                "error": outcome.error,
            }))
            .collect::<Vec<_>>(),
    });
    let report_path = diagnostic_dir().join("smoke-test.json");
    if let Some(parent) = report_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let written = std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap_or_default());
    startup_trace(&format!(
        "smoke: {} за {} мс, отчёт: {}{}",
        if ok { "успех" } else { "провал" },
        started.elapsed().as_millis(),
        report_path.display(),
        match (&written, ok) {
            (Ok(_), _) => String::new(),
            (Err(error), _) => format!(" (не записан: {error})"),
        }
    ));
    let _ = written;
    app.exit(if ok { 0 } else { 3 });
}
