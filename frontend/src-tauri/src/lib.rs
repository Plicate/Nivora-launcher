#![recursion_limit = "512"]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

static TRAY_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

mod rust_engine;
mod smoke;

fn runtime_dir() -> Result<PathBuf, String> {
    rust_engine::ensure_runtime_dir()
}

fn cancellation_marker(operation_id: &str) -> Result<PathBuf, String> {
    if operation_id.is_empty()
        || !operation_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '-' || value == '_')
    {
        return Err("Некорректный идентификатор операции".to_owned());
    }
    Ok(runtime_dir()?.join(format!("nivora-launcher-{operation_id}.cancel")))
}

#[tauri::command]
fn cancel_operation(operation_id: String) -> Result<(), String> {
    let path = cancellation_marker(&operation_id)?;
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| format!("Не удалось остановить операцию: {error}"))
}

// журнал попыток открытия оверлея для диагностики
fn overlay_log_path() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|home| PathBuf::from(home).join(".local/share")))?;
    Some(data_home.join(rust_engine::state::LAUNCHER_DIR).join("logs").join("overlay.log"))
}

fn append_rotating(path: &Path, line: &str, limit: u64) -> Result<(), String> {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= limit {
            let _ = std::fs::rename(path, path.with_extension("log.old"));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог журнала: {error}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Не удалось открыть журнал: {error}"))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|error| format!("Не удалось записать журнал: {error}"))
}

#[tauri::command]
fn overlay_attempt_log(entry: Option<String>) -> Result<Value, String> {
    let path = overlay_log_path().ok_or_else(|| "Не удалось определить каталог данных".to_owned())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let line = entry.unwrap_or_default();
    if !line.trim().is_empty() {
        let environment = format!(
            "GDK_BACKEND={} session={} display={}",
            std::env::var("GDK_BACKEND").unwrap_or_default(),
            std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
            std::env::var("DISPLAY").unwrap_or_default(),
        );
        append_rotating(&path, &format!("[{stamp}] {line} | {environment}"), 128 * 1024)?;
    }
    let tail = std::fs::read_to_string(&path)
        .map(|content| content.chars().rev().take(4096).collect::<String>().chars().rev().collect::<String>())
        .unwrap_or_default();
    Ok(serde_json::json!({ "path": path.to_string_lossy(), "tail": tail }))
}

#[tauri::command]
async fn backend_call(app: AppHandle, request: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || backend_call_sync(app, request))
        .await
        .map_err(|error| format!("Backend task failed: {error}"))?
}

fn backend_call_sync(app: AppHandle, request: Value) -> Result<Value, String> {
    // обновляем sink при каждом вызове: операции могут начаться до setup
    {
        let handle = app.clone();
        rust_engine::events::set_event_sink(std::sync::Arc::new(move |value| {
            let _ = handle.emit("backend-event", value);
        }));
    }
    match rust_engine::registry::route(&request)? {
        rust_engine::registry::Route::Native => {
            let operation = rust_engine::registry::operation_of(&request)?;
            return rust_engine::handle(operation, &request);
        }
        rust_engine::registry::Route::BedrockLaunch => {
            return rust_engine::bedrock::launch_intercepted(&request);
        }
    }
}

#[cfg(windows)]
fn fatal_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "Nivora Launcher — ошибка запуска"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

#[cfg(not(windows))]
fn fatal_dialog(message: &str) {
    // в графическом запуске stderr может быть недоступен; используем системное окно, если оно есть
    #[cfg(target_os = "macos")]
    let script = format!(
        "display dialog \"{}\" with title \"Nivora Launcher — ошибка запуска\" buttons {{\"OK\"}} with icon stop",
        message.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
    );
    #[cfg(target_os = "macos")]
    let attempts: Vec<(&str, Vec<&str>)> = vec![("osascript", vec!["-e", script.as_str()])];
    #[cfg(not(target_os = "macos"))]
    let attempts: Vec<(&str, Vec<&str>)> = vec![
        (
            "zenity",
            vec!["--error", "--title", "Nivora Launcher — ошибка запуска", "--text", message],
        ),
        (
            "kdialog",
            vec!["--title", "Nivora Launcher — ошибка запуска", "--error", message],
        ),
        ("notify-send", vec!["-u", "critical", "Nivora Launcher", message]),
    ];
    for (program, args) in attempts {
        if Command::new(program)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return;
        }
    }
    eprintln!("{message}");
}

