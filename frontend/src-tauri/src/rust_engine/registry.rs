//! реестр GUI-операций; [`route`] выбирает обработчик разрешённого запроса

use serde_json::Value;

use super::{bedrock, NATIVE_OPS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Native,
    BedrockLaunch,
}

pub fn is_allowed(operation: &str) -> bool {
    NATIVE_OPS.contains(&operation)
}

pub fn operation_of(request: &Value) -> Result<&str, String> {
    request
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| "GUI-запрос не содержит operation".to_owned())
}

/// проверка белого списка; формат ошибки сохранён: фронтенд и журналы
/// поддержки ищут именно эту строку
pub fn validate(request: &Value) -> Result<&str, String> {
    let operation = operation_of(request)?;
    if !is_allowed(operation) {
        return Err(format!("Операция GUI не разрешена desktop bridge: {operation}"));
    }
    Ok(operation)
}

pub fn route(request: &Value) -> Result<Route, String> {
    validate(request)?;
    if bedrock::should_intercept_launch(request) {
        return Ok(Route::BedrockLaunch);
    }
    Ok(Route::Native)
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

    /// счётчик скобок учитывает вложенные типы TypeScript
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

    fn native_dispatch() -> BTreeSet<String> {
        let source = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rust_engine/mod.rs"));
        let body = &source[source.find("pub fn handle(").expect("handle() в mod.rs")..];
        let arm = regex::Regex::new(r#"(?m)^\s*"([a-z0-9_]+)" =>"#).unwrap();
        arm.captures_iter(body).map(|capture| capture[1].to_owned()).collect()
    }

    #[test]
    fn registry_operations_are_unique() {
        let mut seen = BTreeSet::new();
        for operation in NATIVE_OPS.iter() {
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
    fn unknown_and_malformed_requests_are_rejected_with_stable_messages() {
        assert_eq!(
            validate(&json!({"operation": "lan_session_create"})).unwrap_err(),
            "Операция GUI не разрешена desktop bridge: lan_session_create"
        );
        assert_eq!(validate(&json!({})).unwrap_err(), "GUI-запрос не содержит operation");
        assert_eq!(validate(&json!({"operation": 5})).unwrap_err(), "GUI-запрос не содержит operation");
    }

    #[test]
    fn launch_operations_are_native_and_mods_ops_are_native() {
        assert_eq!(route(&json!({"operation": "launch", "name": ""})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "launch_instance", "name": ""})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "engine_info"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "mods_install"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "mods_update"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "mods_update_all_safe"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "modpack_import"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "mods_search", "project_type": "plugin"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "mods_search", "project_type": "modpack"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "world_backup_create", "destination": "cloud"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "world_import_begin", "label": "w.tar.bz2"})).unwrap(), Route::Native);
    }

    #[test]
    fn autotune_and_plugin_dev_tools_are_native() {
        assert_eq!(route(&json!({"operation": "instance_autotune", "name": "x"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "plugin_dev_init", "directory": "."})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "plugin_dev_lint", "directory": "."})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "plugin_dev_pack", "directory": ".", "output": "x.zip"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "plugin_dev_verify", "archive": "x.zip"})).unwrap(), Route::Native);
        assert_eq!(route(&json!({"operation": "plugin_dev_keypair", "private_pem": "a", "public_pem": "b"})).unwrap(), Route::Native);
    }

    #[test]
    fn routing_is_stable_under_parallel_repeated_calls() {
        let operations: Vec<&str> = NATIVE_OPS.iter().copied().collect();
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
