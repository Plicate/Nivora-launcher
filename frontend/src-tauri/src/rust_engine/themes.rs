// локальное хранилище тем оформления
use base64::Engine;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::state::launcher_roots;

const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 128 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024;
const MAX_UNPACKED_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 128;
const MAX_RESOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_BUTTON_IMAGE_BYTES: usize = 512 * 1024;
const MAX_BUTTON_SOUND_BYTES: usize = 1024 * 1024;
const MAX_FUNCTION_BYTES: usize = 256 * 1024;
const MAX_FUNCTION_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_ASSET_FILES: usize = 64;
const MAX_ORIGIN_BYTES: u64 = 4096;
const THEMES_LIST_LIMIT: usize = 256;
const COLOR_FIELDS: [&str; 5] = ["accent", "accent_strong", "background", "panel", "panel_soft"];
const DOCUMENT_KEYS: [&str; 13] = [
    "schema_version",
    "id",
    "name",
    "author",
    "description",
    "mode",
    "accent",
    "accent_strong",
    "background",
    "panel",
    "panel_soft",
    "density",
    "sidebar_mode",
];
const OPTIONAL_DOCUMENT_KEYS: [&str; 4] = ["locale", "button_customization", "resources", "plugin"];
const BUTTON_TARGET_IDS: [&str; 4] = ["topbar-button", "card-button", "hero-play", "nav-item"];
const MAX_THEME_RESOURCES: usize = 64;
const LABEL_KEYS: [&str; 6] = ["instances", "accounts", "mods", "modpacks", "themes", "settings"];

fn theme_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*\z").expect("regex id"))
}

fn theme_version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}\z").expect("regex version"))
}

fn theme_color_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^#[0-9a-fA-F]{6}\z").expect("regex color"))
}

fn err(message: &str) -> String {
    message.to_owned()
}

fn reject_unknown_fields(object: &Map<String, Value>, allowed: &[&str], field: &str) -> Result<(), String> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(err(&format!("Недопустимое поле {field}")));
    }
    Ok(())
}

fn bounded_number(value: &Value, field: &str, min: f64, max: f64) -> Result<(), String> {
    let number = value.as_f64().ok_or_else(|| err(&format!("Некорректное числовое поле темы: {field}")))?;
    if !number.is_finite() || number < min || number > max {
        return Err(err(&format!("Числовое поле темы {field} вне допустимого диапазона")));
    }
    Ok(())
}

fn safe_relative_path(path: &str, prefix: &str) -> bool {
    if path.is_empty() || path.len() > 180 || path.contains('\\') || path.starts_with('/') || path.contains(':') {
        return false;
    }
    let Some(rest) = path.strip_prefix(prefix) else { return false };
    !rest.is_empty()
        && rest.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.len() <= 80
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn valid_button_target(target: &str) -> bool {
    if BUTTON_TARGET_IDS.contains(&target) {
        return true;
    }
    let Some((role, identity)) = target.split_once(':') else { return false };
    if !BUTTON_TARGET_IDS.contains(&role) || identity.is_empty() || identity.len() > 160 {
        return false;
    }
    let bytes = identity.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'-' => index += 1,
            b'%' if index + 2 < bytes.len() && bytes[index + 1].is_ascii_hexdigit() && bytes[index + 2].is_ascii_hexdigit() => index += 3,
            _ => return false,
        }
    }
    true
}

fn valid_resource_key(key: &str) -> bool {
    if ["texture", "icon", "sound"].contains(&key) {
        return true;
    }
    ["texture", "icon", "sound"].iter().any(|kind| {
        key.strip_suffix(&format!("-{kind}"))
            .is_some_and(valid_button_target)
    })
}

fn validate_button_customization(value: &Value) -> Result<Value, String> {
    let object = value.as_object().ok_or_else(|| err("button_customization должен быть объектом"))?;
    reject_unknown_fields(
        object,
        &["width", "height", "fill", "border", "font_family", "font_size", "outline", "texture_fit", "crop_enabled", "crop", "labels", "overrides"],
        "button_customization",
    )?;
    for (field, min, max) in [("width", 120.0, 280.0), ("height", 28.0, 64.0), ("font_size", 9.0, 20.0)] {
        if let Some(value) = object.get(field) { bounded_number(value, field, min, max)?; }
    }
    for field in ["fill", "border", "crop_enabled"] {
        if object.get(field).is_some_and(|value| !value.is_boolean()) {
            return Err(err(&format!("Поле button_customization.{field} должно быть boolean")));
        }
    }
    if let Some(value) = object.get("font_family") {
        let family = value.as_str().unwrap_or_default();
        if !["system", "inter", "manrope", "mono"].contains(&family) {
            return Err(err("Некорректное семейство шрифта кнопки"));
        }
    }
    if let Some(value) = object.get("texture_fit") {
        if !["cover", "contain"].contains(&value.as_str().unwrap_or_default()) {
            return Err(err("Некорректное заполнение текстуры кнопки"));
        }
    }
    if let Some(value) = object.get("outline") {
        let outline = value.as_object().ok_or_else(|| err("outline должен быть объектом"))?;
        reject_unknown_fields(outline, &["enabled", "width", "color"], "button_customization.outline")?;
        if outline.get("enabled").is_some_and(|value| !value.is_boolean()) {
            return Err(err("outline.enabled должен быть boolean"));
        }
        if let Some(width) = outline.get("width") { bounded_number(width, "outline.width", 0.0, 3.0)?; }
        if let Some(color) = outline.get("color") {
            if !theme_color_re().is_match(color.as_str().unwrap_or_default()) {
                return Err(err("outline.color должен иметь формат #rrggbb"));
            }
        }
    }
    if let Some(value) = object.get("crop") {
        let crop = value.as_object().ok_or_else(|| err("crop должен быть объектом"))?;
        reject_unknown_fields(crop, &["x", "y", "width", "height"], "button_customization.crop")?;
        for (field, min, max) in [("x", 0.0, 100.0), ("y", 0.0, 100.0), ("width", 5.0, 100.0), ("height", 5.0, 100.0)] {
            let value = crop.get(field).ok_or_else(|| err(&format!("crop.{field} обязателен")))?;
            bounded_number(value, &format!("crop.{field}"), min, max)?;
        }
        if crop["x"].as_f64().unwrap() + crop["width"].as_f64().unwrap() > 100.0
            || crop["y"].as_f64().unwrap() + crop["height"].as_f64().unwrap() > 100.0
        {
            return Err(err("Область crop выходит за границы текстуры"));
        }
    }
    if let Some(value) = object.get("labels") {
        let labels = value.as_object().ok_or_else(|| err("labels должен быть объектом"))?;
        reject_unknown_fields(labels, &LABEL_KEYS, "button_customization.labels")?;
        for (key, label) in labels {
            if !label.is_string() || label.as_str().unwrap().chars().count() > 16 {
                return Err(err(&format!("Подпись кнопки {key} должна содержать не более 16 символов")));
            }
        }
    }
    if let Some(value) = object.get("overrides") {
        let overrides = value.as_object().ok_or_else(|| err("overrides должен быть объектом"))?;
        for (target, value) in overrides {
            if !valid_button_target(target) {
                return Err(err("Недопустимая цель button_customization.overrides"));
            }
            let override_fields = value.as_object().ok_or_else(|| err("Переопределение кнопки должно быть объектом"))?;
            reject_unknown_fields(override_fields, &["width", "height", "fill", "border", "font_family", "font_size", "outline", "texture_fit", "crop_enabled", "crop", "label"], "button_customization.overrides")?;
            if let Some(label) = override_fields.get("label") {
                if !label.is_string() || label.as_str().unwrap().chars().count() > 16 {
                    return Err(err("Подпись индивидуальной кнопки должна содержать не более 16 символов"));
                }
            }
            let fields: serde_json::Map<String, Value> = override_fields.iter()
                .filter(|(key, _)| key.as_str() != "label")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            validate_button_customization(&Value::Object(fields))?;
        }
    }
    Ok(value.clone())
}

fn validate_resource_map(value: &Value) -> Result<BTreeMap<String, String>, String> {
    let object = value.as_object().ok_or_else(|| err("resources должен быть объектом"))?;
    if object.len() > MAX_THEME_RESOURCES {
        return Err(err("Слишком много ресурсов темы"));
    }
    let mut result = BTreeMap::new();
    let mut paths = HashSet::new();
    for (key, value) in object {
        if !valid_resource_key(key) {
            return Err(err("Недопустимый ключ resources"));
        }
        let path = value.as_str().unwrap_or_default();
        if !safe_relative_path(path, "resources/") || !paths.insert(path.to_ascii_lowercase()) {
            return Err(err("Путь ресурса некорректен или повторяется"));
        }
        let extension = Path::new(path).extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
        let kind = resource_kind(key);
        let valid = match kind {
            Some("texture") => ["png", "jpg", "jpeg", "webp", "gif"].contains(&extension.as_str()),
            Some("icon") => ["png", "jpg", "jpeg", "webp"].contains(&extension.as_str()),
            Some("sound") => ["ogg", "mp3", "wav", "webm"].contains(&extension.as_str()),
            _ => false,
        };
        if !valid { return Err(err(&format!("Недопустимый тип файла ресурса {key}"))); }
        result.insert(key.clone(), path.to_owned());
    }
    Ok(result)
}

