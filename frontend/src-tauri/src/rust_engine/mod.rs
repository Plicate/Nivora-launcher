// Движок операций GUI. Запрос — плоский JSON {operation, ...поля},
// ответ — конверт {"ok": true, "operation", "data"} либо {"ok": false,
// "error"}. Список обслуживаемых операций — NATIVE_OPS; всё остальное
// маршрутизируется наружу (worker-бэкенд).
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

mod accounts;
mod backups;
pub mod bedrock;
mod config;
mod constants;
mod crash_reports;
mod installer;
mod instances;
mod loaders;
mod metadata;
mod metrics;
mod mod_dependencies;
mod modpack_export;
mod mods;
mod net;
mod operations;
mod ops;
mod plugin_catalogs;
mod plugin_security;
mod plugins;
mod profiles;
mod runtime;
mod server;
mod snapshots;
mod platform;
mod platform_support;
mod process_guard;
pub mod state;
mod system;
mod themes;

pub const NATIVE_OPS: &[&str] = &[
    "hash_file",
    "hash_directory",
    // Minecraft Bedrock целиком в Rust: Linux — BedrockOnLinux, Windows —
    // официальный Minecraft for Windows из Microsoft Store.
    "bedrock_runtime_status",
    "bedrock_versions",
    "bedrock_setup_version",
    "bedrock_setup_status",
    "bedrock_setup_cancel",
    "bedrock_store_login",
    "bedrock_store_login_status",
    "bedrock_store_login_cancel",
    "bedrock_account_status",
    "bedrock_screenshots_list",
    "bedrock_create_instance",
    "bedrock_content_list",
    "bedrock_content_import",
    "bedrock_content_remove",
    "bedrock_stop",
    "engine_info",
    "system_metrics",
    "overlay_state",
    "jvm_presets_list",
    "launch_history",
    "cache_stats",
    "cache_prune",
    "theme_list",
    "theme_preview",
    "theme_save",
    "theme_import",
    "theme_delete",
    "theme_export",
    "settings_profiles_list",
    "settings_profile_delete",
    "settings_profile_export",
    "settings_profile_import",
    "java_options",
    "security_status",
    "server_cores",
    "world_backup_list",
    "world_backup_create",
    "world_backup_restore",
    "world_backup_delete",
    "world_backup_schedule_get",
    "world_backup_schedule_set",
    "create_offline_account",
    "delete_account",
    "set_default_account",
    "move_account",
    "login_microsoft",
    "microsoft_login_status",
    "install",
    "server_start",
    "server_stop",
    "server_restart",
    "server_command",
    "server_log",
    "server_status",
    "mod_dependency_graph",
    "mod_removal_impact",
    // Поиск источников импорта — только чтение пары каталогов. Через общий
    // Python-воркер он вставал в очередь за долгими операциями, и кнопка
    // «Импорт» бесконечно показывала «Ищу источники…».
    "external_instances_scan",
];

pub fn is_native(operation: &str) -> bool {
    NATIVE_OPS.contains(&operation)
}

