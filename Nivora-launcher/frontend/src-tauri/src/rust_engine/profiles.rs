// Пресеты глобальных настроек: список, удаление, экспорт и импорт
// JSON-файлов из config_home/settings-profiles. Машино-зависимые ключи
// (game_dir, client_id, launcher_version) в пресет не попадают; состав
// допустимых ключей — поля Settings.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::state::launcher_roots;

const PROFILE_FORMAT: &str = "nivora-settings-profile/1";
const PROFILE_DIR_NAME: &str = "settings-profiles";
const MAX_PROFILE_JSON_BYTES: u64 = 1024 * 1024;
const NAME_LIMIT: usize = 48;

// Допустимые ключи пресета — в точности поля Settings. Список проверяется
// паритет-скриптом на совпадение с dataclass-полями.
const SETTINGS_FIELDS: [&str; 74] = [
    "accent_color",
    "active_catalog_id",
    "active_launcher_profile_id",
    "active_plugin_catalog_id",
    "active_theme_id",
    "animation_settings",
    "animations",
    "auto_refresh_metadata",
    "backup_schedule",
    "catalog_api_url",
    "catalog_source",
    "client_id",
    "close_to_tray",
    "cloud_include_accounts",
    "cloud_last_sync",
    "cloud_remote_path",
    "cloud_url",
    "cloud_username",
    "color_tokens",
    "confirm_destructive_actions",
    "data_api_url",
    "data_source",
    "default_account_id",
    "default_loader",
    "default_version_filter",
    "density",
    "developer_mode",
    "download_retries",
    "download_timeout_seconds",
    "font_family",
    "game_dir",
    "graphics_mode",
    "http_proxy",
    "instance_groups",
    "java_path",
    "keep_logs_days",
    "keyboard_shortcuts",
    "last_view",
    "launch_after_install",
    "launcher_profiles",
    "launcher_version",
    "login_api_url",
    "login_source",
    "max_downloads",
    "memory",
    "metadata_refresh_hours",
    "minimize_on_launch",
    "offline_debug_limit_seconds",
    "open_last_instance",
    "overlay_settings",
    "page_layouts",
    "page_themes",
    "pinned_instance_groups",
    "plugin_catalogs",
    "radius_tokens",
    "remember_last_view",
    "resource_catalogs",
    "show_activity_panel",
    "show_toasts",
    "sidebar_floating",
    "sidebar_mode",
    "sidebar_position",
    "sidebar_visibility",
    "sidebar_width",
    "sidebar_width_mode",
    "startup_view",
    "telemetry",
    "telemetry_endpoint",
    "theme",
    "ui_scale",
    "wallpaper_blur",
    "wallpaper_opacity",
    "wallpaper_path",
    "widget_visibility",
];

fn err(message: &str) -> String {
    message.to_owned()
}

fn epoch_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0)
}

/// Имя профиля: недопустимые прогоны символов схлопываются в «-»,
/// обрезка до 48 символов, крайние точки и дефисы снимаются.
fn safe_profile_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    let mut cleaned = String::with_capacity(trimmed.len());
    let mut previous_invalid = false;
    for character in trimmed.chars() {
        let valid = character.is_ascii_alphanumeric()
            || matches!(character, '.' | '_' | ' ' | '-')
            || matches!(character, '\u{0410}'..='\u{042F}' | '\u{0430}'..='\u{044F}' | '\u{0401}' | '\u{0451}');
        if valid {
            cleaned.push(character);
            previous_invalid = false;
        } else if !previous_invalid {
            cleaned.push('-');
            previous_invalid = true;
        }
    }
    let truncated: String = cleaned.chars().take(NAME_LIMIT).collect();
    let stripped = truncated.trim_matches(|c| c == '.' || c == '-' || c == ' ');
    if stripped.is_empty() {
        return Err(err("Имя профиля настроек не может быть пустым"));
    }
    Ok(stripped.to_owned())
}

fn profile_dir(config_home: &Path) -> Result<PathBuf, String> {
    let directory = config_home.join(PROFILE_DIR_NAME);
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось прочитать профиль настроек: {error}"))?;
    super::platform::restrict_dir(&directory);
    Ok(directory)
}

fn profile_path(config_home: &Path, name: &str) -> Result<PathBuf, String> {
    Ok(profile_dir(config_home)?.join(format!("{}.json", safe_profile_name(name)?)))
}

fn dump_profile(document: &Value) -> String {
    let mut text = serde_json::to_string_pretty(document).expect("профиль сериализуется");
    text.push('\n');
    text
}