fn validate_plugin(value: &Value) -> Result<(Value, Vec<String>), String> {
    let object = value.as_object().ok_or_else(|| err("plugin должен быть декларативным объектом"))?;
    reject_unknown_fields(object, &["id", "name", "version", "api_version", "entrypoint", "sources"], "plugin")?;
    let id = text_field(object.get("id"), "plugin.id", 48)?;
    if !theme_id_re().is_match(&id) { return Err(err("plugin.id должен быть в формате kebab-case")); }
    let name = text_field(object.get("name"), "plugin.name", 80)?;
    let version = text_field(object.get("version"), "plugin.version", 64)?;
    if !theme_version_re().is_match(&version) { return Err(err("Некорректная версия plugin")); }
    if object.get("api_version").and_then(Value::as_str) != Some("3") {
        return Err(err("plugin.api_version должен быть строкой \"3\""));
    }
    let entrypoint = object.get("entrypoint").and_then(Value::as_str).unwrap_or_default();
    if !safe_relative_path(entrypoint, "functions/") || !["js", "mjs"].contains(&Path::new(entrypoint).extension().and_then(|v| v.to_str()).unwrap_or("")) {
        return Err(err("plugin.entrypoint должен указывать на исходник JavaScript в functions/"));
    }
    let raw_sources = object.get("sources").and_then(Value::as_array).ok_or_else(|| err("plugin.sources должен содержать список исходников"))?;
    if raw_sources.is_empty() || raw_sources.len() > 32 { return Err(err("plugin.sources должен содержать от 1 до 32 исходников")); }
    let mut sources = Vec::new();
    let mut unique = HashSet::new();
    for raw in raw_sources {
        let path = raw.as_str().unwrap_or_default();
        let extension = Path::new(path).extension().and_then(|v| v.to_str()).unwrap_or("");
        if !safe_relative_path(path, "functions/") || !["js", "mjs", "json", "txt"].contains(&extension) || !unique.insert(path.to_ascii_lowercase()) {
            return Err(err("Путь исходника plugin некорректен или повторяется"));
        }
        sources.push(path.to_owned());
    }
    if !unique.contains(&entrypoint.to_ascii_lowercase()) { return Err(err("plugin.entrypoint должен быть указан в plugin.sources")); }
    sources.sort();
    let normalized = json!({"id": id, "name": name, "version": version, "api_version": "3", "entrypoint": entrypoint, "sources": sources});
    Ok((normalized, sources))
}

fn resource_kind(key: &str) -> Option<&'static str> {
    if key == "texture" || key.ends_with("-texture") { Some("texture") }
    else if key == "icon" || key.ends_with("-icon") { Some("icon") }
    else if key == "sound" || key.ends_with("-sound") { Some("sound") }
    else { None }
}

fn validate_resource_bytes(key: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
    let kind = resource_kind(key).ok_or_else(|| err("Недопустимый ключ resources"))?;
    let limit = if kind == "sound" { MAX_BUTTON_SOUND_BYTES } else { MAX_BUTTON_IMAGE_BYTES };
    if bytes.is_empty() || bytes.len() > limit { return Err(err("Файл ресурса пустой или слишком большой")); }
    let extension = Path::new(path).extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    let valid = match (kind, extension.as_str()) {
        ("texture" | "icon", "png") => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        ("texture" | "icon", "jpg" | "jpeg") => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        ("texture" | "icon", "webp") => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        ("texture", "gif") => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        ("sound", "ogg") => bytes.starts_with(b"OggS"),
        ("sound", "wav") => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        ("sound", "mp3") => bytes.starts_with(b"ID3") || bytes.get(..2).is_some_and(|header| header[0] == 0xff && header[1] & 0xe0 == 0xe0),
        ("sound", "webm") => bytes.starts_with(b"\x1a\x45\xdf\xa3"),
        _ => false,
    };
    if !valid { return Err(err("Содержимое ресурса не соответствует ожидаемому типу файла")); }
    Ok(())
}

fn validate_asset_bundle(theme: &Value, resources: &BTreeMap<String, Vec<u8>>, functions: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let resource_paths = if let Some(value) = theme.get("resources") { validate_resource_map(value)? } else { BTreeMap::new() };
    if resources.len() != resource_paths.len() || resources.keys().any(|key| !resource_paths.contains_key(key)) {
        return Err(err("Отсутствуют ресурсы темы или присутствуют незаявленные данные"));
    }
    for (key, path) in &resource_paths {
        validate_resource_bytes(key, path, &resources[key])?;
    }
    let plugin_sources = if let Some(plugin) = theme.get("plugin") { validate_plugin(plugin)?.1 } else { Vec::new() };
    if functions.len() != plugin_sources.len() || functions.keys().any(|path| !plugin_sources.contains(path)) {
        return Err(err("Отсутствуют исходники plugin или присутствуют незаявленные файлы"));
    }
    let mut total = 0usize;
    for (path, bytes) in functions {
        if bytes.is_empty() || bytes.len() > MAX_FUNCTION_BYTES || std::str::from_utf8(bytes).is_err() {
            return Err(err("Исходник plugin пустой, слишком большой или не является UTF-8"));
        }
        total = total.saturating_add(bytes.len());
        if total > MAX_FUNCTION_TOTAL_BYTES { return Err(err("Исходники plugin превышают ограничение размера")); }
        if !safe_relative_path(path, "functions/") { return Err(err("Путь исходника plugin некорректен")); }
    }
    if resources.values().map(Vec::len).sum::<usize>().saturating_add(total) > MAX_UNPACKED_BYTES as usize {
        return Err(err("Ресурсы и исходники темы превышают общий лимит"));
    }
    Ok(())
}

fn text_field(value: Option<&Value>, field: &str, limit: usize) -> Result<String, String> {
    let raw = match value {
        Some(Value::String(text)) => text.trim(),
        _ => "",
    };
    if raw.is_empty() || raw.chars().count() > limit {
        return Err(err(&format!("Некорректное поле темы: {field}")));
    }
    Ok(raw.to_owned())
}

