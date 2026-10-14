#![recursion_limit = "512"]

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// Операции GUI, обслуживаемые локально, без воркера.
mod rust_engine;

#[cfg(not(windows))]
fn python_path() -> Option<String> {
    if Path::new("/usr/lib/nivora-launcher").is_dir() {
        return Some("/usr/lib/nivora-launcher".to_owned());
    }

    let mut directory = std::env::current_exe().ok()?.parent().map(Path::to_path_buf)?;
    for _ in 0..8 {
        if directory.join("nivora_launcher").is_dir() {
            return Some(directory.to_string_lossy().into_owned());
        }
        if !directory.pop() {
            break;
        }
    }
    None
}

fn runtime_dir() -> Result<PathBuf, String> {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nivora-launcher-runtime"));
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать runtime-каталог: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Не удалось ограничить runtime-каталог: {error}"))?;
    Ok(directory)
}

fn backend_log_path() -> Result<PathBuf, String> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("USERPROFILE")
                            .map(|home| PathBuf::from(home).join("AppData/Local"))
                    })
                    .unwrap_or_else(std::env::temp_dir)
            } else {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
                    .unwrap_or_else(std::env::temp_dir)
            }
        });
    let directory = data_home.join(rust_engine::state::LAUNCHER_DIR).join("logs");
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать каталог логов: {error}"))?;
    let path = directory.join("backend.log");
    #[cfg(unix)]
    if path.exists() {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Не удалось ограничить лог backend: {error}"))?;
    }
    Ok(path)
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

fn backend_executable_name() -> &'static str {
    if cfg!(windows) {
        "nivora-launcher-backend.exe"
    } else {
        "nivora-launcher-backend"
    }
}

fn backend_resource_path(resource_dir: &Path) -> PathBuf {
    resource_dir.join("backend").join(backend_executable_name())
}

fn bundled_backend_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    // 1. Установленный режим: ресурс рядом с приложением (resource_dir()/backend/…).
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(backend_resource_path(&resource_dir));
    }
    // 2–3. Portable-режим: каталог самого nivora-launcher.exe. Поддерживаем оба
    //    расклада — <dir>/backend/nivora-launcher-backend.exe и просто два exe
    //    рядом (так лежат артефакты dist/windows — раньше это не работало, и
    //    приложение падало в fallback на системный python).
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            candidates.push(backend_resource_path(exe_dir));
            candidates.push(exe_dir.join(backend_executable_name()));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

// Встроенный бэкенд: nivora-launcher-backend.exe упаковывается внутрь самого
// nivora-launcher.exe (include_bytes!), поэтому портативная сборка — ОДИН файл.
// На Linux (dev и Arch-пакет) файла нет и он не нужен — там работает системный
// python3, поэтому const и функция целиком выключены через cfg(windows).
// Для dev-сборки на Windows бэкенд собирается заранее: scripts/build_windows_exe.sh --only-backend.
#[cfg(windows)]
const EMBEDDED_BACKEND_EXE: &[u8] =
    include_bytes!("../resources/backend/nivora-launcher-backend.exe");

// Распаковывает встроенный бэкенд в %LOCALAPPDATA%\nivora-launcher\backend\
// (писать рядом с exe нельзя: Program Files доступен только для чтения) и
// возвращает путь. Имя файла содержит версию приложения; при несовпадении
// размера/хеша бэкенд перезаписывается, файлы прошлых версий удаляются.
#[cfg(windows)]
fn embedded_backend_exe() -> Result<PathBuf, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher as _;

    fn content_hash(bytes: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        hasher.write(bytes);
        hasher.finish()
    }

    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TEMP").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let directory = base.join(rust_engine::state::LAUNCHER_DIR).join("backend");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Не удалось создать каталог бэкенда: {error}"))?;
    let file_name = format!("nivora-launcher-backend-v{}.exe", env!("CARGO_PKG_VERSION"));
    let target = directory.join(&file_name);
    let up_to_date = match fs::read(&target) {
        Ok(existing) => {
            existing.len() == EMBEDDED_BACKEND_EXE.len()
                && content_hash(&existing) == content_hash(EMBEDDED_BACKEND_EXE)
        }
        Err(_) => false,
    };
    if !up_to_date {
        let temporary = directory.join(format!("{file_name}.tmp"));
        fs::write(&temporary, EMBEDDED_BACKEND_EXE)
            .and_then(|()| fs::rename(&temporary, &target))
            .map_err(|error| format!("Не удалось распаковать встроенный backend: {error}"))?;
    }
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let raw_name = entry.file_name();
            let name = raw_name.to_string_lossy();
            let name: &str = &name;
            if name.starts_with("nivora-launcher-backend") && name != file_name.as_str() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(target)
}

