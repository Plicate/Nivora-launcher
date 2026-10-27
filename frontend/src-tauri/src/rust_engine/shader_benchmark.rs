// сравнивает нагрузку игры без шейдеров и с ними в главном меню
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use super::accounts;
use super::config::ConfigStore;
use super::events;
use super::instances::InstanceStore;
use super::installer::MinecraftInstaller;
use super::launcher::{self, GameLauncher};
use super::metrics;
use super::net::{progress_client_from_settings, HttpClient};
use super::platform_support;

pub(crate) const MENU_MARKER: &str = "Sound engine started";
pub(crate) const MENU_WAIT_SECONDS: u64 = 150;
pub(crate) const SAMPLE_SECONDS: u64 = 15;

fn active() -> &'static Mutex<HashMap<String, Map<String, Value>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, Map<String, Value>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn benchmark_state(instance_name: &str) -> Value {
    let guard = active().lock().unwrap_or_else(|poison| poison.into_inner());
    match guard.get(instance_name) {
        None => json!({ "phase": "idle", "available": true }),
        Some(state) => {
            let mut snapshot = state.clone();
            snapshot.insert("available".to_owned(), json!(false));
            Value::Object(snapshot)
        }
    }
}

fn update_state(instance_name: &str, values: &[(&str, Value)]) {
    let mut guard = active().lock().unwrap_or_else(|poison| poison.into_inner());
    let state = guard
        .entry(instance_name.to_owned())
        .or_default();
    for (key, value) in values {
        state.insert((*key).to_owned(), value.clone());
    }
}

fn finish_state(instance_name: &str, error: &str) {
    let mut values = vec![
        ("phase", json!(if error.is_empty() { "done" } else { "failed" })),
        ("progress", json!(100)),
    ];
    if !error.is_empty() {
        values.push(("error", json!(error)));
    }
    values.push(("ended_at", json!(epoch_seconds())));
    update_state(instance_name, &values);
}

fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn start_benchmark(
    instance_name: &str,
    instance_path: &Path,
    minecraft_version: &str,
    shader_file: &str,
) -> Result<(), String> {
    {
        let mut guard = active().lock().unwrap_or_else(|poison| poison.into_inner());
        if guard.contains_key(instance_name) {
            return Err("Бенчмарк шейдеров уже идёт для этого экземпляра".to_owned());
        }
        let mut state = Map::new();
        state.insert("phase".to_owned(), json!("prepare"));
        state.insert("progress".to_owned(), json!(0));
        state.insert("message".to_owned(), json!("Готовлю запуск…"));
        guard.insert(instance_name.to_owned(), state);
    }
    let instance_name = instance_name.to_owned();
    let instance_path = instance_path.to_path_buf();
    let minecraft_version = minecraft_version.to_owned();
    let shader_file = shader_file.to_owned();
    std::thread::Builder::new()
        .name(format!("shader-benchmark-{instance_name}"))
        .spawn(move || {
            let options_path = instance_path.join("options.txt");
            let original = read_options(&options_path);
            let result = run_benchmark(
                &instance_name,
                &instance_path,
                &minecraft_version,
                &shader_file,
                &options_path,
                &original,
            );
            match result {
                Ok(()) => {}
                Err(error) => finish_state(&instance_name, &error),
            }
            // всегда восстанавливаем исходный `options.txt`
            if let Err(error) = write_options(&options_path, &original) {
                update_state(&instance_name, &[("restore_error", json!(error))]);
            }
        })
        .map_err(|error| format!("Не удалось запустить поток бенчмарка: {error}"))?;
    Ok(())
}