pub fn normalize_theme(value: &Value, metadata: &Value, source: &str, source_url: &str) -> Result<Value, String> {
    let theme_object = value.as_object().ok_or_else(|| err("ZIP темы должен содержать theme.json и theme.yml с объектами темы и описания"))?;
    let metadata_object = metadata.as_object().ok_or_else(|| err("ZIP темы должен содержать theme.json и theme.yml с объектами темы и описания"))?;
    let schema_version = match theme_object.get("schema_version") {
        None | Some(Value::Null) => 1,
        Some(Value::Number(number)) => number.as_i64().unwrap_or(-1),
        _ => -1,
    };
    if ![1, 2].contains(&schema_version) {
        return Err(err("Эта версия схемы темы не поддерживается"));
    }
    if schema_version == 2 {
        reject_unknown_fields(theme_object, &["schema_version", "id", "name", "author", "description", "mode", "accent", "accent_strong", "background", "panel", "panel_soft", "density", "sidebar_mode", "locale", "button_customization", "resources", "plugin", "featured", "resource_data", "version", "creators", "source", "source_url"], "theme.json")?;
    }

    let theme_id = text_field(theme_object.get("id"), "id", 48)?.to_lowercase();
    if !theme_id_re().is_match(&theme_id) {
        return Err(err("id темы должен быть в формате kebab-case"));
    }
    let name = text_field(theme_object.get("name"), "name", 80)?;
    let author = text_field(metadata_object.get("author"), "author", 120)?;
    let description = text_field(metadata_object.get("description"), "description", 500)?;
    let version = text_field(metadata_object.get("version"), "version", 64)?;
    if !theme_version_re().is_match(&version) {
        return Err(err("Версия темы содержит недопустимые символы"));
    }

    let raw_creators = metadata_object
        .get("creators")
        .and_then(Value::as_array)
        .ok_or_else(|| err("theme.yml должен содержать список creators от 1 до 16 авторов"))?;
    if raw_creators.is_empty() || raw_creators.len() > 16 {
        return Err(err("theme.yml должен содержать список creators от 1 до 16 авторов"));
    }
    let mut creators: Vec<String> = Vec::new();
    let mut seen_creators: HashSet<String> = HashSet::new();
    for item in raw_creators {
        let creator = text_field(Some(item), "creators", 120)?;
        if seen_creators.insert(creator.clone()) {
            creators.push(creator);
        }
    }

    let mut colors = Map::new();
    for field in COLOR_FIELDS {
        let raw = theme_object.get(field).and_then(Value::as_str).unwrap_or("").trim();
        if !theme_color_re().is_match(raw) {
            return Err(err(&format!("Цвет темы {field} должен иметь формат #rrggbb")));
        }
        colors.insert(field.to_owned(), json!(raw.to_lowercase()));
    }

    let enum_field = |container: &Map<String, Value>, key: &str, allowed: &[&str], message: &str| -> Result<String, String> {
        let raw = container.get(key).and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        if allowed.contains(&raw.as_str()) {
            Ok(raw)
        } else {
            Err(err(message))
        }
    };
    let mode = enum_field(theme_object, "mode", &["system", "light", "dark", "glass"], "Некорректный режим темы")?;
    let density = enum_field(theme_object, "density", &["compact", "comfortable", "spacious"], "Некорректная плотность темы")?;
    let sidebar_mode = enum_field(theme_object, "sidebar_mode", &["expanded", "adaptive", "compact"], "Некорректный режим боковой панели темы")?;
    if !["local", "github"].contains(&source) {
        return Err(err("Неизвестный источник темы"));
    }

    let mut result = Map::new();
    result.insert("schema_version".into(), json!(schema_version));
    result.insert("id".into(), json!(theme_id));
    result.insert("name".into(), json!(name));
    result.insert("author".into(), json!(author));
    result.insert("description".into(), json!(description));
    result.insert("version".into(), json!(version));
    result.insert("creators".into(), json!(creators));
    result.insert("source".into(), json!(source));
    result.insert("source_url".into(), json!(if source == "github" { source_url } else { "" }));
    result.insert("mode".into(), json!(mode));
    for field in COLOR_FIELDS {
        result.insert(field.to_owned(), colors.get(field).cloned().unwrap_or(Value::Null));
    }
    result.insert("density".into(), json!(density));
    result.insert("sidebar_mode".into(), json!(sidebar_mode));
    result.insert("featured".into(), json!(theme_object.get("featured") == Some(&Value::Bool(true))));
    if let Some(locale) = theme_object.get("locale") {
        let locale = locale.as_str().unwrap_or_default();
        if !["ru", "en"].contains(&locale) { return Err(err("locale темы должен быть ru или en")); }
        result.insert("locale".into(), json!(locale));
    }
    if let Some(buttons) = theme_object.get("button_customization") {
        result.insert("button_customization".into(), validate_button_customization(buttons)?);
    }
    if let Some(resources) = theme_object.get("resources") {
        let normalized = validate_resource_map(resources)?;
        let map: Map<String, Value> = normalized.into_iter().map(|(key, path)| (key, json!(path))).collect();
        result.insert("resources".into(), Value::Object(map));
    }
    if let Some(plugin) = theme_object.get("plugin") {
        result.insert("plugin".into(), validate_plugin(plugin)?.0);
    }
    Ok(Value::Object(result))
}

fn strip_yaml_comment(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut quote = '\0';
    let mut escaped = false;
    for index in 0..chars.len() {
        let character = chars[index];
        if escaped {
            escaped = false;
        } else if character == '\\' && quote == '"' {
            escaped = true;
        } else if character == '\'' || character == '"' {
            if quote == '\0' {
                quote = character;
            } else if quote == character {
                quote = '\0';
            }
        } else if character == '#' && quote == '\0' && (index == 0 || chars[index - 1].is_whitespace()) {
            return chars[..index].iter().collect();
        }
    }
    line.to_owned()
}

fn as_json_string(value: &Value) -> Option<&str> {
    value.as_str()
}

fn yaml_scalar(raw: &str) -> Result<Value, String> {
    let value = raw.trim();
    if value.is_empty() || value.starts_with(['!', '&', '*', '|', '>']) {
        return Err(err("theme.yml содержит неподдерживаемое YAML-значение"));
    }
    if let Some(rest) = value.strip_prefix('"') {
        let _ = rest;
        let parsed: Value = serde_json::from_str(value).map_err(|_| err("Некорректная строка в theme.yml"))?;
        return match as_json_string(&parsed) {
            Some(text) => Ok(json!(text)),
            None => Err(err("Значения метаданных theme.yml должны быть строками")),
        };
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        let inner = &value[1..value.len() - 1];
        return Ok(json!(inner.replace("''", "'")));
    }
    if value.starts_with('[') && value.ends_with(']') {
        let parsed: Value = serde_json::from_str(value).map_err(|_| err("Список creators в theme.yml должен быть JSON-подобным"))?;
        let items = parsed.as_array().ok_or_else(|| err("Список creators в theme.yml должен быть JSON-подобным"))?;
        if items.iter().any(|item| !item.is_string()) {
            return Err(err("Список creators в theme.yml должен содержать строки"));
        }
        return Ok(parsed);
    }
    Ok(json!(value))
}

pub fn parse_theme_yml(content: &[u8]) -> Result<Value, String> {
    if content.is_empty() || content.len() as u64 > MAX_METADATA_BYTES {
        return Err(err("theme.yml пустой или слишком большой"));
    }
    let text = std::str::from_utf8(content).map_err(|_| err("theme.yml должен быть в UTF-8"))?;
    let mut result = Map::new();
    let mut creators_list_open = false;
    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        if raw_line.contains('\t') {
            return Err(err(&format!("theme.yml содержит tab на строке {line_number}")));
        }
        let stripped = strip_yaml_comment(raw_line);
        let line = stripped.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("---") {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let value = trimmed;
        if indent == 0 {
            creators_list_open = false;
            let (key, raw_value) = value.split_once(':').ok_or_else(|| err(&format!("Некорректная строка theme.yml: {line_number}")))?;
            let key = key.trim();
            if !["author", "description", "version", "creators"].contains(&key) || result.contains_key(key) {
                return Err(err(&format!("Недопустимое или повторяющееся поле theme.yml: {key}")));
            }
            if key == "creators" && raw_value.trim().is_empty() {
                result.insert(key.to_owned(), json!([]));
                creators_list_open = true;
            } else {
                result.insert(key.to_owned(), yaml_scalar(raw_value)?);
            }
            continue;
        }
        if indent == 2 && creators_list_open {
            if let Some(item) = value.strip_prefix("- ") {
                let creators = result.get_mut("creators").and_then(Value::as_array_mut).expect("creators инициализирован выше");
                creators.push(yaml_scalar(item)?);
                continue;
            }
        }
        return Err(err(&format!("Некорректный отступ в theme.yml на строке {line_number}")));
    }
    if result.len() != 4 || !["author", "description", "version", "creators"].iter().all(|key| result.contains_key(*key)) {
        return Err(err("theme.yml должен содержать author, description, version и creators"));
    }
    if !result.get("creators").is_some_and(Value::is_array) {
        return Err(err("creators в theme.yml должен быть списком"));
    }
    Ok(Value::Object(result))
}