struct BackendWorker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl BackendWorker {
    fn spawn(app: &AppHandle) -> Result<Self, String> {
        let stderr_path = backend_log_path()?;
        let stderr_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)
            .map_err(|error| format!("Не удалось открыть лог Python backend: {error}"))?;
        #[cfg(unix)]
        fs::set_permissions(&stderr_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Не удалось ограничить лог Python backend: {error}"))?;
        // Порядок запуска бэкенда:
        //   Windows — 1) exe-файл рядом (ресурс установщика / portable-раскладка),
        //             2) встроенный в nivora-launcher.exe бэкенд (один файл!);
        //   Linux   — 1) exe-файл рядом, 2) системный python3 (dev и Arch-пакет).
        // Системный python на Windows не используется вовсе: на чистой системе
        // это заглушка Store («Python was not found», окно консоли).
        #[cfg(windows)]
        let mut command = if let Some(executable) = bundled_backend_path(app) {
            startup_trace(&format!("бэкенд: файл рядом с приложением: {}", executable.display()));
            Command::new(executable)
        } else {
            match embedded_backend_exe() {
                Ok(executable) => {
                    startup_trace(&format!("бэкенд: встроенный, распакован: {}", executable.display()));
                    Command::new(executable)
                }
                Err(error) => {
                    startup_trace(&format!("бэкенд: распаковка встроенного не удалась: {error}"));
                    return Err(error);
                }
            }
        };
        #[cfg(not(windows))]
        let mut command = if let Some(executable) = bundled_backend_path(app) {
            startup_trace(&format!("бэкенд: файл рядом с приложением: {}", executable.display()));
            Command::new(executable)
        } else {
            startup_trace("бэкенд: системный python3");
            let mut command = Command::new("python3");
            command.args(["-m", "nivora_launcher.gui_service"]);
            if let Some(path) = python_path() {
                command.env("PYTHONPATH", path);
            }
            command
        };
        command
            // Python на Windows по умолчанию пишет в пайпы в ANSI-кодировке
            // (cp1251) — русские строки в JSON ломают UTF-8-протокол
            // («stream did not contain valid UTF-8»). Принудительный UTF-8.
            .env("PYTHONUTF8", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr_file));
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("Не удалось запустить Python backend: {error}");
                startup_trace(&format!("бэкенд: {message}"));
                return Err(message);
            }
        };
        startup_trace("бэкенд: процесс запущен, рукопожатие");
        let input = child
            .stdin
            .take()
            .ok_or_else(|| "Python backend не открыл stdin".to_owned())?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| "Python backend не открыл stdout".to_owned())?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output),
        })
    }

    fn request(&mut self, app: &AppHandle, request: &Value) -> Result<Value, String> {
        validate_backend_request(request)?;
        if self
            .child
            .try_wait()
            .map_err(|error| format!("Не удалось проверить Python backend: {error}"))?
            .is_some()
        {
            return Err("Python backend неожиданно завершился".to_owned());
        }
        serde_json::to_writer(&mut self.input, request)
            .map_err(|error| format!("Не удалось передать GUI-запрос: {error}"))?;
        self.input
            .write_all(b"\n")
            .and_then(|_| self.input.flush())
            .map_err(|error| format!("Не удалось завершить GUI-запрос: {error}"))?;

        let mut buffer: Vec<u8> = Vec::new();
        loop {
            buffer.clear();
            let count = self
                .output
                .read_until(b'\n', &mut buffer)
                .map_err(|error| format!("Не удалось прочитать Python backend: {error}"))?;
            if count == 0 {
                return Err("Python backend закрыл stdout".to_owned());
            }
            // Читаем байты и декодируем с потерями: один «кривой» символ
            // не должен убивать весь поток (read_line падает на не-UTF-8).
            let line = String::from_utf8_lossy(&buffer);
            let text = line.trim_end();
            let value: Value = serde_json::from_str(text)
                .map_err(|error| format!("Некорректное событие Python backend: {error}"))?;
            if let Some(event) = value.get("event") {
                app.emit("backend-event", event)
                    .map_err(|error| format!("Не удалось передать событие в UI: {error}"))?;
            } else {
                return Ok(value);
            }
        }
    }
}