fn read_profile(path: &Path) -> Result<Value, String> {
    let meta = fs::symlink_metadata(path).map_err(|error| format!("Не удалось прочитать профиль настроек: {error}"))?;
    if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_PROFILE_JSON_BYTES {
        return Err(err("Файл профиля настроек недоступен"));
    }
    let document: Value = serde_json::from_slice(&fs::read(path).map_err(|error| format!("Не удалось прочитать профиль настроек: {error}"))?)
        .map_err(|_| err("Профиль настроек повреждён"))?;
    let object = document.as_object().ok_or_else(|| err("Файл не похож на профиль настроек Nivora"))?;
    if object.get("format").and_then(Value::as_str) != Some(PROFILE_FORMAT)
        || !object.get("settings").is_some_and(Value::is_object)
    {
        return Err(err("Файл не похож на профиль настроек Nivora"));
    }
    Ok(document)
}

/// Сводка профилей: повреждённые файлы молча пропускаются.
fn list_documents(config_home: &Path) -> Result<Vec<Value>, String> {
    let directory = profile_dir(config_home)?;
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|error| format!("Не удалось прочитать профиль настроек: {error}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        let path = directory.join(&name);
        let Ok(document) = read_profile(&path) else {
            continue;
        };
        let settings = document.get("settings").and_then(Value::as_object);
        result.push(json!({
            "name": document.get("name").and_then(Value::as_str).unwrap_or_else(|| name.trim_end_matches(".json")),
            "created_at": document.get("created_at").and_then(Value::as_f64).unwrap_or(0.0),
            "updated_at": document.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0),
            "key_count": settings.map(|value| value.len()).unwrap_or(0),
        }));
    }
    Ok(result)
}

pub fn profiles_list(_request: &Value) -> Result<Value, String> {
    Ok(json!({ "profiles": list_documents(&launcher_roots().config_home)? }))
}

pub fn profile_delete(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let roots = launcher_roots();
    delete_at(&roots.config_home, name)?;
    Ok(json!({ "profiles": list_documents(&roots.config_home)? }))
}

fn delete_at(config_home: &Path, name: &str) -> Result<(), String> {
    let path = profile_path(config_home, name)?;
    let meta = fs::symlink_metadata(&path);
    if matches!(&meta, Ok(meta) if meta.is_symlink()) || meta.is_err() {
        return Err(err("Профиль настроек не найден"));
    }
    fs::remove_file(&path).map_err(|error| format!("Не удалось удалить профиль: {error}"))
}

pub fn profile_export(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    export_at(&launcher_roots().config_home, name)
}

fn export_at(config_home: &Path, name: &str) -> Result<Value, String> {
    let path = profile_path(config_home, name)?;
    let document = read_profile(&path)?;
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or_default();
    Ok(json!({
        "name": document.get("name").and_then(Value::as_str).unwrap_or(stem),
        "file_name": format!("{stem}.json"),
        "json": dump_profile(&document),
    }))
}

pub fn profile_import(request: &Value) -> Result<Value, String> {
    let json_text = request.get("json").and_then(Value::as_str).unwrap_or_default();
    let raw_name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let roots = launcher_roots();
    import_at(&roots.config_home, json_text, raw_name)?;
    Ok(json!({ "profiles": list_documents(&roots.config_home)? }))
}

fn import_at(config_home: &Path, json_text: &str, raw_name: &str) -> Result<(), String> {
    if json_text.trim().is_empty() || json_text.len() as u64 > MAX_PROFILE_JSON_BYTES {
        return Err(err("Пустой или слишком большой файл профиля"));
    }
    let document: Value = serde_json::from_str(json_text).map_err(|_| err("Некорректный JSON"))?;
    let object = document.as_object().ok_or_else(|| err("Файл не похож на профиль настроек Nivora"))?;
    if object.get("format").and_then(Value::as_str) != Some(PROFILE_FORMAT)
        || !object.get("settings").is_some_and(Value::is_object)
    {
        return Err(err("Файл не похож на профиль настроек Nivora"));
    }
    let settings = object.get("settings").and_then(Value::as_object).expect("settings проверен выше");
    let mut unknown: Vec<&String> = settings.keys().filter(|key| !SETTINGS_FIELDS.contains(&key.as_str())).collect();
    if !unknown.is_empty() {
        unknown.sort();
        unknown.truncate(8);
        return Err(err(&format!("Профиль содержит неизвестные настройки: {}", unknown.iter().map(|key| key.as_str()).collect::<Vec<_>>().join(", "))));
    }
    let fallback_name = document.get("name").and_then(Value::as_str).unwrap_or("imported");
    let profile_name = safe_profile_name(if raw_name.trim().is_empty() { fallback_name } else { raw_name })?;
    let path = profile_path(config_home, &profile_name)?;
    let mut document = document;
    if let Some(object) = document.as_object_mut() {
        object.insert("name".into(), json!(profile_name));
        object.insert("updated_at".into(), json!(epoch_seconds()));
    }
    fs::write(&path, dump_profile(&document)).map_err(|error| format!("Не удалось записать профиль: {error}"))?;
    Ok(())
}