fn theme_document(theme: &Value) -> Value {
    let source = theme.as_object().expect("нормализованная тема — объект");
    let mut document = Map::new();
    for key in DOCUMENT_KEYS {
        if let Some(value) = source.get(key) {
            document.insert(key.to_owned(), value.clone());
        }
    }
    for key in OPTIONAL_DOCUMENT_KEYS {
        if let Some(value) = source.get(key) {
            document.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(document)
}

fn json_quote(text: &str) -> String {
    serde_json::to_string(text).expect("строка кодируется в JSON")
}

fn metadata_yaml(theme: &Value) -> String {
    let object = theme.as_object().expect("нормализованная тема — объект");
    let creators = object.get("creators").and_then(Value::as_array).cloned().unwrap_or_default();
    let field = |key: &str| object.get(key).and_then(Value::as_str).unwrap_or_default();
    let mut lines = vec![
        format!("author: {}", json_quote(field("author"))),
        format!("description: {}", json_quote(field("description"))),
        format!("version: {}", json_quote(field("version"))),
        "creators:".to_owned(),
    ];
    lines.extend(creators.iter().map(|creator| format!("  - {}", json_quote(creator.as_str().unwrap_or_default()))));
    let mut yaml = lines.join("\n");
    yaml.push('\n');
    yaml
}

pub(crate) struct ArchiveTheme {
    pub(crate) theme: Value,
    pub(crate) metadata: Value,
    pub(crate) resources: BTreeMap<String, Vec<u8>>,
    pub(crate) functions: BTreeMap<String, Vec<u8>>,
}

pub(crate) fn read_archive(content: &[u8]) -> Result<ArchiveTheme, String> {
    if content.is_empty() || content.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(err("ZIP темы пустой или превышает 8 MiB"));
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(content)).map_err(|_| err("Не удалось прочитать ZIP темы"))?;
    if archive.len() < 2 || archive.len() > MAX_ARCHIVE_FILES {
        return Err(err("ZIP темы неполный или содержит слишком много файлов"));
    }
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut seen_paths = HashSet::new();
    let mut total_size: u64 = 0;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).map_err(|_| err("Не удалось прочитать ZIP темы"))?;
        if member.is_dir() { return Err(err("ZIP темы не должен содержать записи каталогов")); }
        let name = member.name().map(|value| value.into_owned()).map_err(|_| err("Некорректный путь в ZIP темы"))?;
        if name.contains('\\') || name.starts_with('/') || name.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(err("ZIP темы содержит небезопасный путь"));
        }
        let permitted_path = name == "theme.json" || name == "theme.yml"
            || safe_relative_path(&name, "resources/") || safe_relative_path(&name, "functions/");
        if !permitted_path { return Err(err("ZIP темы содержит неожиданный путь")); }
        if !seen_paths.insert(name.to_ascii_lowercase()) || files.contains_key(&name) { return Err(err("ZIP темы содержит дублирующийся путь")); }
        if let Some(mode) = member.unix_mode() {
            let kind = mode & 0o170000;
            if kind == 0o120000 || (kind != 0 && kind != 0o100000) {
                return Err(err("ZIP темы не может содержать символьные ссылки или специальные файлы"));
            }
        }
        let limit = match name.as_str() {
            "theme.json" => MAX_JSON_BYTES,
            "theme.yml" => MAX_METADATA_BYTES,
            _ if name.starts_with("resources/") => MAX_RESOURCE_BYTES as u64,
            _ => MAX_FUNCTION_BYTES as u64,
        };
        let declared = member.size();
        if declared > limit { return Err(err("Файл в ZIP темы превышает ограничение размера")); }
        let mut buffer = Vec::new();
        member.by_ref().take(limit + 1).read_to_end(&mut buffer).map_err(|_| err("Не удалось прочитать ZIP темы"))?;
        if buffer.len() as u64 > limit || buffer.len() as u64 != declared { return Err(err("Файл в ZIP темы превышает ограничение размера или повреждён")); }
        total_size = total_size.saturating_add(buffer.len() as u64);
        if total_size > MAX_UNPACKED_BYTES { return Err(err("Распакованный ZIP темы слишком велик")); }
        files.insert(name, buffer);
    }
    let raw_json = files.get("theme.json").ok_or_else(|| err("В ZIP темы отсутствует theme.json"))?;
    let raw_yaml = files.get("theme.yml").ok_or_else(|| err("В ZIP темы отсутствует theme.yml"))?;
    let theme: Value = serde_json::from_slice(raw_json).map_err(|_| err("theme.json должен содержать корректный JSON в UTF-8"))?;
    let metadata = parse_theme_yml(raw_yaml)?;
    let normalized = normalize_theme(&theme, &metadata, "local", "")?;
    let resource_paths = match normalized.get("resources") {
        Some(value) => validate_resource_map(value)?,
        None => BTreeMap::new(),
    };
    let function_paths = match normalized.get("plugin") {
        Some(value) => validate_plugin(value)?.1,
        None => Vec::new(),
    };
    let expected: HashSet<String> = std::iter::once("theme.json".to_owned())
        .chain(std::iter::once("theme.yml".to_owned()))
        .chain(resource_paths.values().cloned())
        .chain(function_paths.iter().cloned())
        .collect();
    if files.keys().any(|path| !expected.contains(path)) || expected.iter().any(|path| !files.contains_key(path)) {
        return Err(err("ZIP темы содержит отсутствующие или незаявленные файлы"));
    }
    let mut resources = BTreeMap::new();
    for (key, path) in resource_paths {
        let bytes = files.remove(&path).ok_or_else(|| err("В ZIP темы отсутствует ресурс"))?;
        validate_resource_bytes(&key, &path, &bytes)?;
        resources.insert(key, bytes);
    }
    let mut functions = BTreeMap::new();
    for path in function_paths {
        let bytes = files.remove(&path).ok_or_else(|| err("В ZIP темы отсутствует исходник plugin"))?;
        functions.insert(path, bytes);
    }
    validate_asset_bundle(&normalized, &resources, &functions)?;
    Ok(ArchiveTheme { theme, metadata, resources, functions })
}

fn write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = super::platform::create_private_new(path).map_err(|error| format!("Не удалось сохранить тему: {error}"))?;
    file.write_all(contents).map_err(|error| format!("Не удалось сохранить тему: {error}"))?;
    file.sync_all().map_err(|error| format!("Не удалось сохранить тему: {error}"))?;
    Ok(())
}

fn pretty_json(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("тема сериализуется");
    text.push('\n');
    text
}

pub(crate) fn themes_root() -> Result<PathBuf, String> {
    let root = launcher_roots().data_home.join("themes");
    let metadata = fs::symlink_metadata(&root);
    if let Ok(meta) = &metadata {
        if meta.is_symlink() {
            return Err(err("Каталог тем является символьной ссылкой"));
        }
    }
    fs::create_dir_all(&root).map_err(|error| format!("Каталог тем недоступен: {error}"))?;
    super::platform::restrict_dir(&root);
    let metadata = fs::symlink_metadata(&root).map_err(|error| format!("Каталог тем недоступен: {error}"))?;
    if metadata.is_symlink() || !metadata.is_dir() {
        return Err(err("Каталог тем недоступен"));
    }
    Ok(root)
}

fn read_tree_files(directory: &Path, prefix: &str, files: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let root = directory.join(prefix);
    let root_meta = match fs::symlink_metadata(&root) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(err("Пакет локальной темы повреждён")),
    };
    if root_meta.is_symlink() || !root_meta.is_dir() { return Err(err("Пакет локальной темы повреждён")); }
    fn visit(base: &Path, current: &Path, prefix: &str, files: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
        for entry in fs::read_dir(current).map_err(|_| err("Пакет локальной темы повреждён"))? {
            let entry = entry.map_err(|_| err("Пакет локальной темы повреждён"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| err("Пакет локальной темы повреждён"))?;
            if metadata.is_symlink() { return Err(err("Пакет локальной темы содержит символьную ссылку")); }
            let relative = path.strip_prefix(base).map_err(|_| err("Пакет локальной темы повреждён"))?.to_string_lossy().replace('\\', "/");
            let logical_path = format!("{prefix}{relative}");
            if metadata.is_dir() {
                if !safe_relative_path(&format!("{logical_path}/placeholder"), if prefix == "resources/" { "resources/" } else { "functions/" }) {
                    return Err(err("Пакет локальной темы содержит небезопасный путь"));
                }
                let count_before = files.len();
                visit(base, &path, prefix, files)?;
                if files.len() == count_before { return Err(err("Пакет локальной темы содержит пустой каталог")); }
            } else if metadata.is_file() {
                let per_file_limit = if prefix == "resources/" { MAX_RESOURCE_BYTES as u64 } else { MAX_FUNCTION_BYTES as u64 };
                let current_size: u64 = files.values().map(|contents| contents.len() as u64).sum();
                if !safe_relative_path(&logical_path, prefix) || metadata.len() > per_file_limit
                    || files.len() >= MAX_ASSET_FILES || current_size.saturating_add(metadata.len()) > MAX_UNPACKED_BYTES
                {
                    return Err(err("Пакет локальной темы содержит недопустимый файл"));
                }
                let bytes = fs::read(&path).map_err(|_| err("Не удалось прочитать файл локальной темы"))?;
                if bytes.len() as u64 != metadata.len() { return Err(err("Размер файла локальной темы изменился во время чтения")); }
                if files.keys().any(|existing| existing.eq_ignore_ascii_case(&logical_path)) {
                    return Err(err("Пакет локальной темы содержит дублирующийся путь"));
                }
                files.insert(logical_path, bytes);
            } else {
                return Err(err("Пакет локальной темы содержит специальный файл"));
            }
        }
        Ok(())
    }
    visit(&root, &root, prefix, files)
}