fn run_benchmark(
    instance_name: &str,
    instance_path: &Path,
    minecraft_version: &str,
    shader_file: &str,
    options_path: &Path,
    original: &str,
) -> Result<(), String> {
    write_options(options_path, &apply_shader_selection(original, None, minecraft_version))?;
    update_state(
        instance_name,
        &[
            ("phase", json!("baseline")),
            ("progress", json!(10)),
            ("message", json!("Запускаю игру без шейдера…")),
        ],
    );
    let baseline = measure_phase(instance_name, instance_path, "baseline")?;
    write_options(
        options_path,
        &apply_shader_selection(original, Some(shader_file), minecraft_version),
    )?;
    update_state(
        instance_name,
        &[
            ("phase", json!("shader")),
            ("progress", json!(55)),
            ("message", json!("Запускаю игру с шейдером…")),
            ("baseline", baseline.clone()),
        ],
    );
    let with_shader = measure_phase(instance_name, instance_path, "shader")?;
    update_state(
        instance_name,
        &[
            ("phase", json!("done")),
            ("progress", json!(100)),
            ("baseline", baseline.clone()),
            ("shader", with_shader.clone()),
            ("verdict", compare(&baseline, &with_shader)),
            ("message", json!("Бенчмарк завершён")),
        ],
    );
    Ok(())
}