/// Облачные резервные копии хранятся в WebDAV — их обслуживает
/// Python-воркер. Локальные копии и расписания идут нативно даже при
/// настроенном облаке, кроме операций, чей ответ включает облачный список.
pub fn needs_worker(operation: &str, request: &Value) -> bool {
    match operation {
        // На Windows песочница — AppContainer; её проверку (пробный запуск
        // процесса в контейнере) выполняет Python-бэкенд.
        "security_status" => return cfg!(windows),
        "world_backup_create" | "world_backup_restore" | "world_backup_delete" | "world_backup_list" => {}
        // Legacy-токены Microsoft в файле переносимы в Secret Service только
        // через Python; чистые файлы обслуживаются нативно.
        "create_offline_account" | "delete_account" | "set_default_account" | "move_account" => {
            return accounts::has_legacy_tokens(&state::launcher_roots().config_home);
        }
        _ => return false,
    }
    if operation == "world_backup_create" && request.get("destination").and_then(Value::as_str) == Some("cloud") {
        return true;
    }
    if matches!(operation, "world_backup_restore" | "world_backup_delete")
        && request
            .get("backup_id")
            .and_then(Value::as_str)
            .map(|value| value.ends_with("@cloud"))
            .unwrap_or(false)
    {
        return true;
    }
    let roots = state::launcher_roots();
    backups::cloud_configured(&roots.config_home)
}pub fn handle(operation: &str, request: &Value) -> Result<Value, String> {
    match operation {
        "hash_file" => hash_file(request),
        "hash_directory" => hash_directory(request),
        "system_metrics" => Ok(ok_response(operation, metrics::system_metrics(request)?)),
        "overlay_state" => Ok(ok_response(operation, state::overlay_state(request)?)),
        "jvm_presets_list" => Ok(ok_response(operation, ops::jvm_presets_list(request)?)),
        "launch_history" => Ok(ok_response(operation, ops::launch_history(request)?)),
        "cache_stats" => Ok(ok_response(operation, ops::cache_stats(request)?)),
        "cache_prune" => Ok(ok_response(operation, ops::cache_prune(request)?)),
        "theme_list" => Ok(ok_response(operation, themes::theme_list(request)?)),
        "theme_preview" => Ok(ok_response(operation, themes::theme_preview(request)?)),
        "theme_save" => Ok(ok_response(operation, themes::theme_save(request)?)),
        "theme_import" => Ok(ok_response(operation, themes::theme_import(request)?)),
        "theme_delete" => Ok(ok_response(operation, themes::theme_delete(request)?)),
        "theme_export" => Ok(ok_response(operation, themes::theme_export(request)?)),
        "settings_profiles_list" => Ok(ok_response(operation, profiles::profiles_list(request)?)),
        "settings_profile_delete" => Ok(ok_response(operation, profiles::profile_delete(request)?)),
        "settings_profile_export" => Ok(ok_response(operation, profiles::profile_export(request)?)),
        "settings_profile_import" => Ok(ok_response(operation, profiles::profile_import(request)?)),
        "java_options" => Ok(ok_response(operation, system::java_options(request)?)),
        "security_status" => Ok(ok_response(operation, system::security_status(request)?)),
        "server_cores" => Ok(ok_response(operation, system::server_cores(request)?)),
        "world_backup_list" => Ok(ok_response(operation, backups::world_backup_list(request)?)),
        "world_backup_create" => Ok(ok_response(operation, backups::world_backup_create(request)?)),
        "world_backup_restore" => Ok(ok_response(operation, backups::world_backup_restore(request)?)),
        "world_backup_delete" => Ok(ok_response(operation, backups::world_backup_delete(request)?)),
        "world_backup_schedule_get" => Ok(ok_response(operation, backups::world_backup_schedule_get(request)?)),
        "world_backup_schedule_set" => Ok(ok_response(operation, backups::world_backup_schedule_set(request)?)),
        "create_offline_account" => Ok(ok_response(operation, accounts::create_offline_account(request)?)),
        "delete_account" => Ok(ok_response(operation, accounts::delete_account(request)?)),
        "set_default_account" => Ok(ok_response(operation, accounts::set_default_account(request)?)),
        "move_account" => Ok(ok_response(operation, accounts::move_account(request)?)),
        "microsoft_login_status" => Ok(ok_response(operation, accounts::microsoft_login_status(request)?)),
        "login_microsoft" => Ok(ok_response(operation, accounts::login_microsoft(request)?)),
        "install" => Ok(ok_response(operation, installer::install(request)?)),
        "server_start" => Ok(ok_response(operation, server::server_start(request)?)),
        "server_stop" => Ok(ok_response(operation, server::server_stop(request)?)),
        "server_restart" => Ok(ok_response(operation, server::server_restart(request)?)),
        "server_command" => Ok(ok_response(operation, server::server_command(request)?)),
        "server_log" => Ok(ok_response(operation, server::server_log(request)?)),
        "server_status" => Ok(ok_response(operation, server::server_status(request)?)),
        "mod_dependency_graph" => Ok(ok_response(operation, mod_dependencies::mod_dependency_graph(request)?)),
        "mod_removal_impact" => Ok(ok_response(operation, mod_dependencies::mod_removal_impact(request)?)),
        "external_instances_scan" => Ok(ok_response(operation, instances::external_instances_scan(request)?)),
        "bedrock_runtime_status" => Ok(ok_response(operation, bedrock::runtime_status(request)?)),
        "bedrock_versions" => Ok(ok_response(operation, bedrock::versions(request)?)),
        "bedrock_setup_version" => Ok(ok_response(operation, bedrock::setup_version(request)?)),
        "bedrock_setup_status" => Ok(ok_response(operation, bedrock::setup_status(request)?)),
        "bedrock_setup_cancel" => Ok(ok_response(operation, bedrock::setup_cancel(request)?)),
        "bedrock_store_login" => Ok(ok_response(operation, bedrock::store_login(request)?)),
        "bedrock_store_login_status" => Ok(ok_response(operation, bedrock::store_login_status(request)?)),
        "bedrock_store_login_cancel" => Ok(ok_response(operation, bedrock::store_login_cancel(request)?)),
        "bedrock_account_status" => Ok(ok_response(operation, bedrock::account_status(request)?)),
        "bedrock_screenshots_list" => Ok(ok_response(operation, bedrock::screenshots_list(request)?)),
        "bedrock_create_instance" => Ok(ok_response(operation, bedrock::create_instance(request)?)),
        "bedrock_content_list" => Ok(ok_response(operation, bedrock::content_list(request)?)),
        "bedrock_content_import" => Ok(ok_response(operation, bedrock::content_import(request)?)),
        "bedrock_content_remove" => Ok(ok_response(operation, bedrock::content_remove(request)?)),
        "bedrock_stop" => Ok(ok_response(operation, bedrock::stop(request)?)),
        "engine_info" => Ok(ok_response(operation, engine_info())),
        other => Err(format!("rust-движок не обслуживает операцию {other}")),
    }
}