fn load_asset_bundle(directory: &Path, theme: &Value) -> Result<(BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>), String> {
    let mut resource_files = BTreeMap::new();
    let mut function_files = BTreeMap::new();
    read_tree_files(directory, "resources/", &mut resource_files)?;
    read_tree_files(directory, "functions/", &mut function_files)?;
    let resource_paths = match theme.get("resources") {
        Some(value) => validate_resource_map(value)?,
        None => BTreeMap::new(),
    };
    let function_paths = match theme.get("plugin") {
        Some(value) => validate_plugin(value)?.1,
        None => Vec::new(),
    };
    let mut resources = BTreeMap::new();
    for (key, path) in resource_paths {
        let bytes = resource_files.remove(&path).ok_or_else(|| err("Пакет локальной темы не содержит заявленный ресурс"))?;
        validate_resource_bytes(&key, &path, &bytes)?;
        resources.insert(key, bytes);
    }
    let mut functions = BTreeMap::new();
    for path in function_paths {
        let bytes = function_files.remove(&path).ok_or_else(|| err("Пакет локальной темы не содержит заявленный исходник"))?;
        functions.insert(path, bytes);
    }
    if !resource_files.is_empty() || !function_files.is_empty() {
        return Err(err("Пакет локальной темы содержит незаявленные файлы"));
    }
    validate_asset_bundle(theme, &resources, &functions)?;
    Ok((resources, functions))
}

fn read_theme(directory: &Path) -> Result<Value, String> {
    let theme_path = directory.join("theme.json");
    let metadata_path = directory.join("theme.yml");
    for path in [&theme_path, &metadata_path] {
        let meta = fs::symlink_metadata(path).map_err(|_| err("Пакет локальной темы повреждён"))?;
        if meta.is_symlink() || !meta.is_file() {
            return Err(err("Пакет локальной темы повреждён"));
        }
    }
    if fs::symlink_metadata(&theme_path).map(|meta| meta.len()).unwrap_or(u64::MAX) > MAX_JSON_BYTES
        || fs::symlink_metadata(&metadata_path).map(|meta| meta.len()).unwrap_or(u64::MAX) > MAX_METADATA_BYTES
    {
        return Err(err("Пакет локальной темы превышает ограничение размера"));
    }
    let raw_theme: Value = serde_json::from_slice(&fs::read(&theme_path).map_err(|_| err("Не удалось прочитать локальную тему"))?)
        .map_err(|_| err("Не удалось прочитать локальную тему"))?;
    let metadata = parse_theme_yml(&fs::read(&metadata_path).map_err(|_| err("Не удалось прочитать локальную тему"))?)?;

    let mut source = "local".to_owned();
    let mut source_url = String::new();
    let origin_path = directory.join("origin.json");
    if fs::symlink_metadata(&origin_path).is_ok() {
        let meta = fs::symlink_metadata(&origin_path).map_err(|_| err("Метаданные источника темы повреждены"))?;
        if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_ORIGIN_BYTES {
            return Err(err("Метаданные источника темы повреждены"));
        }
        let origin: Value = serde_json::from_slice(&fs::read(&origin_path).map_err(|_| err("Метаданные источника темы повреждены"))?)
            .map_err(|_| err("Метаданные источника темы повреждены"))?;
        if let Some(stored) = origin.get("source").and_then(Value::as_str) {
            if stored == "local" || stored == "github" {
                source = stored.to_owned();
                source_url = origin.get("source_url").and_then(Value::as_str).unwrap_or("").to_owned();
            }
        }
    }
    let theme = normalize_theme(&raw_theme, &metadata, &source, &source_url)?;
    let _ = load_asset_bundle(directory, &theme)?;
    for entry in fs::read_dir(directory).map_err(|_| err("Пакет локальной темы повреждён"))? {
        let entry = entry.map_err(|_| err("Пакет локальной темы повреждён"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !["theme.json", "theme.yml", "origin.json", "resources", "functions"].contains(&name.as_str()) {
            return Err(err("Пакет локальной темы содержит неожиданный файл"));
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| err("Пакет локальной темы повреждён"))?;
        if meta.is_symlink() { return Err(err("Пакет локальной темы содержит символьную ссылку")); }
    }
    if theme["id"].as_str() != Some(directory.file_name().and_then(|name| name.to_str()).unwrap_or("")) {
        return Err(err("id темы не совпадает с каталогом"));
    }
    Ok(theme)
}

pub fn list() -> Result<Vec<Value>, String> {
    let root = themes_root()?;
    let mut names: Vec<String> = fs::read_dir(&root)
        .map_err(|error| format!("Каталог тем недоступен: {error}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.truncate(THEMES_LIST_LIMIT);
    let mut themes = Vec::new();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let directory = root.join(&name);
        let meta = match fs::symlink_metadata(&directory) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.is_symlink() || !meta.is_dir() {
            continue;
        }
        if let Ok(theme) = read_theme(&directory) {
            themes.push(theme);
        }
    }
    Ok(themes)
}

fn existing_directory(root: &Path, theme_id: &str) -> Result<PathBuf, String> {
    if !theme_id_re().is_match(theme_id) {
        return Err(err("Некорректный id темы"));
    }
    let directory = root.join(theme_id);
    let meta = fs::symlink_metadata(&directory).map_err(|_| err("Локальная тема не найдена"))?;
    if meta.is_symlink() || !meta.is_dir() {
        return Err(err("Локальная тема не найдена"));
    }
    Ok(directory)
}

fn write_bundle_file(root: &Path, path: &str, contents: &[u8]) -> Result<(), String> {
    let prefix = if path.starts_with("resources/") { "resources/" } else if path.starts_with("functions/") { "functions/" } else { return Err(err("Некорректный путь файла темы")); };
    if !safe_relative_path(path, prefix) { return Err(err("Некорректный путь файла темы")); }
    let relative = path.strip_prefix(prefix).ok_or_else(|| err("Некорректный путь файла темы"))?;
    let target = root.join(prefix).join(relative);
    let parent = target.parent().ok_or_else(|| err("Некорректный путь файла темы"))?;
    fs::create_dir_all(parent).map_err(|error| format!("Не удалось сохранить файл темы: {error}"))?;
    super::platform::restrict_dir(&root.join(prefix));
    write_private(&target, contents)
}

pub(crate) fn persist_with_files(
    root: &Path,
    theme: &Value,
    resources: &BTreeMap<String, Vec<u8>>,
    functions: &BTreeMap<String, Vec<u8>>,
    replace: bool,
) -> Result<Value, String> {
    validate_asset_bundle(theme, resources, functions)?;
    let theme_id = theme["id"].as_str().unwrap_or_default().to_owned();
    if !theme_id_re().is_match(&theme_id) { return Err(err("Некорректный id темы")); }
    let target = root.join(&theme_id);
    let target_meta = fs::symlink_metadata(&target);
    if matches!(&target_meta, Ok(meta) if meta.is_symlink()) {
        return Err(err("Каталог темы является символьной ссылкой"));
    }
    let exists = target_meta.is_ok();
    if exists && !target_meta.is_ok_and(|meta| meta.is_dir()) {
        return Err(err("Путь локальной темы занят не каталогом"));
    }
    if exists && !replace {
        return Err(err(&format!("Тема «{theme_id}» уже сохранена")));
    }

    let unique = format!("{}-{}", std::process::id(), epoch_nanos());
    let staging = root.join(format!(".theme-install-{unique}"));
    let backup = root.join(format!(".theme-backup-{unique}"));
    fs::create_dir(&staging).map_err(|error| format!("Не удалось сохранить тему: {error}"))?;
    let result = (|| -> Result<Value, String> {
        write_private(&staging.join("theme.json"), pretty_json(&theme_document(theme)).as_bytes())?;
        write_private(&staging.join("theme.yml"), metadata_yaml(theme).as_bytes())?;
        let origin = json!({ "source": theme["source"].clone(), "source_url": theme["source_url"].clone() });
        write_private(&staging.join("origin.json"), pretty_json(&origin).as_bytes())?;
        for (key, bytes) in resources {
            let path = theme.get("resources").and_then(|value| value.get(key)).and_then(Value::as_str).unwrap_or_default();
            write_bundle_file(&staging, path, bytes)?;
        }
        for (path, bytes) in functions { write_bundle_file(&staging, path, bytes)?; }
        if exists {
            fs::rename(&target, &backup).map_err(|error| format!("Не удалось сохранить тему: {error}"))?;
        }
        if let Err(error) = fs::rename(&staging, &target) {
            if exists && fs::symlink_metadata(&backup).is_ok() {
                let _ = fs::rename(&backup, &target);
            }
            return Err(format!("Не удалось сохранить тему: {error}"));
        }
        let saved = read_theme(&target)?;
        let _ = fs::remove_dir_all(&backup);
        Ok(saved)
    })();
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&backup);
    result
}

pub(crate) fn persist(root: &Path, theme: &Value, replace: bool) -> Result<Value, String> {
    persist_with_files(root, theme, &BTreeMap::new(), &BTreeMap::new(), replace)
}

fn epoch_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_nanos()).unwrap_or(0)
}