impl Drop for BackendWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static BACKEND_WORKER: OnceLock<Mutex<Option<BackendWorker>>> = OnceLock::new();

fn backend_worker() -> &'static Mutex<Option<BackendWorker>> {
    BACKEND_WORKER.get_or_init(|| Mutex::new(None))
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

// Журнал попыток открыть окно оверлея: каждая попытка дописывается строкой
// (режим, план окна, результат, ошибка). Читается командой той же команды —
// она возвращает путь и хвост журнала. Без этого файла «оверлей не
// открывается» диагностировался вслепую.
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
    // Родные Rust-операции (хеширование, инвентаризация каталогов) идут
    // мимо Python-воркера: тот же allowlist-гейт, но обслуживаются нативно —
    // без spawn, без блокировки воркера и без python3 вообще. Ветке воркера
    // отдаются и нативные операции, которым нужен WebDAV (см. needs_worker).
    if let Some(operation) = request.get("operation").and_then(Value::as_str) {
        if rust_engine::is_native(operation) && !rust_engine::needs_worker(operation, &request) {
            validate_backend_request(&request)?;
            return rust_engine::handle(operation, &request);
        }
    }

    // Запуск Bedrock-экземпляров ведёт Rust-движок (Linux — BedrockOnLinux,
    // Windows — официальный Minecraft for Windows): игровая сессия больше не
    // требует отдельного Python-воркера. Allowlist проверяется ДО запуска.
    if rust_engine::bedrock::should_intercept_launch(&request) {
        validate_backend_request(&request)?;
        return rust_engine::bedrock::launch_intercepted(&request);
    }

    if uses_transient_backend_worker(&request) {
        let mut worker = BackendWorker::spawn(&app)?;
        let response = worker.request(&app, &request)?;
        return parse_backend_response(response);
    }

    let worker_mutex = backend_worker();
    let mut worker_guard = worker_mutex
        .lock()
        .map_err(|_| "Python backend lock повреждён".to_owned())?;
    if worker_guard.is_none() {
        *worker_guard = Some(BackendWorker::spawn(&app)?);
    }
    let response = match worker_guard.as_mut().expect("backend worker initialized").request(&app, &request) {
        Ok(value) => value,
        Err(error) => {
            worker_guard.take();
            return Err(error);
        }
    };
    parse_backend_response(response)
}

fn uses_transient_backend_worker(request: &Value) -> bool {
    matches!(
        request.get("operation").and_then(Value::as_str),
        Some("launch") | Some("launch_instance")
    )
}

fn parse_backend_response(response: Value) -> Result<Value, String> {
    if response.get("ok") == Some(&Value::Bool(false)) {
        return Err(response
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| "Python backend завершился с ошибкой".to_owned()));
    }

    Ok(response)
}

