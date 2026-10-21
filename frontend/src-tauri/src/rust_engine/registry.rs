//! Единый реестр GUI-операций desktop-моста.
//!
//! Раньше белый список жил отдельным массивом в `lib.rs`, нативные операции —
//! в `NATIVE_OPS`, а операции воркера — только в диспетчере `gui_service.py`.
//! Три списка расходились, и новая функция падала с «Операция GUI не
//! разрешена desktop bridge». Теперь разрешено ровно объединение
//! `NATIVE_OPS ∪ WORKER_OPS`, маршрут выбирает одна функция [`route`], а
//! тесты ниже сверяют реестр с фронтендом, диспетчером движка и воркером.
use serde_json::Value;

use super::{bedrock, is_native, needs_worker, NATIVE_OPS};

/// Операции, которые обслуживает только Python-воркер. Нативные операции,
/// временно уходящие в воркер по условию (`needs_worker`), здесь не
/// дублируются — они в `NATIVE_OPS`.
pub const WORKER_OPS: &[&str] = &[
    "auto_repair_apply", "auto_repair_diagnose", "auto_repair_mute", "export_modpack",
    "external_instance_import", "github_theme_install", "github_theme_search",
    "graphics_diagnostics", "instance_log", "instance_repair_version", "instance_snapshot_create",
    "instance_snapshot_delete", "instance_snapshot_list", "instance_snapshot_restore",
    "instance_version_import", "instance_version_override_save", "instance_versions", "launch",
    "launch_instance", "mod_audit_accept", "mod_audit_cancel", "mod_audit_prepare",
    "mod_audit_status", "modpack_import", "mods_install", "mods_remove", "mods_update",
    "open_external", "open_folder", "optimization_apply", "plugin_api_authorize",
    "plugin_api_capabilities", "plugin_api_config_projected_read", "plugin_api_files_list",
    "plugin_api_files_read", "plugin_api_files_remove", "plugin_api_files_write",
    "plugin_api_instances_list", "plugin_api_open_external", "plugin_api_operation_trace",
    "plugin_api_permissions_get", "plugin_api_permissions_set", "plugin_api_process_metrics",
    "plugin_api_proxy_routes_add", "plugin_api_proxy_routes_get", "plugin_api_proxy_routes_remove",
    "plugin_api_proxy_routes_save", "plugin_api_runtime_info", "plugin_api_source",
    "plugin_config_get", "plugin_config_remove", "plugin_config_set", "plugin_database_execute",
    "plugin_database_query", "plugin_file_list", "plugin_file_read", "plugin_file_remove",
    "plugin_file_write", "plugin_http_request", "plugin_install", "plugin_local_revoke",
    "plugin_local_unrevoke", "plugin_local_untrusted_execution", "plugin_network_dns",
    "plugin_network_fetch", "plugin_network_ping", "plugin_network_port_check",
    "plugin_permission_grants_set", "plugin_publisher_pin",
    "plugin_publisher_revocation_authority_set", "plugin_publisher_unpin",
    "plugin_revocations_import", "plugin_security_status", "plugin_server_list_add",
    "plugin_server_list_get", "plugin_server_list_remove", "plugin_server_list_save",
    "plugin_set_enabled", "plugin_storage_get", "plugin_storage_remove", "plugin_storage_set",
    "plugin_uninstall", "save_server_plugin_catalogs", "server_core_download", "server_create",
    "server_details", "server_file_delete", "server_file_list", "server_file_mkdir",
    "server_file_read", "server_file_upload", "server_file_write", "server_plugin_catalogs",
    "server_plugin_details", "server_plugin_install", "server_plugin_remove",
    "server_plugin_search", "server_plugin_update", "server_plugin_updates", "server_save_config",
    "server_snapshot_create", "server_snapshot_delete", "server_snapshot_list",
    "server_snapshot_restore", "server_upload_core", "settings_profile_apply",
    "settings_profile_save", "shader_benchmark_start", "shader_benchmark_status", "system_notify",
    "update_instance_profile", "wallpaper_clear", "wallpaper_import", "world_import_begin",
    "world_import_cancel", "world_import_chunk", "world_import_finish",
];

/// Операции запуска игры: обслуживаются отдельным короткоживущим воркером,
/// чтобы долгая игровая сессия не держала общий воркер.
const TRANSIENT_WORKER_OPS: &[&str] = &["launch", "launch_instance"];