fn decode_archive_field(request: &Value) -> Result<Vec<u8>, String> {
    let raw = request.get("data").and_then(Value::as_str).unwrap_or_default().trim();
    if raw.is_empty() || raw.len() > ((MAX_ARCHIVE_BYTES as usize + 2) / 3) * 4 + 8 {
        return Err(err("ZIP темы пустой или превышает 8 MiB"));
    }
    base64::engine::general_purpose::STANDARD.decode(raw).map_err(|_| err("ZIP темы пустой или превышает 8 MiB"))
}

fn decode_resource_data(request: &Value, theme: &Value) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let expected = match theme.get("resources") { Some(value) => validate_resource_map(value)?, None => BTreeMap::new() };
    let data = match request.get("resource_data").or_else(|| theme.get("resource_data")) {
        Some(value) => value.as_object().ok_or_else(|| err("resource_data должен быть объектом"))?,
        None if expected.is_empty() => return Ok(BTreeMap::new()),
        None => return Err(err("Для ресурсов требуется resource_data с base64-данными")),
    };
    if data.len() != expected.len() || data.keys().any(|key| !expected.contains_key(key)) {
        return Err(err("resource_data должен точно соответствовать resources"));
    }
    let mut result = BTreeMap::new();
    for (key, path) in expected {
        let encoded = data[&key].as_str().ok_or_else(|| err("Данные ресурса должны быть base64-строкой"))?;
        if encoded.len() > ((MAX_RESOURCE_BYTES + 2) / 3) * 4 + 8 { return Err(err("Файл ресурса слишком большой")); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|_| err("Некорректные base64-данные ресурса"))?;
        validate_resource_bytes(&key, &path, &bytes)?;
        result.insert(key, bytes);
    }
    Ok(result)
}

fn decode_function_data(request: &Value, theme: &Value) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let expected = match theme.get("plugin") { Some(value) => validate_plugin(value)?.1, None => Vec::new() };
    let data = match request.get("function_data") {
        Some(value) => value.as_object().ok_or_else(|| err("function_data должен быть объектом"))?,
        None if expected.is_empty() => return Ok(BTreeMap::new()),
        None => return Err(err("Для plugin требуется function_data с исходниками")),
    };
    if data.len() != expected.len() || data.keys().any(|path| !expected.contains(path)) {
        return Err(err("function_data должен точно соответствовать plugin.sources"));
    }
    let mut result = BTreeMap::new();
    let mut total = 0usize;
    for path in expected {
        let source = data[&path].as_str().ok_or_else(|| err("Исходник plugin должен быть UTF-8 строкой"))?;
        if source.is_empty() || source.len() > MAX_FUNCTION_BYTES { return Err(err("Исходник plugin пустой или слишком большой")); }
        total = total.saturating_add(source.len());
        if total > MAX_FUNCTION_TOTAL_BYTES { return Err(err("Исходники plugin превышают ограничение размера")); }
        result.insert(path, source.as_bytes().to_vec());
    }
    Ok(result)
}

fn with_resource_data(mut theme: Value, resources: &BTreeMap<String, Vec<u8>>) -> Value {
    let encoded: Map<String, Value> = resources.iter().map(|(key, bytes)| (key.clone(), json!(base64::engine::general_purpose::STANDARD.encode(bytes)))).collect();
    if let Some(object) = theme.as_object_mut() { object.insert("resource_data".into(), Value::Object(encoded)); }
    theme
}

fn live_themes() -> Result<Vec<Value>, String> {
    let themes = list()?;
    let root = themes_root()?;
    themes.into_iter().map(|theme| {
        let directory = existing_directory(&root, theme.get("id").and_then(Value::as_str).unwrap_or_default())?;
        let (resources, _) = load_asset_bundle(&directory, &theme)?;
        Ok(with_resource_data(theme, &resources))
    }).collect()
}

pub fn theme_list(_request: &Value) -> Result<Value, String> {
    Ok(json!({ "themes": live_themes()? }))
}

pub fn theme_preview(request: &Value) -> Result<Value, String> {
    let content = decode_archive_field(request)?;
    let archive = read_archive(&content)?;
    let theme = normalize_theme(&archive.theme, &archive.metadata, "local", "")?;
    Ok(json!({ "theme": with_resource_data(theme, &archive.resources) }))
}

fn metadata_for_save(request: &Value, theme: &Value) -> Value {
    request.get("metadata").cloned().unwrap_or_else(|| {
        json!({
            "author": theme.get("author").cloned().unwrap_or(Value::Null),
            "description": theme.get("description").cloned().unwrap_or(Value::Null),
            "version": theme.get("version").cloned().unwrap_or(Value::Null),
            "creators": theme.get("creators").cloned().unwrap_or(Value::Null)
        })
    })
}

fn reusable_bundle(root: &Path, theme: &Value) -> Result<(BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>), String> {
    let theme_id = theme.get("id").and_then(Value::as_str).unwrap_or_default();
    if !theme_id_re().is_match(theme_id) { return Err(err("Некорректный id темы")); }
    let target = root.join(theme_id);
    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((BTreeMap::new(), BTreeMap::new())),
        Err(_) => Err(err("Не удалось проверить сохранённые данные темы")),
        Ok(_) => {
            let directory = existing_directory(root, theme_id)?;
            let previous = read_theme(&directory)?;
            let (old_resources, old_functions) = load_asset_bundle(&directory, &previous)?;
            let resources = if previous.get("resources") == theme.get("resources") { old_resources } else { BTreeMap::new() };
            let functions = if previous.get("plugin") == theme.get("plugin") { old_functions } else { BTreeMap::new() };
            Ok((resources, functions))
        }
    }
}

pub fn theme_save(request: &Value) -> Result<Value, String> {
    let theme = request.get("theme").cloned().unwrap_or(Value::Null);
    let metadata = metadata_for_save(request, &theme);
    let replace = request.get("replace").and_then(Value::as_bool).unwrap_or(false);
    let normalized = normalize_theme(&theme, &metadata, "local", "")?;
    let root = themes_root()?;
    let (previous_resources, previous_functions) = reusable_bundle(&root, &normalized)?;
    let resources = if request.get("resource_data").is_some() || theme.get("resource_data").is_some() {
        decode_resource_data(request, &normalized)?
    } else {
        previous_resources
    };
    let functions = if request.get("function_data").is_some() {
        decode_function_data(request, &normalized)?
    } else {
        previous_functions
    };
    validate_asset_bundle(&normalized, &resources, &functions)?;
    let saved = persist_with_files(&root, &normalized, &resources, &functions, replace)?;
    Ok(json!({ "theme": with_resource_data(saved, &resources), "themes": live_themes()? }))
}

pub fn theme_import(request: &Value) -> Result<Value, String> {
    let content = decode_archive_field(request)?;
    let archive = read_archive(&content)?;
    let normalized = normalize_theme(&archive.theme, &archive.metadata, "local", "")?;
    let root = themes_root()?;
    let saved = persist_with_files(&root, &normalized, &archive.resources, &archive.functions, request.get("replace").and_then(Value::as_bool).unwrap_or(false))?;
    Ok(json!({ "theme": with_resource_data(saved, &archive.resources), "themes": live_themes()? }))
}

pub fn theme_delete(request: &Value) -> Result<Value, String> {
    let theme_id = request.get("theme_id").and_then(Value::as_str).unwrap_or_default();
    delete_at(&themes_root()?, theme_id)?;
    Ok(json!({ "removed": true, "themes": list()? }))
}

fn delete_at(root: &Path, theme_id: &str) -> Result<(), String> {
    if !theme_id_re().is_match(theme_id) {
        return Err(err("Некорректный id темы"));
    }
    let target = root.join(theme_id);
    let meta = fs::symlink_metadata(&target);
    if matches!(&meta, Ok(meta) if meta.is_symlink()) {
        return Err(err("Каталог темы является символьной ссылкой"));
    }
    if !meta.is_ok_and(|meta| meta.is_dir()) {
        return Err(err("Локальная тема не найдена"));
    }
    fs::remove_dir_all(&target).map_err(|error| format!("Не удалось удалить тему: {error}"))
}