fn validate_backend_request(request: &Value) -> Result<(), String> {
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| "GUI-запрос не содержит operation".to_owned())?;
    const ALLOWED: &[&str] = &[
        "bootstrap", "versions", "java_options", "java_validate", "graphics_diagnostics", "security_status", "loader_choices", "save_settings",
        "cloud_status", "cloud_configure", "cloud_test", "cloud_list", "cloud_sync", "cloud_restore", "cloud_delete", "cloud_disconnect",
        "wallpaper_import", "wallpaper_clear", "create_offline_account", "login_microsoft", "microsoft_login_status", "delete_account", "set_default_account", "move_account",
        "server_network_diagnose", "server_network_status", "server_network_prepare", "server_network_open", "server_network_close",
        "server_cores", "server_core_versions", "server_plugin_catalogs", "save_server_plugin_catalogs", "server_create", "server_details", "server_save_config", "server_upload_core", "server_core_download", "server_start", "server_stop", "server_restart", "server_command", "server_log", "server_status", "server_file_list", "server_file_read", "server_file_write", "server_file_mkdir", "server_file_upload", "server_file_delete", "server_plugin_search", "server_plugin_details", "server_plugin_updates", "server_plugin_update", "server_plugin_install", "server_plugin_remove", "server_snapshot_list", "server_snapshot_create", "server_snapshot_restore", "server_snapshot_delete",
        "delete_instance", "duplicate_instance", "external_instances_scan", "external_instance_import", "instance_details", "instance_snapshot_list", "instance_snapshot_create", "instance_snapshot_restore", "instance_snapshot_delete", "instance_groups", "create_instance_group", "delete_instance_group", "update_instance_profile", "instance_log", "instance_file_list", "instance_file_read", "instance_file_write", "instance_file_mkdir", "instance_file_upload", "instance_file_delete", "instance_log_archives", "instance_log_archive_read", "save_instance_settings",
        "plugin_dev_changes", "plugin_api_operation_trace", "plugin_list", "plugin_install", "plugin_set_enabled", "plugin_uninstall", "plugin_security_status", "plugin_permission_grants_set", "plugin_local_untrusted_execution", "plugin_publisher_pin", "plugin_publisher_unpin", "plugin_local_revoke", "plugin_local_unrevoke", "plugin_api_source", "plugin_api_runtime_info", "plugin_api_capabilities", "plugin_api_quota_usage", "plugin_api_permissions_get", "plugin_api_permissions_set", "plugin_api_authorize", "plugin_api_open_external", "open_external", "plugin_api_files_list", "plugin_api_files_read", "plugin_api_files_write", "plugin_api_files_remove", "plugin_api_config_projected_read", "plugin_http_request", "plugin_api_instances_list", "plugin_api_process_metrics", "plugin_api_proxy_routes_get", "plugin_api_proxy_routes_save", "plugin_api_proxy_routes_add", "plugin_api_proxy_routes_remove", "plugin_network_fetch", "plugin_network_dns", "plugin_network_ping", "plugin_network_port_check", "plugin_server_list_get", "plugin_server_list_save", "plugin_server_list_add", "plugin_server_list_remove", "plugin_storage_get", "plugin_storage_set", "plugin_storage_remove", "plugin_config_get", "plugin_config_set", "plugin_config_remove", "plugin_file_list", "plugin_file_read", "plugin_file_write", "plugin_file_remove", "plugin_database_query", "plugin_database_execute", "lan_session_create", "lan_session_join", "lan_session_status", "lan_session_leave", "github_plugin_status", "github_plugin_search", "github_plugin_details", "github_plugin_install", "github_plugin_oauth_start", "github_plugin_oauth_poll", "github_plugin_sign_out", "github_plugin_publish",
        "remove_instance_asset", "toggle_mod", "import_asset", "install", "launch", "launch_instance", "open_folder", "modpack_import", "mods_search", "mods_details", "mods_check_updates", "mods_update", "mods_install", "mods_remove", "mod_audit_prepare", "mod_audit_accept", "mod_audit_cancel", "mod_audit_status", "skins_list", "skins_import", "skin_profile", "apply_skin", "clear_skin", "apply_cape", "clear_cape", "open_skins_folder",
        "bedrock_runtime_status", "bedrock_versions", "bedrock_setup_version", "bedrock_setup_status", "bedrock_setup_cancel", "bedrock_create_instance", "bedrock_content_list", "bedrock_content_import", "bedrock_content_remove", "bedrock_store_login", "bedrock_store_login_status", "bedrock_store_login_cancel", "bedrock_account_status", "bedrock_screenshots_list", "bedrock_stop", "rename_instance_group",
        "system_notify", "recording_start", "recording_stop", "recording_status",
        // Профили настроек, бэкапы миров, темы оформления и обслуживание кеша
        // раньше отбивались ошибкой «Операция GUI не разрешена desktop bridge»,
        // хотя бэкенд их полностью поддерживает.
        "settings_profiles_list", "settings_profile_save", "settings_profile_apply", "settings_profile_delete", "settings_profile_export", "settings_profile_import",
        "world_backup_list", "world_backup_create", "world_backup_restore", "world_backup_delete", "world_backup_schedule_get", "world_backup_schedule_set",
        "theme_list", "theme_preview", "theme_save", "theme_delete", "theme_import", "theme_export", "github_theme_search", "github_theme_install",
        "mod_dependency_graph", "mod_removal_impact", "export_modpack", "optimization_analyze", "optimization_apply",
        "plugin_publisher_revocation_authority_set", "plugin_revocations_import",
        // Родные Rust-операции: обслуживаются движком в rust_engine.rs
        // и не требуют Python-воркера. Список = rust_engine::NATIVE_OPS.
        "hash_file", "hash_directory", "engine_info", "system_metrics", "overlay_state",
        "jvm_presets_list", "launch_history", "cache_stats", "cache_prune",
    ];
    if !ALLOWED.contains(&operation) {
        return Err("Операция GUI не разрешена desktop bridge".to_owned());
    }
    Ok(())
}