fn ok_response(operation: &str, data: Value) -> Value {
    // Конверт протокола; фронтенд ждёт данные в envelope.data.
    json!({ "ok": true, "operation": operation, "data": data })
}

fn request_field<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    request
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Поле «{name}» обязательно для этой операции"))
}

fn sha256_hex_of_file(path: &Path) -> Result<(u64, String), String> {
    let file = File::open(path).map_err(|error| format!("Не удалось открыть {}: {error}", path.display()))?;
    let size = file
        .metadata()
        .map_err(|error| format!("Не удалось прочитать свойства {}: {error}", path.display()))?
        .len();
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn hash_file(request: &Value) -> Result<Value, String> {
    let path = request_field(request, "path")?;
    let path = Path::new(path);
    if !path.is_file() {
        return Err(format!("Файл не найден: {}", path.display()));
    }
    let (size, sha256) = sha256_hex_of_file(path)?;
    Ok(ok_response(
        "hash_file",
        json!({ "path": path.to_string_lossy(), "size": size, "sha256": sha256 }),
    ))
}

fn hash_directory(request: &Value) -> Result<Value, String> {
    let root = PathBuf::from(request_field(request, "path")?);
    if !root.is_dir() {
        return Err(format!("Каталог не найден: {}", root.display()));
    }
    let mut files: Vec<Value> = Vec::new();
    let mut total_size: u64 = 0;
    // Обход вглубь от корня: пути собираются ОТ корня, поэтому «выход»
    // наружу невозможен по построению. Символические ссылки пропускаем —
    // это и безопаснее, и честнее для инвентаризации модпака.
    fn walk(dir: &Path, prefix: &str, files: &mut Vec<Value>, total_size: &mut u64) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|error| format!("Не удалось прочитать каталог {}: {error}", dir.display()))?
            .filter_map(|entry| entry.ok())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let relative = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let path = entry.path();
            if file_type.is_dir() {
                walk(&path, &relative, files, total_size)?;
            } else if file_type.is_file() {
                if let Ok((size, sha256)) = sha256_hex_of_file(&path) {
                    *total_size += size;
                    files.push(json!({ "path": relative, "size": size, "sha256": sha256 }));
                }
            }
        }
        Ok(())
    }
    walk(&root, "", &mut files, &mut total_size)?;
    Ok(ok_response(
        "hash_directory",
        json!({
            "path": root.to_string_lossy(),
            "count": files.len(),
            "total_size": total_size,
            "files": files,
        }),
    ))
}