/// Куда отправить разрешённый запрос.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Rust-движок в процессе Tauri.
    Native,
    /// Запуск Bedrock-экземпляра движком (перехват `launch_instance`).
    BedrockLaunch,
    /// Отдельный воркер на время игровой сессии.
    TransientWorker,
    /// Общий Python-воркер.
    Worker,
}

pub fn is_allowed(operation: &str) -> bool {
    NATIVE_OPS.contains(&operation) || WORKER_OPS.contains(&operation)
}

pub fn operation_of(request: &Value) -> Result<&str, String> {
    request
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| "GUI-запрос не содержит operation".to_owned())
}

/// Проверка белого списка. Формат ошибки сохранён: фронтенд и журналы
/// поддержки ищут именно эту строку.
pub fn validate(request: &Value) -> Result<&str, String> {
    let operation = operation_of(request)?;
    if !is_allowed(operation) {
        return Err(format!("Операция GUI не разрешена desktop bridge: {operation}"));
    }
    Ok(operation)
}

/// Единственная точка выбора обработчика: сначала белый список, затем
/// маршрут. `lib.rs` только исполняет выбранный маршрут.
pub fn route(request: &Value) -> Result<Route, String> {
    let operation = validate(request)?;
    if is_native(operation) && !needs_worker(operation, request) {
        return Ok(Route::Native);
    }
    if bedrock::should_intercept_launch(request) {
        return Ok(Route::BedrockLaunch);
    }
    if TRANSIENT_WORKER_OPS.contains(&operation) {
        return Ok(Route::TransientWorker);
    }
    Ok(Route::Worker)
}