/// Любой сбой старта обязан быть ВИДИМЫМ. У оконной подсистемы нет консоли,
/// «тихое закрытие» не даёт пользователю ничего — поэтому фатальная ошибка
/// показывается системным окном сообщения (MessageBoxW), а не только логом.
/// Вызывается уже после записи в startup.log/panic.log.
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
fn fatal_dialog(_message: &str) {}

/// Паники оконной сборки никто не видит: у windows-подсистемы нет консоли,
/// stderr отброшен, и падение на старте выглядит как «лаунчер просто
/// закрывается». Хук дописывает панику с местом и трассировкой в
/// %LOCALAPPDATA%\nivora-launcher\panic.log (на unix — во временный каталог),
/// чтобы краш можно было диагностировать по файлу. В release на Windows
/// паника дополнительно показывается окном — «молча» приложение больше
/// не умирает никогда.
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
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        let directory = base.join(rust_engine::state::LAUNCHER_DIR);
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
                    "Внутренняя ошибка Nivora Launcher (паника):\n\n{payload}\n\nМесто: {location}\n\nПодробности: %LOCALAPPDATA%\\nivora-launcher\\panic.log"
                ));
            }
        }
    }));
}

/// Нативные краши (access violation и другие SEH-исключения) паниками
/// не являются — panic-хук их не видит, и падение остаётся «тихим».
/// Фильтр необработанных исключений дописывает код и адрес сбоя в
/// native-crash.log рядом с panic.log.
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
        // Нативный краш тоже показываем окном: иначе он выглядит как
        // мгновенное «тихое» закрытие без единого сообщения.
        use std::sync::atomic::{AtomicBool, Ordering};
        static DIALOG_SHOWN: AtomicBool = AtomicBool::new(false);
        if !DIALOG_SHOWN.swap(true, Ordering::SeqCst) {
            fatal_dialog(&format!(
                "Аварийный сбой Nivora Launcher.\n\nКод исключения: {code:#010x}\nАдрес: {address:#x}\n\nПодробности: %LOCALAPPDATA%\\nivora-launcher\\native-crash.log"
            ));
        }
        // EXCEPTION_CONTINUE_SEARCH: отчёт об ошибке продолжает Windows.
        0
    }

    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
    }
}

/// Трассировка старта: установленная (setup) версия падала молча, и было
/// невозможно узнать, как далеко она дошла. Каждая веха запуска дописывается
/// в %LOCALAPPDATA%\\nivora-launcher\\startup.log (на unix — в каталог данных
/// окружения) — при любом странном поведении файл сразу даёт картину.
fn startup_trace(message: &str) {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let directory = base.join(rust_engine::state::LAUNCHER_DIR);
    let _ = fs::create_dir_all(&directory);
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("startup.log"))
    else {
        return;
    };
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "[{seconds}] v{} {message}", env!("CARGO_PKG_VERSION"));
}