fn engine_info() -> Value {
    json!({
        "engine": "rust",
        "crate_version": env!("CARGO_PKG_VERSION"),
        "native_ops": NATIVE_OPS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-rust-engine-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn native_ops_are_listed_and_routable() {
        for operation in NATIVE_OPS {
            assert!(is_native(operation), "{operation}");
        }
        assert!(!is_native("bootstrap"));
        let info = handle("engine_info", &json!({})).expect("engine_info");
        assert_eq!(info["ok"], json!(true));
        assert_eq!(info["data"]["engine"], json!("rust"));
        assert_eq!(info["data"]["native_ops"].as_array().map(Vec::len), Some(NATIVE_OPS.len()));
    }

    #[test]
    fn hash_file_matches_reference_vector() {
        let root = temp_root("file");
        let file = root.join("hello.txt");
        fs::write(&file, b"hello\n").expect("записать файл");
        let response = handle("hash_file", &json!({ "path": file.to_string_lossy() })).expect("hash_file");
        assert_eq!(response["ok"], json!(true));
        assert_eq!(response["data"]["size"], json!(6));
        // Эталон — вывод sha256sum для этого файла.
        assert_eq!(response["data"]["sha256"], json!("5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"));
        let missing = handle("hash_file", &json!({ "path": root.join("nope").to_string_lossy() }));
        assert!(missing.is_err());
    }

    #[test]
    #[cfg(unix)] // создание symlink без DevMode на Windows недоступно
    fn hash_directory_walks_sorted_and_skips_symlinks() {
        let root = temp_root("dir");
        fs::write(root.join("a.txt"), b"aaa").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("b.txt"), b"bbb").unwrap();
        symlink(root.join("a.txt"), root.join("link.txt")).expect("создать ссылку");
        let response = handle("hash_directory", &json!({ "path": root.to_string_lossy() })).expect("hash_directory");
        assert_eq!(response["ok"], json!(true));
        assert_eq!(response["data"]["count"], json!(2));
        assert_eq!(response["data"]["total_size"], json!(6));
        let paths: Vec<&str> = response["data"]["files"].as_array().unwrap().iter().map(|item| item["path"].as_str().unwrap()).collect();
        // Сортировка и прямые слеши в относительных путях — контракт
        // манифеста, на который ориентируется сравнение с манифестом модпака.
        assert_eq!(paths, vec!["a.txt", "sub/b.txt"]);
        let missing = handle("hash_directory", &json!({ "path": root.join("nope").to_string_lossy() }));
        assert!(missing.is_err());
    }

    #[test]
    fn required_fields_are_reported_in_russian() {
        let error = handle("hash_file", &json!({})).expect_err("path обязателен");
        assert!(error.contains("path"), "текст ошибки: {error}");
    }

    /// Сверка тем/профилей/справочников с эталонной реализацией: скрипт
    /// scripts/parity_ops.sh подставляет XDG-корни и входные данные через
    /// переменные окружения. Без них тест пропускается.
    #[test]
    fn live_parity_domain_fixture() {
        let root = std::env::var("NIVORA_PARITY_DOMAIN_ROOT").ok();
        let (Some(root), Ok(output)) = (root, std::env::var("NIVORA_PARITY_DOMAIN_OUT")) else {
            return;
        };
        let root = PathBuf::from(root);
        let theme_request = |name: &str| {
            let path = root.join(format!("{name}.json"));
            let raw = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)
        };
        let mut report = serde_json::Map::new();
        let save = themes::theme_save(&theme_request("theme-save"));
        report.insert("theme-save".into(), outcome(&save));
        let export = themes::theme_export(&theme_request("theme-export"));
        if let Ok(Value::Object(fields)) = &export {
            let mut normalized = fields.clone();
            if let Some(Value::String(encoded)) = normalized.remove("data") {
                let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap_or_default();
                let archive = {
                    // Содержимое архива важнее байтов deflate-потока.
                    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&decoded)).unwrap();
                    let mut parts = serde_json::Map::new();
                    for index in 0..archive.len() {
                        let mut member = archive.by_index(index).unwrap();
                        let name = member.name().to_owned();
                        let mut buffer = String::new();
                        member.by_ref().read_to_string(&mut buffer).unwrap();
                        parts.insert(name, json!(buffer));
                    }
                    Value::Object(parts)
                };
                normalized.insert("zip".into(), archive);
            }
            report.insert("theme-export".into(), Value::Object(normalized));
        } else {
            report.insert("theme-export".into(), outcome(&export));
        }
        report.insert("theme-list".into(), outcome(&themes::theme_list(&json!({}))));
        report.insert("theme-import".into(), outcome(&themes::theme_import(&theme_request("theme-import"))));
        report.insert("profile-import".into(), outcome(&profiles::profile_import(&theme_request("profile-import"))));
        report.insert("profiles-list".into(), outcome(&profiles::profiles_list(&json!({}))));
        report.insert("profile-export".into(), outcome(&profiles::profile_export(&theme_request("profile-export"))));
        report.insert("server-cores".into(), outcome(&system::server_cores(&json!({}))));
        report.insert("java-options".into(), outcome(&system::java_options(&json!({}))));
        report.insert("security-status".into(), outcome(&system::security_status(&json!({}))));
        fs::write(&output, serde_json::to_string_pretty(&Value::Object(report)).unwrap()).unwrap();
    }

    /// Живой паритет доменов бэкапов и аккаунтов: фикстура строится в
    /// XDG-корнях скриптом parity_ops.sh, обе стороны прогоняют одинаковые
    /// последовательности. Метки времени и размеры архивов гасятся.
    #[test]
    fn live_parity_backup_fixture() {
        let Ok(output) = std::env::var("NIVORA_PARITY_STATE_OUT") else {
            return;
        };
        let roots = state::launcher_roots();
        let game_dir = state::settings_game_dir(&roots.config_home, &roots.data_home);
        let instance = game_dir.join("instances").join("Alpha");
        let mut report = serde_json::Map::new();

        let normalize_backup = |value: Value| -> Value { normalize_backup_value(value) };
        let created = backups::world_backup_create(&json!({ "name": "Alpha", "destination": "local", "reason": "manual" }));
        let backup_id = created
            .as_ref()
            .ok()
            .and_then(|value| value["backup"]["backup_id"].as_str())
            .unwrap_or_default()
            .to_owned();
        report.insert("backup-create".into(), outcome(&created.map(normalize_backup)));
        report.insert("backup-list".into(), outcome(&backups::world_backup_list(&json!({ "name": "Alpha" })).map(normalize_backup)));
        report.insert(
            "schedule-set".into(),
            outcome(&backups::world_backup_schedule_set(&json!({ "name": "Alpha", "values": { "interval_hours": 48, "keep": 2, "destination": "local" }, "override": true })).map(normalize_backup)),
        );
        report.insert("schedule-get".into(), outcome(&backups::world_backup_schedule_get(&json!({ "name": "Alpha" })).map(normalize_backup)));

        // Портим мир и восстанавливаем: содержимое обязано вернуться.
        fs::write(instance.join("saves/New World/level.dat"), "сломано").unwrap();
        report.insert("backup-restore".into(), outcome(&backups::world_backup_restore(&json!({ "name": "Alpha", "backup_id": backup_id })).map(normalize_backup)));
        let restored = fs::read_to_string(instance.join("saves/New World/level.dat")).unwrap_or_default();
        report.insert("restored-content".into(), json!(restored));
        report.insert("backup-delete".into(), outcome(&backups::world_backup_delete(&json!({ "name": "Alpha", "backup_id": backup_id })).map(normalize_backup)));

        // Аккаунты: файл, порядок и default_account_id обязаны совпадать.
        report.insert("account-create".into(), outcome(&accounts::create_offline_account(&json!({ "username": "  Игрок_77!  " }))));
        let accounts_file = roots.config_home.join("accounts.json");
        let mut records: Vec<Value> = serde_json::from_str(&fs::read_to_string(&accounts_file).unwrap_or_default().to_owned()).unwrap_or_default();
        records.push(json!({ "account_id": "beta", "kind": "offline", "username": "Beta", "alias": "Бета", "expires_at": 0, "xuid": "", "used_seconds": 0, "limit_seconds": 0, "is_default": false }));
        let pretty = {
            let mut text = serde_json::to_string_pretty(&Value::Array(records)).unwrap();
            text.push('\n');
            text
        };
        fs::write(&accounts_file, pretty).unwrap();
        report.insert(
            "account-default".into(),
            outcome(&accounts::set_default_account(&json!({ "account_id": "beta" }))),
        );
        let settings_default = state::read_bounded_json(&roots.config_home.join("settings.json"), 1024 * 1024)
            .and_then(|value| value.get("default_account_id").cloned())
            .unwrap_or(Value::Null);
        report.insert("settings-default-account".into(), settings_default);
        report.insert("account-move".into(), outcome(&accounts::move_account(&json!({ "account_id": "beta", "direction": "up" }))));
        report.insert("account-delete".into(), outcome(&accounts::delete_account(&json!({ "account_id": "offline-debug" }))));
        report.insert("accounts-file".into(), json!(fs::read_to_string(&accounts_file).unwrap_or_default()));

        fs::create_dir_all(&roots.cache_home).unwrap();
        fs::write(roots.cache_home.join("microsoft-login.json"), json!({ "status": "device_code", "code": "XYZ 123" }).to_string()).unwrap();
        report.insert("login-status".into(), outcome(&accounts::microsoft_login_status(&json!({}))));

        fs::write(&output, serde_json::to_string_pretty(&Value::Object(report)).unwrap()).unwrap();
    }

    /// Метка времени в backup_id — первые 16 символов (YYYYmmdd-HHMMSS-);
    /// created_at и размеры гасятся: реализация deflate может отличаться.
    fn normalize_backup_value(value: Value) -> Value {
        let strip_id = |id: &str| id.chars().skip(16).collect::<String>();
        let fix_entry = |mut entry: Value| {
            if let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_owned) {
                entry["id"] = json!(strip_id(&id));
            }
            if let Some(object) = entry.as_object_mut() {
                if let Some(created) = object.get_mut("created_at") {
                    *created = json!(0);
                }
                if let Some(size) = object.get_mut("size_bytes") {
                    *size = json!(size.as_i64().unwrap_or(0) > 0);
                }
            }
            entry
        };
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            if let Some(backup) = object.get_mut("backup").and_then(Value::as_object_mut) {
                if let Some(id) = backup.get("backup_id").and_then(Value::as_str).map(str::to_owned) {
                    backup.insert("backup_id".into(), json!(strip_id(&id)));
                }
                if let Some(size) = backup.get_mut("size_bytes") {
                    *size = json!(size.as_i64().unwrap_or(0) > 0);
                }
                if let Some(backups) = backup.get_mut("backups").and_then(Value::as_array_mut) {
                    for entry in backups.iter_mut() {
                        *entry = fix_entry(entry.clone());
                    }
                }
            }
            if let Some(backups) = object.get_mut("backups").and_then(Value::as_array_mut) {
                for entry in backups.iter_mut() {
                    *entry = fix_entry(entry.clone());
                }
            }
            if let Some(schedule) = object.get_mut("schedule").and_then(Value::as_object_mut) {
                schedule.insert("last_run".into(), json!(0));
            }
        }
        value
    }

    fn outcome(result: &Result<Value, String>) -> Value {
        match result {
            Ok(value) => value.clone(),
            Err(error) => json!({ "__error__": error }),
        }
    }
}