/// записывает панику в диагностический каталог; в release на Windows также показывает окно
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| location.to_string())
            .unwrap_or_else(|| "неизвестное место".to_owned());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "паника без текста".to_owned());
        let directory = diagnostic_dir();
        let _ = fs::create_dir_all(&directory);
        let trace = std::backtrace::Backtrace::force_capture();
        let content = format!(
            "version: {}\nlocation: {location}\npanic: {payload}\nbacktrace:\n{trace}\n",
            env!("CARGO_PKG_VERSION")
        );
        let _ = fs::write(directory.join("panic.log"), content);
        if cfg!(all(windows, not(debug_assertions))) {
            use std::sync::atomic::{AtomicBool, Ordering};
            static DIALOG_SHOWN: AtomicBool = AtomicBool::new(false);
            if !DIALOG_SHOWN.swap(true, Ordering::SeqCst) {
                fatal_dialog(&format!(
                    "Внутренняя ошибка Nivora Launcher (паника):\n\n{payload}\n\nМесто: {location}\n\nПодробности: {}",
                    diagnostic_dir().join("panic.log").display()
                ));
            }
        }
    }));
}

/// записывает код и адрес SEH-исключений, которые не попадают в panic-hook
#[cfg(windows)]
fn install_native_crash_filter() {
    use windows_sys::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        let record = (*info).ExceptionRecord;
        let code = (*record).ExceptionCode as u32;
        let address = (*record).ExceptionAddress as usize;
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("TEMP").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        let directory = base.join(rust_engine::state::LAUNCHER_DIR);
        let _ = fs::create_dir_all(&directory);
        let _ = fs::write(
            directory.join("native-crash.log"),
            format!(
                "version: {}\nexception_code: {code:#010x}\naddress: {address:#x}\n",
                env!("CARGO_PKG_VERSION")
            ),
        );
        use std::sync::atomic::{AtomicBool, Ordering};
        static DIALOG_SHOWN: AtomicBool = AtomicBool::new(false);
        if !DIALOG_SHOWN.swap(true, Ordering::SeqCst) {
            fatal_dialog(&format!(
                "Аварийный сбой Nivora Launcher.\n\nКод исключения: {code:#010x}\nАдрес: {address:#x}\n\nПодробности: %LOCALAPPDATA%\\nivora-launcher\\native-crash.log"
            ));
        }
        // код EXCEPTION_CONTINUE_SEARCH: отчёт об ошибке продолжает Windows
        0
    }

    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
    }
}

fn startup_trace(message: &str) {
    let base = diagnostic_dir();
    let _ = fs::create_dir_all(&base);
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(base.join("startup.log"))
    else {
        return;
    };
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "[{seconds}] v{} {message}", env!("CARGO_PKG_VERSION"));
}

/// каталог диагностики: %LOCALAPPDATA% в Windows, ~/.local/share в Linux
fn diagnostic_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join(rust_engine::state::LAUNCHER_DIR);
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join(rust_engine::state::LAUNCHER_DIR);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local/share")
            .join(rust_engine::state::LAUNCHER_DIR);
    }
    std::env::temp_dir().join(rust_engine::state::LAUNCHER_DIR)
}


fn diagnostic_log_hint() -> String {
    diagnostic_dir()
        .join("startup.log")
        .display()
        .to_string()
}

