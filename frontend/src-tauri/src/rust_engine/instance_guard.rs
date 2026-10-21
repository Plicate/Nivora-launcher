//! Предохранитель от второй копии лаунчера.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{Emitter, Manager};

const LOCK_FILE: &str = "nivora-instance.lock";
const SHOW_FILE: &str = "nivora-show-request";
/// запрос «запустить экземпляр» от второй копии (ярлык на рабочем столе):
/// содержимое — имя экземпляра, свежесть — по mtime файла.
const LAUNCH_FILE: &str = "nivora-launch-request";
/// период сердцебиения: заметно меньше STALE_AFTER, чтобы jitter файловой
/// системы не заставил живой экземпляр выглядеть мёртвым.
const HEARTBEAT: Duration = Duration::from_millis(400);
const STALE_AFTER: u128 = 6;
/// просроченный запрос показа окна (остался после сбоя) игнорируем.
const SHOW_REQUEST_TTL: u128 = 60_000_000;

static WATCHER_STOP: AtomicBool = AtomicBool::new(false);
/// пути лок-файлов владельца: release() выполняется в момент выхода, когда
/// handle приложения уже недоступен — запоминаем при захвате.
static LOCK_PATHS: OnceLock<(PathBuf, PathBuf, PathBuf)> = OnceLock::new();

fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_micros())
        .unwrap_or(0)
}

fn lock_paths(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Не определить каталог кеша: {error}"))?;
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать {}: {error}", directory.display()))?;
    Ok((
        directory.join(LOCK_FILE),
        directory.join(SHOW_FILE),
        directory.join(LAUNCH_FILE),
    ))
}

fn read_heartbeat(path: &std::path::Path) -> u128 {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| raw.trim().parse::<u128>().ok())
        .unwrap_or(0)
}

pub fn acquire(app: tauri::AppHandle, launch_request: Option<&str>) -> Result<(), String> {
    let (lock, show, launch) = lock_paths(&app)?;
    let _ = LOCK_PATHS.set((lock.clone(), show.clone(), launch.clone()));
    for _ in 0..3 {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut file) => {
                let _ = write!(file, "{}", now_micros());
                spawn_watcher(app, lock.clone(), show, launch);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let heartbeat_age = now_micros().saturating_sub(read_heartbeat(&lock)) / 1_000_000;
                if heartbeat_age < STALE_AFTER {
                    // живой экземпляр: будим его окно и выходим. Если вторую
                    // копию поднял ярлык экземпляра — передаём и целевой
                    // экземпляр, чтобы живой лаунчер сам запустил игру.
                    if let Some(name) = launch_request {
                        let _ = fs::write(&launch, name);
                    }
                    let _ = fs::write(&show, now_micros().to_string());
                    return Err("already-running".to_owned());
                }
                // протухший лок (жёсткое завершение прошлой копии) — забираем.
                let _ = fs::remove_file(&lock);
                continue;
            }
            Err(error) => return Err(format!("Не удалось открыть {}: {error}", lock.display())),
        }
    }
    // третья попытка не удалась — не мешаем запуску, просто живём без защиты.
    Ok(())
}

/// Показ/фокус окна — строго в главном потоке: прямые вызовы GUI из
/// фоновых нитей на macOS не допускаются, а watcher живёт в своей.
fn bring_to_front(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = app.run_on_main_thread(move || {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        });
    }
}

/// фоновая нить владельца: сердцебиение + реакция на запрос показа окна
/// и на запрос запуска экземпляра от второй копии.
fn spawn_watcher(app: tauri::AppHandle, lock: PathBuf, show: PathBuf, launch: PathBuf) {
    std::thread::spawn(move || loop {
        if WATCHER_STOP.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&lock);
            let _ = fs::remove_file(&show);
            let _ = fs::remove_file(&launch);
            return;
        }
        std::thread::sleep(HEARTBEAT);
        if WATCHER_STOP.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&lock);
            let _ = fs::remove_file(&show);
            let _ = fs::remove_file(&launch);
            return;
        }
        // сердцебиение: перезапись содержимого обновляет «мгновение» жизни.
        if let Ok(mut file) = OpenOptions::new().write(true).truncate(true).open(&lock) {
            let _ = write!(file, "{}", now_micros());
        }
        // запрос запуска экземпляра от второй копии (ярлык).
        if launch.exists() {
            let requested_at = fs::metadata(&launch)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|age| age.as_micros())
                .unwrap_or(0);
            let name = fs::read_to_string(&launch).unwrap_or_default();
            let _ = fs::remove_file(&launch);
            let fresh = now_micros().saturating_sub(requested_at) <= SHOW_REQUEST_TTL;
            // имя проверяется тем же валидатором, что и argv: чужой мусор в
            // каталоге кеша не должен превращаться в запуск чего-либо.
            if fresh && super::shortcuts::set_pending_launch(name.trim()) {
                let pending = name.trim().to_owned();
                bring_to_front(&app);
                let _ = app.emit("nivora-launch-request", serde_json::json!({ "name": pending }));
            }
        }
        // запрос показа окна от второй копии.
        if show.exists() {
            let requested_at = fs::read_to_string(&show)
                .ok()
                .and_then(|raw| raw.trim().parse::<u128>().ok())
                .unwrap_or(0);
            let _ = fs::remove_file(&show);
            if now_micros().saturating_sub(requested_at) > SHOW_REQUEST_TTL {
                continue;
            }
            bring_to_front(&app);
            let _ = app.emit("nivora-brought-to-front", ());
        }
    });
}

pub fn release() {
    WATCHER_STOP.store(true, Ordering::SeqCst);
    if let Some((lock, show, launch)) = LOCK_PATHS.get() {
        let _ = fs::remove_file(lock);
        let _ = fs::remove_file(show);
        let _ = fs::remove_file(launch);
    }
}