fn export_at(root: &Path, theme_id: &str) -> Result<(String, Vec<u8>), String> {
    let directory = existing_directory(root, theme_id)?;
    let theme = read_theme(&directory)?;
    let (resources, functions) = load_asset_bundle(&directory, &theme)?;
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("theme.json", options).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        writer.write_all(pretty_json(&theme_document(&theme)).as_bytes()).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        writer.start_file("theme.yml", options).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        writer.write_all(metadata_yaml(&theme).as_bytes()).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        for (key, bytes) in &resources {
            let path = theme.get("resources").and_then(|value| value.get(key)).and_then(Value::as_str).unwrap_or_default();
            writer.start_file(path, options).map_err(|_| err("Не удалось собрать ZIP темы"))?;
            writer.write_all(bytes).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        }
        for (path, bytes) in &functions {
            writer.start_file(path, options).map_err(|_| err("Не удалось собрать ZIP темы"))?;
            writer.write_all(bytes).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        }
        writer.finish().map_err(|_| err("Не удалось собрать ZIP темы"))?;
    }
    let content = buffer.into_inner();
    if content.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(err("ZIP темы превышает ограничение размера"));
    }
    let file_name = format!("{}-{}.zip", theme["id"].as_str().unwrap_or_default(), theme["version"].as_str().unwrap_or_default());
    Ok((file_name, content))
}