pub fn run() {
    install_panic_hook();
    #[cfg(windows)]
    install_native_crash_filter();
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "?".to_owned());
    startup_trace(&format!("запуск: exe={executable}"));
    if let Err(error) = tauri::Builder::default()
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Крестик НЕ завершает приложение: игрок часто закрывает
                // лаунчер «на время игры», а вместе с ним умирали глобальные
                // клавиши и оверлей. Прячем окно в трей; полный выход —
                // «Выйти» в меню значка. Для окна оверлея — то же самое.
                api.prevent_close();
                let _ = window.hide();
                let _ = window.emit("nivora-hidden-to-tray", ());
            }
        })
        .setup(|app| {
            // Ошибка setup раньше уходила в никуда: окно не создавалось, и
            // приложение «просто закрывалось». Теперь — лог + видимое окно.
            if let Err(error) = setup_app(app) {
                let message = error.to_string();
                startup_trace(&format!("setup: ошибка: {message}"));
                fatal_dialog(&format!(
                    "Не удалось инициализировать приложение:\n\n{message}\n\nПодробности: %LOCALAPPDATA%\\nivora-launcher\\startup.log"
                ));
                return Err(error);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![backend_call, cancel_operation, overlay_attempt_log])
        .run(tauri::generate_context!())
    {
        // Раньше здесь стоял .expect(): паника писалась в panic.log, но на
        // экране (оконная подсистема, без консоли) выглядела мгновенным
        // закрытием. Показываем причину и выходим с явным кодом.
        let message = error.to_string();
        startup_trace(&format!("run: ошибка: {message}"));
        fatal_dialog(&format!(
            "Не удалось запустить интерфейс:\n\n{message}\n\nПодробности: %LOCALAPPDATA%\\nivora-launcher\\startup.log"
        ));
        std::process::exit(1);
    }
}

fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    startup_trace("setup: начало");
    // Апдейтер не должен убивать лаунчер: сервера обновлений в этой
    // сборке нет (endpoints пустые), а отсутствие/кривизна его конфига
    // раньше валила всё приложение до создания окна. UI при вызове
    // проверки и так вежливо сообщает, что функция не настроена.
    #[cfg(desktop)]
    if let Err(error) = app.handle().plugin(tauri_plugin_updater::Builder::new().build()) {
        startup_trace(&format!("setup: updater не инициализирован: {error}"));
    }
    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_process::init())?;
    // Горячие клавиши игрового оверлея (Shift+Tab) и записи (F7).
    // Регистрируются из JS только на время игровой сессии.
    // (в v2.4.x у плагина нет init() — только Builder::new().build())
    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_global_shortcut::Builder::new().build())?;
    startup_trace("setup: плагины готовы");
    // Одноразовая миграция бренда: данные пользователя из исторического
    // «trial-launcher» переезжают в актуальный «nivora-launcher»
    // (переименование целиком либо слияние без затирания).
    match rust_engine::state::migrate_legacy_roots() {
        Some(note) => startup_trace(&format!("setup: legacy-миграция — {note}")),
        None => startup_trace("setup: legacy-миграция не требуется"),
    }
    let show = MenuItem::with_id(app, "show", "Показать Nivora Launcher", true, None::<&str>)?;
    // Запасной способ открыть оверлей: на Wayland глобальные клавиши
    // не работают в принципе, а пункт меню в трее — работает везде.
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
                // Главное окно владеет состоянием оверлея и создаст его
                // окно при необходимости — сюда только сигнал.
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

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::{backend_resource_path, uses_transient_backend_worker, validate_backend_request};
    use crate::rust_engine;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn rust_native_operations_are_allowlisted_and_not_duplicated_in_python_routing() {
        // Контракт: каждая нативная операция обязана проходить allowlist
        // (иначе мост отдаёт «Операция GUI не разрешена» до движка), а
        // запуски игр по-прежнему уходят в transient-воркер.
        for operation in rust_engine::NATIVE_OPS {
            assert!(rust_engine::is_native(operation), "{operation}");
            assert!(
                validate_backend_request(&json!({ "operation": operation })).is_ok(),
                "нативная операция {operation} не прошла allowlist"
            );
            assert!(!uses_transient_backend_worker(&json!({ "operation": operation })));
        }
    }

    #[test]
    fn backend_resource_path_uses_the_platform_executable_name() {
        let executable = if cfg!(windows) {
            "nivora-launcher-backend.exe"
        } else {
            "nivora-launcher-backend"
        };
        assert_eq!(
            backend_resource_path(Path::new("resources")),
            Path::new("resources/backend").join(executable)
        );
    }

    #[test]
    fn transient_backend_worker_is_used_only_for_launch_operations() {
        assert!(uses_transient_backend_worker(&json!({"operation": "launch"})));
        assert!(uses_transient_backend_worker(&json!({"operation": "launch_instance"})));
        assert!(!uses_transient_backend_worker(&json!({"operation": "server_start"})));
        assert!(!uses_transient_backend_worker(&json!({})));
    }

    #[test]
    fn lan_and_github_catalog_operations_are_allowed() {
        for operation in [
            "lan_session_create",
            "lan_session_join",
            "lan_session_status",
            "lan_session_leave",
            "github_plugin_status",
            "github_plugin_search",
            "github_plugin_details",
            "github_plugin_install",
            "github_plugin_oauth_start",
            "github_plugin_oauth_poll",
            "github_plugin_sign_out",
            "github_plugin_publish",
        ] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
        }
    }

    #[test]
    fn server_network_operations_are_allowed() {
        for operation in ["server_network_diagnose", "server_network_status", "server_network_prepare", "server_network_open", "server_network_close"] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
            assert!(!uses_transient_backend_worker(&json!({"operation": operation})));
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
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
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
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
        }
    }
}