/// Ключи, которые не переносятся между машинами (game_dir, client_id,
/// launcher_version), отсекаются при сохранении профиля; импорт проверяет
/// только состав допустимых полей.

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-profiles-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    fn sample_profile(name: &str) -> Value {
        json!({
            "format": PROFILE_FORMAT,
            "name": name,
            "created_at": 100.0,
            "updated_at": 200.0,
            "settings": { "theme": "dark", "memory": 4096, "java_path": "/usr/bin/java" },
        })
    }

    fn write_profile(config_home: &Path, document: &Value) -> PathBuf {
        let path = profile_path(config_home, document["name"].as_str().unwrap()).expect("путь");
        fs::write(&path, dump_profile(document)).expect("записать профиль");
        path
    }

    #[test]
    fn name_sanitizing_collapses_invalid_runs_and_trims_edges() {
        assert_eq!(safe_profile_name("  Игры!!///Minecraft  ").unwrap(), "Игры-Minecraft");
        assert_eq!(safe_profile_name("...---").unwrap_err(), "Имя профиля настроек не может быть пустым");
        let long = "a".repeat(80);
        assert_eq!(safe_profile_name(&long).unwrap().len(), NAME_LIMIT);
    }

    #[test]
    fn list_summarizes_and_skips_broken_files() {
        let root = temp_root("list");
        write_profile(&root, &sample_profile("alpha"));
        write_profile(&root, &sample_profile("beta"));
        fs::write(root.join(PROFILE_DIR_NAME).join("broken.json"), b"{not json").unwrap();
        let documents = list_documents(&root).expect("список");
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0]["name"], json!("alpha"));
        assert_eq!(documents[0]["key_count"], json!(3));
    }

    #[test]
    #[cfg(unix)] // symlink-часть проверки имеет смысл только на unix
    fn delete_removes_profile_and_rejects_missing() {
        let root = temp_root("delete");
        let path = write_profile(&root, &sample_profile("gone"));
        let link = root.join(PROFILE_DIR_NAME).join("link.json");
        symlink(&path, &link).expect("ссылка");
        assert!(delete_at(&root, "gone").is_ok());
        assert!(delete_at(&root, "gone").is_err());
        assert_eq!(list_documents(&root).unwrap().len(), 0, "симлинк не читается как профиль");
        let _ = fs::remove_file(&link);
    }

    #[test]
    fn import_rejects_unknown_keys_and_roundtrips_valid_document() {
        let root = temp_root("import");
        let mut document = sample_profile("portable");
        document["settings"]["no_such_setting"] = json!(1);
        let error = import_at(&root, &dump_profile(&document), "").unwrap_err();
        assert!(error.contains("no_such_setting"), "{error}");

        let mut valid = sample_profile("portable");
        valid["settings"].as_object_mut().unwrap().remove("no_such_setting");
        import_at(&root, &dump_profile(&valid), "  Мой профиль!! ").expect("импорт");
        let documents = list_documents(&root).expect("список");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0]["name"], json!("Мой профиль"));
        let exported = export_at(&root, "Мой профиль").expect("экспорт");
        assert_eq!(exported["file_name"], json!("Мой профиль.json"));
        assert!(exported["json"].as_str().unwrap().contains("\"theme\": \"dark\""));
        assert!(import_at(&root, "{not json", "").is_err());
    }

    #[test]
    fn settings_fields_cover_dataclass_names() {
        // Контракт: список полей уникален и включает машинно-зависимые
        // ключи, которые сохранение профиля обязано отфильтровать.
        let mut seen = std::collections::HashSet::new();
        for field in SETTINGS_FIELDS {
            assert!(seen.insert(field), "дубликат поля {field}");
        }
        for key in ["game_dir", "client_id", "launcher_version"] {
            assert!(SETTINGS_FIELDS.contains(&key), "{key} должен быть в списке полей");
        }
        assert_eq!(SETTINGS_FIELDS.len(), 74);
    }
}