pub fn run() {
    install_panic_hook();
    #[cfg(windows)]
    install_native_crash_filter();
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "?".to_owned());
    startup_trace(&format!("запуск: exe={executable}"));
    // --launch-instance передаёт проверенное имя экземпляра в UI или запущенную копию
    let launch_argument = rust_engine::shortcuts::launch_argument(std::env::args().skip(1));
    if let Some(name) = &launch_argument {
        if !rust_engine::shortcuts::set_pending_launch(name) {
            startup_trace(&format!("run: --launch-instance отброшен (невалидное имя): {name}"));
        }
    }
    let app = match tauri::Builder::default()
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" || !TRAY_AVAILABLE.load(std::sync::atomic::Ordering::SeqCst) {
                    // без трея окно нельзя скрывать
                    return;
                }
                // закрытие скрывает окно в трей; полный выход доступен в меню значка
                api.prevent_close();
                let _ = window.hide();
                let _ = window.emit("nivora-hidden-to-tray", ());
            }
        })
        .setup(move |app| {
            // второй запуск будит существующий процесс и передаёт ему цель запуска
            if let Err(reason) =
                rust_engine::instance_guard::acquire(app.handle().clone(), launch_argument.as_deref())
            {
                if reason == "already-running" {
                    startup_trace("run: уже запущено — показываю окно первой копии и выхожу");
                    std::process::exit(0);
                }
                startup_trace(&format!("run: guard недоступен ({reason}) — продолжаю без него"));
            }
            // показываем ошибку setup в логе и окне
            if let Err(error) = setup_app(app) {
                let message = error.to_string();
                startup_trace(&format!("setup: ошибка: {message}"));
                fatal_dialog(&format!(
                    "Не удалось инициализировать приложение:\n\n{message}\n\nПодробности: {}",
                    diagnostic_log_hint()
                ));
                return Err(error);
            }
            if smoke::requested() {
                let handle = app.handle().clone();
                std::thread::spawn(move || smoke::run(handle));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![backend_call, cancel_operation, overlay_attempt_log])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            let message = error.to_string();
            startup_trace(&format!("run: ошибка: {message}"));
            fatal_dialog(&format!(
                "Не удалось запустить интерфейс:\n\n{message}\n\nПодробности: {}",
                diagnostic_log_hint()
            ));
            std::process::exit(1);
        }
    };
    app.run(|_app_handle, event| {
        // снимаем блокировку при штатном выходе; heartbeat покрывает аварийное завершение
        if let tauri::RunEvent::Exit = event {
            rust_engine::instance_guard::release();
        }
    });
}

fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    startup_trace("setup: начало");
    {
        let handle = app.handle().clone();
        rust_engine::events::set_event_sink(std::sync::Arc::new(move |value| {
            let _ = handle.emit("backend-event", value);
        }));
    }
    // ошибка конфигурации апдейтера не должна блокировать запуск
    #[cfg(desktop)]
    if let Err(error) = app.handle().plugin(tauri_plugin_updater::Builder::new().build()) {
        startup_trace(&format!("setup: updater не инициализирован: {error}"));
    }
    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_process::init())?;
    // горячие клавиши регистрируются только на время игровой сессии
    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_global_shortcut::Builder::new().build())?;
    startup_trace("setup: плагины готовы");
    // переносит данные из trial-launcher в nivora-launcher без перезаписи
    match rust_engine::state::migrate_legacy_roots() {
        Some(note) => startup_trace(&format!("setup: legacy-миграция — {note}")),
        None => startup_trace("setup: legacy-миграция не требуется"),
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
    }
    #[cfg(desktop)]
    match setup_tray(app) {
        Ok(()) => TRAY_AVAILABLE.store(true, std::sync::atomic::Ordering::SeqCst),
        // ошибка инициализации трея не должна блокировать запуск
        Err(error) => startup_trace(&format!("setup: трей недоступен ({error}) — без трея")),
    }
    Ok(())
}

#[cfg(desktop)]
fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Показать Nivora Launcher", true, None::<&str>)?;
    // меню трея позволяет открыть оверлей на Wayland
    let overlay = MenuItem::with_id(app, "overlay", "Игровой оверлей (Shift+Tab)", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &overlay, &quit])?;
    startup_trace("setup: меню готово");
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Nivora Launcher")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "overlay" => {
                let _ = app.emit("nivora-overlay-open", ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    startup_trace("setup: трей готов, окно создаётся");
    Ok(())
}