fn measure_phase(instance_name: &str, instance_path: &Path, phase_label: &str) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let http: Arc<HttpClient> = progress_client_from_settings(config.settings(), "")?;
    let mut installer = MinecraftInstaller::new(&config, http.clone())?;
    let install = installer.load_existing(instance_path, None)?;
    let account = benchmark_account()?;
    let pid_slot: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let session_name = instance_name.to_owned();
    let sink_session = session_name.clone();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let launch_id = format!("shader-benchmark-{}-{phase_label}-{stamp}", std::process::id());
    let log_id = format!("{launch_id}.log");
    let launch_id_for_logs = launch_id.clone();
    let log_id_for_logs = log_id.clone();
    let mut log_sequence = 0_u64;
    let sink = Arc::new(Mutex::new(move |line: &str| {
        launcher::session_append(&sink_session, line);
        log_sequence = log_sequence.saturating_add(1);
        events::emit_launch_log(line, &session_name, &launch_id_for_logs, &log_id_for_logs, log_sequence);
    }));
    let worker_pid = pid_slot.clone();
    let worker = {
        let launcher = GameLauncher::new(ConfigStore::load()?, http.clone(), None)?;
        std::thread::Builder::new()
            .name(format!("shader-benchmark-{instance_name}-{phase_label}"))
            .spawn(move || {
                // ошибка запуска отражается в статусе; ждём появления главного меню
                let _ = launcher.run(&install, &account, false, &launch_id, &log_id, sink, &json!({}), Some(&worker_pid));
            })
            .map_err(|error| format!("Не удалось запустить игровой поток: {error}"))?
    };
    let started = Instant::now();
    let mut menu_seen = false;
    while started.elapsed().as_secs() < MENU_WAIT_SECONDS {
        if worker.is_finished() {
            return Err(format!("Игра закрылась раньше главного меню ({phase_label})"));
        }
        let (lines, _) = launcher::session_snapshot(instance_name);
        let joined = lines.join("\n");
        if joined.contains(MENU_MARKER)
            || (joined.contains("Reloading ResourceManager") && joined.contains("Created: "))
        {
            menu_seen = true;
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if !menu_seen {
        stop_game(&pid_slot);
        let _ = worker.join();
        return Err(format!("Главное меню не появилось за {MENU_WAIT_SECONDS} с ({phase_label})"));
    }
    update_state(instance_name, &[("message", json!(format!("Замеряю нагрузку ({phase_label})…")))]);
    let mut samples = 0usize;
    let mut cpu_total = 0.0f64;
    let mut gpu_total = 0.0f64;
    let mut gpu_mem_peak = 0u64;
    let mut ram_peak = 0u64;
    let mut previous_cpu: Option<f64> = None;
    let mut previous_at: Option<Instant> = None;
    let measure_start = Instant::now();
    while measure_start.elapsed().as_secs() < SAMPLE_SECONDS {
        let pid = pid_slot.lock().map(|guard| guard.unwrap_or(0)).unwrap_or(0);
        let sample = if pid > 0 { metrics::process_sample(pid) } else { None };
        if let Some((cpu_seconds, ram_bytes)) = sample {
            let now = Instant::now();
            if let (Some(previous), Some(at)) = (previous_cpu, previous_at) {
                let elapsed = now.duration_since(at).as_secs_f64().max(0.05);
                cpu_total += ((cpu_seconds - previous) / elapsed * 100.0).max(0.0);
                samples += 1;
            }
            previous_cpu = Some(cpu_seconds);
            previous_at = Some(now);
            ram_peak = ram_peak.max(ram_bytes);
            let gpu = metrics::gpu_process_sample(pid);
            if let Some(utilization) = gpu.get("util_percent").and_then(Value::as_f64) {
                gpu_total += utilization;
            }
            if let Some(memory) = gpu.get("memory_bytes").and_then(Value::as_u64) {
                gpu_mem_peak = gpu_mem_peak.max(memory);
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    stop_game(&pid_slot);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if samples > 0 {
        return Ok(json!({
            "cpu_percent": (cpu_total / samples as f64 * 10.0).round() / 10.0,
            "ram_mb": (ram_peak as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0,
            "gpu_percent": if gpu_total > 0.0 { json!((gpu_total / SAMPLE_SECONDS as f64 * 10.0).round() / 10.0) } else { Value::Null },
            "gpu_memory_mb": if gpu_mem_peak > 0 { json!((gpu_mem_peak as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0) } else { Value::Null },
            "samples": samples,
        }));
    }
    Ok(json!({
        "cpu_percent": null,
        "ram_mb": 0.0,
        "gpu_percent": null,
        "gpu_memory_mb": null,
        "samples": 0,
    }))
}

fn benchmark_account() -> Result<Map<String, Value>, String> {
    let config = ConfigStore::load()?;
    let roots = super::state::launcher_roots();
    let records = accounts::load_accounts_with_tokens(&roots.config_home)?;
    if let Some(default) = records
        .iter()
        .find(|record| record.get("is_default").and_then(Value::as_bool).unwrap_or(false))
    {
        return Ok(default.clone());
    }
    let _ = config;
    accounts::offline_debug_account(&roots.config_home)
}

fn stop_game(pid_slot: &Arc<Mutex<Option<i32>>>) {
    let pid = pid_slot.lock().map(|guard| guard.unwrap_or(0)).unwrap_or(0);
    if pid <= 0 || !platform_support::process_exists(pid) {
        return;
    }
    platform_support::terminate_process_tree(pid, false);
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && platform_support::process_exists(pid) {
        std::thread::sleep(Duration::from_millis(100));
    }
    if platform_support::process_exists(pid) {
        platform_support::terminate_process_tree(pid, true);
    }
}

pub(crate) fn compare(baseline: &Value, with_shader: &Value) -> Value {
    let load = |sample: &Value| -> f64 {
        for key in ["gpu_percent", "cpu_percent"] {
            if let Some(value) = sample.get(key).and_then(Value::as_f64) {
                return value;
            }
        }
        0.0
    };
    let before = load(baseline);
    let after = load(with_shader);
    let delta = (after - before).max(0.0);
    let (label, tone) = if after >= 95.0 {
        (
            "Видеокарта упирается в потолок — шейдер слишком тяжёлый",
            "danger",
        )
    } else if delta >= 40.0 {
        ("Тяжёлый шейдер: нагрузка на карту выросла сильно", "warning")
    } else if delta >= 15.0 {
        ("Заметная нагрузка — играбельно на средних настройках", "warning")
    } else {
        ("Лёгкий для этой системы", "success")
    };
    json!({
        "label": label,
        "tone": tone,
        "gpu_delta_percent": (delta * 10.0).round() / 10.0,
        "metric": if baseline.get("gpu_percent").map(|value| !value.is_null()).unwrap_or(false) { "gpu_percent" } else { "cpu_percent" },
    })
}

fn read_options(path: &Path) -> String {
    super::options_file::read_raw(path)
}

fn write_options(path: &Path, content: &str) -> Result<(), String> {
    super::options_file::write_raw(path, content)
}

fn shader_options_line(minecraft_version: &str, shader_file: &str) -> String {
    // начиная с 1.21.4 выбор хранится JSON-строкой
    let mut modern = false;
    let numbers: Vec<u64> = minecraft_version
        .split('.')
        .map(|part| part.trim_start_matches(|character: char| !character.is_ascii_digit()))
        .filter_map(|part| part.parse::<u64>().ok())
        .collect();
    if numbers.len() >= 2 {
        let (major, minor) = (numbers[0], numbers[1]);
        modern = (major, minor) >= (1, 21)
            && (minor > 21
                || matches!(minecraft_version.trim(), "1.21.4" | "1.21.5" | "1.21.6" | "1.21.7" | "1.21.8" | "1.21.9" | "1.21.10" | "1.21.11"));
    }
    if modern {
        let payload = json!({ "id": "file", "file": shader_file });
        format!("shaderPack:{payload}")
    } else {
        format!("shaderPack:{shader_file}")
    }
}

pub(crate) fn apply_shader_selection(options_content: &str, shader_file: Option<&str>, minecraft_version: &str) -> String {
    let mut lines: Vec<String> = options_content
        .lines()
        .filter(|line| !line.starts_with("shaderPack:"))
        .map(str::to_owned)
        .collect();
    if let Some(shader_file) = shader_file {
        lines.push(shader_options_line(minecraft_version, shader_file));
    }
    if lines.is_empty() {
        String::new()
    } else {
        let mut joined = lines.join("\n");
        joined.push('\n');
        joined
    }
}


pub fn shader_benchmark_start(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let shader = request.get("shader").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    if name.is_empty() || shader.is_empty() {
        return Err("Укажите экземпляр и файл шейдерпака".to_owned());
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.running {
        return Err("Остановите игру перед бенчмарком шейдеров".to_owned());
    }
    let shader_path = info.path.join("shaderpacks").join(&shader);
    if shader_path.is_symlink() || !shader_path.exists() {
        return Err(format!("Шейдерпак не найден в экземпляре: {shader}"));
    }
    let mods_dir = info.path.join("mods");
    let has_iris = fs::read_dir(&mods_dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().to_lowercase().starts_with("iris"))
        })
        .unwrap_or(false);
    if !has_iris {
        return Err("Бенчмарку нужен Iris: установите Iris из каталога модов — шейдеры без него не работают".to_owned());
    }
    let minecraft_version = if info.minecraft_version.is_empty() {
        info.version_id.clone()
    } else {
        info.minecraft_version.clone()
    };
    start_benchmark(&name, &info.path, &minecraft_version, &shader)?;
    Ok(json!({ "started": true }))
}

pub fn shader_benchmark_status(request: &Value) -> Result<Value, String> {
    Ok(benchmark_state(request.get("name").and_then(Value::as_str).unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_state_is_available() {
        let state = benchmark_state("точно-не-существующий-экземпляр");
        assert_eq!(state["phase"], json!("idle"));
        assert_eq!(state["available"], json!(true));
    }

    #[test]
    fn started_benchmark_marks_instance_unavailable() {
        let name = format!("bench-state-{}", std::process::id());
        update_state(&name, &[("phase", json!("baseline"))]);
        let state = benchmark_state(&name);
        assert_eq!(state["phase"], json!("baseline"));
        assert_eq!(state["available"], json!(false));
        finish_state(&name, "");
        let done = benchmark_state(&name);
        assert_eq!(done["phase"], json!("done"));
        assert_eq!(done["progress"], json!(100));
        finish_state(&name, "ошибка");
        let failed = benchmark_state(&name);
        assert_eq!(failed["phase"], json!("failed"));
        assert_eq!(failed["error"], json!("ошибка"));
        active().lock().unwrap().remove(&name);
    }

    #[test]
    fn duplicate_benchmark_is_rejected() {
        let name = format!("bench-dup-{}", std::process::id());
        let guard = active();
        guard.lock().unwrap().insert(name.clone(), Map::new());
        let error = start_benchmark(&name, Path::new("/tmp"), "1.20.1", "shader.zip").unwrap_err();
        assert_eq!(error, "Бенчмарк шейдеров уже идёт для этого экземпляра");
        guard.lock().unwrap().remove(&name);
    }

    #[test]
    fn shader_selection_removes_and_sets_the_line() {
        let options = "version:1.20.1\nshaderPack:old.zip\nfov:70\n";
        let cleared = apply_shader_selection(options, None, "1.20.1");
        assert_eq!(cleared, "version:1.20.1\nfov:70\n");
        let set_legacy = apply_shader_selection(options, Some("new.zip"), "1.20.1");
        assert_eq!(set_legacy, "version:1.20.1\nfov:70\nshaderPack:new.zip\n");
        assert_eq!(apply_shader_selection("", None, "1.20.1"), "");
        let set_legacy_empty = apply_shader_selection("", Some("new.zip"), "1.20.1");
        assert_eq!(set_legacy_empty, "shaderPack:new.zip\n");
    }

    #[test]
    fn modern_versions_use_json_shader_payload() {
        assert_eq!(shader_options_line("1.20.1", "pack.zip"), "shaderPack:pack.zip");
        assert_eq!(shader_options_line("1.21.3", "pack.zip"), "shaderPack:pack.zip");
        assert_eq!(
            shader_options_line("1.21.4", "pack.zip"),
            "shaderPack:{\"id\":\"file\",\"file\":\"pack.zip\"}"
        );
        assert_eq!(
            shader_options_line("1.21.11", "pack.zip"),
            "shaderPack:{\"id\":\"file\",\"file\":\"pack.zip\"}"
        );
        assert_eq!(
            shader_options_line("1.22", "pack.zip"),
            "shaderPack:{\"id\":\"file\",\"file\":\"pack.zip\"}"
        );
    }

    #[test]
    fn compare_labels_follow_load_delta() {
        let heavy = json!({ "gpu_percent": 97.0 });
        let light = json!({ "gpu_percent": 10.0 });
        let cpu_only = json!({ "cpu_percent": 40.0 });
        let idle = json!({ "cpu_percent": 8.0 });

        let danger = compare(&light, &heavy);
        assert_eq!(danger["tone"], json!("danger"));
        assert_eq!(danger["metric"], json!("gpu_percent"));

        let warning = compare(&light, &json!({ "gpu_percent": 60.0 }));
        assert_eq!(warning["tone"], json!("warning"));
        assert_eq!(warning["gpu_delta_percent"], json!(50.0));

        let moderate = compare(&idle, &cpu_only);
        assert_eq!(moderate["tone"], json!("warning"));
        assert_eq!(moderate["gpu_delta_percent"], json!(32.0));
        assert_eq!(moderate["metric"], json!("cpu_percent"));

        let success = compare(&light, &json!({ "gpu_percent": 20.0 }));
        assert_eq!(success["tone"], json!("success"));
        assert_eq!(success["gpu_delta_percent"], json!(10.0));
    }

    #[test]
    fn options_roundtrip_restores_original() {
        let directory = std::env::temp_dir().join(format!("nivora-bench-opts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let options_path = directory.join("options.txt");
        fs::write(&options_path, "fov:70\nshaderPack:old.zip\n").unwrap();
        let original = read_options(&options_path);
        assert_eq!(original, "fov:70\nshaderPack:old.zip\n");
        write_options(&options_path, &apply_shader_selection(&original, Some("new.zip"), "1.20.1")).unwrap();
        assert_eq!(fs::read_to_string(&options_path).unwrap(), "fov:70\nshaderPack:new.zip\n");
        write_options(&options_path, "").unwrap();
        assert!(!options_path.exists());
        let _ = fs::remove_dir_all(&directory);
    }
}