pub fn uses_transient_worker(request: &Value) -> bool {
    request
        .get("operation")
        .and_then(Value::as_str)
        .map(|operation| TRANSIENT_WORKER_OPS.contains(&operation))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn project_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// Строковые литералы-операции в `request<...>('op'` и `request('op'`.
    /// Дженерики фронтенда бывают вложенными (`Array<{...}>`, `import('...')`),
    /// поэтому скобки не разбираются регуляркой, а пропускаются счётчиком.
    fn frontend_operations() -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![project_root().join("frontend/src")];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("");
                if !matches!(extension, "ts" | "vue") {
                    continue;
                }
                collect_requests(&read(&path), &mut found);
            }
        }
        found
    }

    fn collect_requests(source: &str, found: &mut BTreeSet<String>) {
        let bytes = source.as_bytes();
        let mut index = 0;
        while let Some(offset) = source[index..].find("request") {
            let start = index + offset;
            index = start + "request".len();
            let preceded_by_ident = start > 0 && {
                let previous = bytes[start - 1];
                previous.is_ascii_alphanumeric() || previous == b'_' || previous == b'.'
            };
            if preceded_by_ident {
                continue;
            }
            let mut cursor = index;
            if bytes.get(cursor) == Some(&b'<') {
                let mut depth = 0i32;
                while cursor < bytes.len() {
                    match bytes[cursor] {
                        b'<' => depth += 1,
                        b'>' if cursor > 0 && bytes[cursor - 1] == b'=' => {}
                        b'>' => {
                            depth -= 1;
                            if depth == 0 {
                                cursor += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    cursor += 1;
                }
            }
            if bytes.get(cursor) != Some(&b'(') {
                continue;
            }
            cursor += 1;
            while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_whitespace()) {
                cursor += 1;
            }
            let Some(&quote) = bytes.get(cursor) else { continue };
            if quote != b'\'' && quote != b'`' && quote != b'"' {
                continue;
            }
            let rest = &source[cursor + 1..];
            let Some(end) = rest.find(quote as char) else { continue };
            let name = &rest[..end];
            if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_') {
                found.insert(name.to_owned());
            }
        }
    }

    fn worker_dispatch() -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let pattern = regex::Regex::new(r#"operation == "([a-z0-9_]+)""#).unwrap();
        let group = regex::Regex::new(r#"operation in [\{\(]([^}\)]*)[\}\)]"#).unwrap();
        let literal = regex::Regex::new(r#""([a-z0-9_]+)""#).unwrap();
        for name in ["gui_service.py", "gui_cloud_ops.py"] {
            let source = read(&project_root().join("nivora_launcher").join(name));
            for capture in pattern.captures_iter(&source) {
                found.insert(capture[1].to_owned());
            }
            for capture in group.captures_iter(&source) {
                for item in literal.captures_iter(&capture[1]) {
                    found.insert(item[1].to_owned());
                }
            }
        }
        found
    }

    fn native_dispatch() -> BTreeSet<String> {
        let source = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rust_engine/mod.rs"));
        let body = &source[source.find("pub fn handle(").expect("handle() в mod.rs")..];
        let arm = regex::Regex::new(r#"(?m)^\s*"([a-z0-9_]+)" =>"#).unwrap();
        arm.captures_iter(body).map(|capture| capture[1].to_owned()).collect()
    }

    #[test]
    fn registry_lists_do_not_overlap_or_repeat() {
        let mut seen = BTreeSet::new();
        for operation in NATIVE_OPS.iter().chain(WORKER_OPS) {
            assert!(seen.insert(*operation), "операция {operation} указана в реестре дважды");
        }
    }

    #[test]
    fn every_frontend_operation_is_allowed_by_the_bridge() {
        let frontend = frontend_operations();
        assert!(frontend.len() > 150, "разбор bridge.ts нашёл слишком мало операций: {}", frontend.len());
        let missing: Vec<_> = frontend.iter().filter(|operation| !is_allowed(operation)).collect();
        assert!(missing.is_empty(), "фронтенд вызывает операции вне реестра: {missing:?}");
    }

    #[test]
    fn every_native_operation_has_an_engine_handler() {
        let handled = native_dispatch();
        let missing: Vec<_> = NATIVE_OPS.iter().filter(|operation| !handled.contains(**operation)).collect();
        assert!(missing.is_empty(), "NATIVE_OPS без ветки в handle(): {missing:?}");
    }

    #[test]
    fn every_worker_operation_has_a_python_handler() {
        let handled = worker_dispatch();
        assert!(handled.len() > 100, "разбор gui_service.py выглядит неверным");
        let missing: Vec<_> = WORKER_OPS.iter().filter(|operation| !handled.contains(**operation)).collect();
        assert!(missing.is_empty(), "WORKER_OPS без обработчика в gui_service.py: {missing:?}");
    }

    #[test]
    fn conditional_worker_fallbacks_exist_in_python() {
        // needs_worker() отдаёт эти нативные операции воркеру по условию —
        // у воркера обязан быть обработчик, иначе условие превращается в краш.
        let handled = worker_dispatch();
        for operation in [
            "security_status", "instance_file_list", "instance_file_read", "instance_file_write",
            "instance_file_mkdir", "instance_file_upload", "instance_file_delete", "delete_instance",
            "world_backup_create", "world_backup_restore", "world_backup_delete", "world_backup_list",
            "create_offline_account", "delete_account", "set_default_account", "move_account",
            "mods_search", "mods_details", "mods_check_updates",
        ] {
            assert!(NATIVE_OPS.contains(&operation), "{operation}");
            assert!(handled.contains(operation), "у воркера нет фолбэка для {operation}");
        }
    }

    #[test]
    fn unknown_and_malformed_requests_are_rejected_with_stable_messages() {
        assert_eq!(
            validate(&json!({"operation": "lan_session_create"})).unwrap_err(),
            "Операция GUI не разрешена desktop bridge: lan_session_create"
        );
        assert_eq!(validate(&json!({})).unwrap_err(), "GUI-запрос не содержит operation");
        assert_eq!(validate(&json!({"operation": 5})).unwrap_err(), "GUI-запрос не содержит operation");
    }

    #[test]
    fn launch_operations_use_the_transient_worker_only() {
        assert!(uses_transient_worker(&json!({"operation": "launch"})));
        assert!(uses_transient_worker(&json!({"operation": "launch_instance"})));
        assert!(!uses_transient_worker(&json!({"operation": "server_start"})));
        assert_eq!(route(&json!({"operation": "launch"})).unwrap(), Route::TransientWorker);
        assert_eq!(route(&json!({"operation": "engine_info"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "mods_install"})).unwrap(), Route::Worker);
    }

    #[test]
    fn routing_is_stable_under_parallel_repeated_calls() {
        // 32 потока × 25 повторов по всему реестру: маршрут детерминирован и
        // ни одна разрешённая операция не отбивается мостом.
        let operations: Vec<&str> = NATIVE_OPS.iter().chain(WORKER_OPS).copied().collect();
        std::thread::scope(|scope| {
            for _ in 0..32 {
                scope.spawn(|| {
                    for _ in 0..25 {
                        for operation in &operations {
                            let request = json!({"operation": operation});
                            assert!(route(&request).is_ok(), "{operation}");
                        }
                    }
                });
            }
        });
    }
}