#[cfg(desktop)]
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use crate::rust_engine;
    use serde_json::json;

    fn allowed(operation: &str) -> bool {
        rust_engine::registry::route(&json!({ "operation": operation, "name": "" })).is_ok()
    }

    #[test]
    fn rust_native_operations_are_allowlisted_and_not_duplicated_in_python_routing() {
        // нативные операции проходят allowlist; запуск игры обрабатывает движок
        for operation in rust_engine::NATIVE_OPS {
            assert!(rust_engine::is_native(operation), "{operation}");
            assert!(allowed(operation), "нативная операция {operation} не прошла allowlist");
        }
    }

    #[test]
    fn launch_operations_are_native_and_bedrock_is_intercepted() {
        for operation in ["launch", "launch_instance"] {
            assert!(rust_engine::is_native(operation), "{operation}");
            assert!(
                matches!(
                    rust_engine::registry::route(&json!({ "operation": operation, "name": "" })),
                    Ok(rust_engine::registry::Route::Native)
                ),
                "{operation} должен идти в движок без экземпляра Bedrock"
            );
        }
    }

    #[test]
    fn github_catalog_operations_are_allowed() {
        for operation in [
            "github_plugin_status",
            "github_plugin_search",
            "github_plugin_details",
            "github_plugin_install",
            "github_plugin_oauth_start",
            "github_plugin_oauth_poll",
            "github_plugin_sign_out",
            "github_plugin_publish",
        ] {
            assert!(allowed(operation), "{operation}");
        }
    }

    #[test]
    fn removed_lan_session_operations_are_rejected() {
        for operation in ["lan_session_create", "lan_session_join", "lan_session_status", "lan_session_leave"] {
            assert!(!allowed(operation), "{operation}");
        }
    }

    #[test]
    fn instance_repair_version_operation_is_allowed() {
        assert!(allowed("instance_repair_version"));
    }

    #[test]
    fn server_network_operations_are_allowed() {
        for operation in ["server_network_diagnose", "server_network_status", "server_network_prepare", "server_network_open", "server_network_close"] {
            assert!(allowed(operation), "{operation}");
        }
    }

    #[test]
    fn bedrock_operations_are_allowed_by_the_desktop_bridge() {
        for operation in [
            "bedrock_runtime_status",
            "bedrock_versions",
            "bedrock_setup_version",
            "bedrock_setup_status",
            "bedrock_setup_cancel",
            "bedrock_create_instance",
            "bedrock_content_list",
            "bedrock_content_import",
            "bedrock_content_remove",
            "bedrock_store_login",
            "bedrock_store_login_status",
            "bedrock_store_login_cancel",
            "bedrock_account_status",
            "bedrock_stop",
            "system_notify",
            "overlay_state",
            "recording_start",
            "recording_stop",
            "recording_status",
        ] {
            assert!(allowed(operation), "{operation}");
        }
    }

    #[test]
    fn plugin_v2_security_and_runtime_operations_are_allowed() {
        for operation in [
            "plugin_security_status",
            "plugin_permission_grants_set",
            "plugin_local_untrusted_execution",
            "plugin_publisher_pin",
            "plugin_publisher_unpin",
            "plugin_local_revoke",
            "plugin_local_unrevoke",
            "plugin_api_source",
            "plugin_api_runtime_info",
            "plugin_api_capabilities",
            "plugin_api_quota_usage",
            "plugin_api_permissions_get",
            "plugin_api_permissions_set",
            "plugin_api_authorize",
            "plugin_api_open_external",
            "plugin_api_files_list",
            "plugin_api_files_read",
            "plugin_api_files_write",
            "plugin_api_files_remove",
            "plugin_api_config_projected_read",
            "plugin_http_request",
            "plugin_api_instances_list",
            "plugin_api_process_metrics",
            "plugin_api_proxy_routes_get",
            "plugin_api_proxy_routes_save",
            "plugin_api_proxy_routes_add",
            "plugin_api_proxy_routes_remove",
        ] {
            assert!(allowed(operation), "{operation}");
        }
    }
}