pub fn theme_export(request: &Value) -> Result<Value, String> {
    let theme_id = request.get("theme_id").and_then(Value::as_str).unwrap_or_default();
    let (file_name, content) = export_at(&themes_root()?, theme_id)?;
    Ok(json!({ "file_name": file_name, "data": base64::engine::general_purpose::STANDARD.encode(&content) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-themes-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    fn sample_theme() -> Value {
        json!({
            "schema_version": 1,
            "id": "midnight-sky",
            "name": "Midnight Sky",
            "mode": "dark",
            "accent": "#4F7CFF",
            "accent_strong": "#3B5FD9",
            "background": "#0B0F1A",
            "panel": "#141B2E",
            "panel_soft": "#1B2338",
            "density": "comfortable",
            "sidebar_mode": "expanded"
        })
    }

    fn sample_theme_v2() -> Value {
        json!({
            "schema_version": 2,
            "id": "midnight-sky",
            "name": "Midnight Sky",
            "mode": "dark",
            "accent": "#4F7CFF",
            "accent_strong": "#3B5FD9",
            "background": "#0B0F1A",
            "panel": "#141B2E",
            "panel_soft": "#1B2338",
            "density": "comfortable",
            "sidebar_mode": "expanded",
            "locale": "ru",
            "button_customization": {
                "width": 180,
                "height": 44,
                "fill": true,
                "border": true,
                "font_family": "inter",
                "font_size": 14,
                "outline": {"enabled": true, "width": 1, "color": "#4F7CFF"},
                "texture_fit": "cover",
                "crop_enabled": false,
                "crop": {"x": 0, "y": 0, "width": 100, "height": 100},
                "labels": {"instances": "Миры", "accounts": "Аккаунты"},
                "overrides": {
                    "hero-play:survival%20one": {"texture": "resources/per-instance.png"},
                    "card-button:survival%20one%3Asettings": {"sound": "resources/per-instance.ogg"}
                }
            },
            "resources": {
                "texture": "resources/button.png",
                "sound": "resources/click.ogg",
                "hero-play:survival%20one-texture": "resources/per-instance.png",
                "card-button:survival%20one%3Asettings-sound": "resources/per-instance.ogg"
            },
            "plugin": {
                "id": "midnight-effects",
                "name": "Midnight effects",
                "version": "1.0.0",
                "api_version": "3",
                "entrypoint": "functions/index.js",
                "sources": ["functions/index.js", "functions/config.json"]
            }
        })
    }

    fn sample_yml() -> &'static str {
        "author: Автор # комментарий\ndescription: \"Тёмная тема\"\nversion: 1.0.0+meta\ncreators:\n  - \"Первый\"\n  - Второй\n"
    }

    fn sample_metadata() -> Value {
        parse_theme_yml(sample_yml().as_bytes()).expect("разбор yml")
    }

    fn archive_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            for (name, bytes) in entries {
                writer.start_file(*name, options).expect("создать запись ZIP");
                writer.write_all(bytes).expect("записать запись ZIP");
            }
            writer.finish().expect("завершить ZIP");
        }
        buffer.into_inner()
    }

    #[test]
    fn webm_sound_resources_require_an_ebml_header() {
        let path = "resources/button-click.webm";
        assert!(validate_resource_map(&json!({"sound": path})).is_ok());
        assert!(validate_resource_bytes("sound", path, b"\x1a\x45\xdf\xa3webm-data").is_ok());
        assert!(validate_resource_bytes("sound", path, b"not-webm").is_err());
    }

    #[test]
    fn button_resource_limits_match_the_editor_upload_limits() {
        let mut image = vec![0; MAX_BUTTON_IMAGE_BYTES];
        image[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(validate_resource_bytes("texture", "resources/button.png", &image).is_ok());
        image.push(0);
        assert!(validate_resource_bytes("texture", "resources/button.png", &image).is_err());

        let mut sound = vec![0; MAX_BUTTON_SOUND_BYTES];
        sound[..4].copy_from_slice(b"RIFF");
        sound[8..12].copy_from_slice(b"WAVE");
        assert!(validate_resource_bytes("sound", "resources/click.wav", &sound).is_ok());
        sound.push(0);
        assert!(validate_resource_bytes("sound", "resources/click.wav", &sound).is_err());
    }

    #[test]
    fn individual_button_overrides_validate_labels_and_asset_types() {
        let valid = json!({"overrides": {"hero-play": {"label": "Запустить", "width": 232}}});
        assert!(validate_button_customization(&valid).is_ok());
        let long_label = json!({"overrides": {"hero-play": {"label": "12345678901234567"}}});
        assert!(validate_button_customization(&long_label).is_err());
        let per_instance = json!({"overrides": {"hero-play:survival%20one": {"label": "Запустить", "width": 232}}});
        assert!(validate_button_customization(&per_instance).is_ok());
        let unsafe_identity = json!({"overrides": {"hero-play:../survival": {"width": 200}}});
        assert!(validate_button_customization(&unsafe_identity).is_err());
        let unknown_target = json!({"overrides": {"unknown": {"width": 200}}});
        assert!(validate_button_customization(&unknown_target).is_err());
        assert!(validate_resource_map(&json!({"hero-play-sound": "resources/button-hero-play-sound.ogg"})).is_ok());
        assert!(validate_resource_map(&json!({"hero-play:survival%20one-sound": "resources/button-survival.wav"})).is_ok());
        assert!(validate_resource_map(&json!({"unknown-sound": "resources/button.ogg"})).is_err());
        assert!(validate_resource_bytes("hero-play-sound", "resources/button.ogg", b"OggSsample").is_ok());
        assert!(validate_resource_bytes("hero-play-sound", "resources/button.ogg", b"not audio").is_err());
    }

    #[test]
    fn normalize_accepts_valid_theme_and_lowercases_colors() {
        let theme = normalize_theme(&sample_theme(), &sample_metadata(), "local", "").expect("валидная тема");
        assert_eq!(theme["id"], json!("midnight-sky"));
        assert_eq!(theme["accent"], json!("#4f7cff"));
        assert_eq!(theme["source_url"], json!(""));
        assert_eq!(theme["creators"], json!(["Первый", "Второй"]));
        assert_eq!(theme["featured"], json!(false));
    }

    #[test]
    fn normalize_rejects_bad_schema_id_and_colors() {
        let metadata = sample_metadata();
        assert!(normalize_theme(&json!({}), &metadata, "local", "").is_err());
        let mut bad_schema = sample_theme();
        bad_schema["schema_version"] = json!(3);
        assert!(normalize_theme(&bad_schema, &metadata, "local", "").is_err());
        let mut bad_id = sample_theme();
        bad_id["id"] = json!("midnight_sky");
        assert!(normalize_theme(&bad_id, &metadata, "local", "").is_err());
        let mut cased_id = sample_theme();
        cased_id["id"] = json!("Midnight-Sky");
        assert_eq!(normalize_theme(&cased_id, &metadata, "local", "").unwrap()["id"], json!("midnight-sky"));
        let mut bad_color = sample_theme();
        bad_color["panel"] = json!("#12345");
        let error = normalize_theme(&bad_color, &metadata, "local", "").unwrap_err();
        assert!(error.contains("panel"), "{error}");
        let mut bad_creators = sample_metadata();
        bad_creators["creators"] = json!([]);
        assert!(normalize_theme(&sample_theme(), &bad_creators, "local", "").is_err());
    }

    #[test]
    fn yml_parser_handles_quotes_comments_and_rejects_tabs() {
        let parsed = parse_theme_yml(sample_yml().as_bytes()).expect("yml");
        assert_eq!(parsed["author"], json!("Автор"));
        assert_eq!(parsed["version"], json!("1.0.0+meta"));
        assert!(parse_theme_yml(b"author: A\nversion: '1.#0'\ndescription: D\ncreators:\n  - 'It''s'\n").is_ok());
        assert!(parse_theme_yml(b"author: X\n").is_err());
        assert!(parse_theme_yml("author: X\nversion: 1\ndescription: d\ncreators:\n  - a\n\t- b\n".as_bytes()).is_err());
        assert!(parse_theme_yml(b"author: X\nversion: 1\ndescription: d\ncreators: [\"a\", \"b\"]\n").is_ok());
        assert!(parse_theme_yml(b"author: X\nversion: 1\ndescription: d\ncreators: [1]\n").is_err());
        assert!(parse_theme_yml(b"author: X\nversion: 1\nauthor: Y\ndescription: d\ncreators: []\n").is_err());
    }

    #[test]
    fn export_import_roundtrip_preserves_theme() {
        let root = temp_root("roundtrip");
        let theme = normalize_theme(&sample_theme(), &sample_metadata(), "local", "").unwrap();
        let saved = persist(&root, &theme, false).expect("сохранить");
        assert_eq!(saved["id"], json!("midnight-sky"));

        let (file_name, content) = export_at(&root, "midnight-sky").expect("экспорт");
        assert_eq!(file_name, "midnight-sky-1.0.0+meta.zip");
        let archive = read_archive(&content).expect("архив");
        let again = normalize_theme(&archive.theme, &archive.metadata, "local", "").expect("нормализация");
        assert_eq!(again["name"], json!("Midnight Sky"));
        assert_eq!(again["creators"], json!(["Первый", "Второй"]));

        assert!(persist(&root, &theme, false).is_err());
        assert!(persist(&root, &theme, true).is_ok());
        delete_at(&root, "midnight-sky").expect("удаление");
        assert!(delete_at(&root, "midnight-sky").is_err());
    }

    #[test]
    fn archive_validation_rejects_unexpected_layouts() {
        let theme = normalize_theme(&sample_theme(), &sample_metadata(), "local", "").unwrap();
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("nested/theme.json", options).unwrap();
            writer.write_all(pretty_json(&theme_document(&theme)).as_bytes()).unwrap();
            writer.start_file("theme.yml", options).unwrap();
            writer.write_all(sample_yml().as_bytes()).unwrap();
        }
        assert!(read_archive(&buffer.into_inner()).is_err());
        assert!(read_archive(b"").is_err());
        assert!(read_archive(&vec![0u8; 32]).is_err());
    }

    #[test]
    fn v2_assets_and_function_sources_roundtrip_without_embedding_payloads() {
        let root = temp_root("v2-roundtrip");
        let normalized = normalize_theme(&sample_theme_v2(), &sample_metadata(), "local", "").expect("валидная тема v2");
        let resources = BTreeMap::from([
            ("texture".to_owned(), b"\x89PNG\r\n\x1a\nsmall-png".to_vec()),
            ("sound".to_owned(), b"OggSsample-audio".to_vec()),
            ("hero-play:survival%20one-texture".to_owned(), b"\x89PNG\r\n\x1a\nbutton-png".to_vec()),
            ("card-button:survival%20one%3Asettings-sound".to_owned(), b"OggSbutton-audio".to_vec()),
        ]);
        let functions = BTreeMap::from([
            ("functions/index.js".to_owned(), b"export const inert = true;\n".to_vec()),
            ("functions/config.json".to_owned(), b"{\"enabled\":false}\n".to_vec()),
        ]);
        let saved = persist_with_files(&root, &normalized, &resources, &functions, false).expect("сохранить тему v2");
        assert_eq!(saved["schema_version"], json!(2));
        assert_eq!(saved["locale"], json!("ru"));
        assert_eq!(saved["button_customization"]["labels"]["instances"], json!("Миры"));
        assert_eq!(saved["button_customization"]["overrides"]["hero-play:survival%20one"]["texture"], json!("resources/per-instance.png"));
        assert_eq!(saved["button_customization"]["overrides"]["card-button:survival%20one%3Asettings"]["sound"], json!("resources/per-instance.ogg"));
        let stored_json = fs::read(root.join("midnight-sky/theme.json")).expect("прочитать theme.json");
        assert!(!String::from_utf8_lossy(&stored_json).contains(&base64::engine::general_purpose::STANDARD.encode(&resources["texture"])));
        assert!(fs::read(root.join("midnight-sky/functions/index.js")).is_ok());

        let (_, exported) = export_at(&root, "midnight-sky").expect("экспортировать ZIP");
        let archive = read_archive(&exported).expect("импортировать ZIP");
        assert_eq!(archive.resources, resources);
        assert_eq!(archive.functions, functions);
        let roundtrip = normalize_theme(&archive.theme, &archive.metadata, "local", "").expect("нормализовать ZIP");
        assert_eq!(roundtrip["button_customization"], normalized["button_customization"]);
        assert!(archive.theme.get("resource_data").is_none());
    }

    #[test]
    fn archive_rejects_traversal_backslashes_symlinks_unreferenced_and_oversized_files() {
        let theme = normalize_theme(&sample_theme(), &sample_metadata(), "local", "").unwrap();
        let json = pretty_json(&theme_document(&theme));
        let yaml = sample_yml();
        let base = [("theme.json", json.as_bytes()), ("theme.yml", yaml.as_bytes())];
        let traversal = archive_with(&[base[0], base[1], ("resources/../escape.png", b"x")]);
        assert!(read_archive(&traversal).is_err());
        let backslash = archive_with(&[base[0], base[1], ("resources\\escape.png", b"x")]);
        assert!(read_archive(&backslash).is_err());
        let case_collision = archive_with(&[base[0], base[1], ("resources/file.png", b"a"), ("resources/FILE.png", b"b")]);
        assert!(read_archive(&case_collision).is_err());
        let orphan = archive_with(&[base[0], base[1], ("resources/orphan.png", b"\x89PNG\r\n\x1a\n")]);
        assert!(read_archive(&orphan).is_err());
        let oversized_payload = vec![b'x'; MAX_RESOURCE_BYTES + 1];
        let oversized = archive_with(&[base[0], base[1], ("resources/large.png", &oversized_payload)]);
        assert!(read_archive(&oversized).is_err());

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            writer.start_file("theme.json", zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(json.as_bytes()).unwrap();
            writer.start_file("theme.yml", zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(yaml.as_bytes()).unwrap();
            writer.start_file("resources/link.png", zip::write::SimpleFileOptions::default().unix_permissions(0o120777)).unwrap();
            writer.write_all(b"target").unwrap();
        }
        assert!(read_archive(&buffer.into_inner()).is_err());
    }

    #[test]
    fn v2_fields_and_labels_are_bounded() {
        let metadata = sample_metadata();
        let normalized = normalize_theme(&sample_theme_v2(), &metadata, "local", "").unwrap();
        let texture = b"\x89PNG\r\n\x1a\nsmall-png";
        let sound = b"OggSsample-audio";
        let request = json!({"resource_data": {
            "texture": base64::engine::general_purpose::STANDARD.encode(texture),
            "sound": base64::engine::general_purpose::STANDARD.encode(sound)
        }});
        let decoded = decode_resource_data(&request, &normalized).expect("bounded resource_data");
        assert_eq!(decoded["texture"], texture);
        assert_eq!(decoded["sound"], sound);
        let mut long_label = sample_theme_v2();
        long_label["button_customization"]["labels"]["instances"] = json!("12345678901234567");
        assert!(normalize_theme(&long_label, &metadata, "local", "").is_err());
        let mut bad_width = sample_theme_v2();
        bad_width["button_customization"]["width"] = json!(281);
        assert!(normalize_theme(&bad_width, &metadata, "local", "").is_err());
        let mut invalid_locale = sample_theme_v2();
        invalid_locale["locale"] = json!("fr");
        assert!(normalize_theme(&invalid_locale, &metadata, "local", "").is_err());
        let mut bad_crop = sample_theme_v2();
        bad_crop["button_customization"]["crop"]["x"] = json!(90);
        assert!(normalize_theme(&bad_crop, &metadata, "local", "").is_err());
        let mut bad_entrypoint = sample_theme_v2();
        bad_entrypoint["plugin"]["entrypoint"] = json!("functions/../evil.js");
        assert!(normalize_theme(&bad_entrypoint, &metadata, "local", "").is_err());
    }

    #[test]
    fn read_theme_requires_matching_directory_name() {
        let root = temp_root("mismatch");
        let theme = normalize_theme(&sample_theme(), &sample_metadata(), "local", "").unwrap();
        persist(&root, &theme, false).expect("сохранить");
        // имя каталога не должно расходиться с id темы
        fs::rename(root.join("midnight-sky"), root.join("other-id")).expect("переименовать");
        assert!(read_theme(&root.join("other-id")).is_err());
        assert!(read_theme(&root.join("midnight-sky")).is_err());
        assert_eq!(read_theme(&root.join("midnight-sky")).unwrap_err(), "Пакет локальной темы повреждён");
    }
}
