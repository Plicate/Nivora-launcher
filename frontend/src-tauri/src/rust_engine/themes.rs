// локальное хранилище тем оформления
use base64::Engine;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::state::launcher_roots;

const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 64 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024;
const MAX_UNPACKED_BYTES: u64 = MAX_JSON_BYTES + MAX_METADATA_BYTES;
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
    let schema_version = theme_object.get("schema_version").unwrap_or(&Value::Null);
    let schema_version = match schema_version {
        Value::Null => 1,
        Value::Number(number) => number.as_i64().unwrap_or(-1),
        _ => -1,
    };
    if schema_version != 1 {
        return Err(err("Эта версия схемы темы не поддерживается"));
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
    result.insert("schema_version".into(), json!(1));
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
        document.insert(key.to_owned(), source.get(key).cloned().unwrap_or(Value::Null));
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
}

pub(crate) fn read_archive(content: &[u8]) -> Result<ArchiveTheme, String> {
    if content.is_empty() || content.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(err("ZIP темы пустой или превышает 8 MiB"));
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(content)).map_err(|_| err("Не удалось прочитать ZIP темы"))?;
    if archive.len() != 2 {
        return Err(err("ZIP темы должен содержать ровно theme.json и theme.yml"));
    }
    let mut files: std::collections::BTreeMap<String, Vec<u8>> = std::collections::BTreeMap::new();
    let mut total_size: u64 = 0;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).map_err(|_| err("Не удалось прочитать ZIP темы"))?;
        if member.is_dir() {
            return Err(err("ZIP темы может содержать только файлы в корне архива"));
        }
        let raw_name = member.name().map(|v| v.replace('\\', "/")).unwrap_or_default();
        let part = raw_name.split('/').next_back().unwrap_or("");
        if raw_name.contains('/') || part.is_empty() || part == "." || part == ".." {
            return Err(err("ZIP темы может содержать только файлы в корне архива"));
        }
        let name = part;
        if !["theme.json", "theme.yml"].contains(&name) || files.contains_key(name) {
            return Err(err("ZIP темы должен содержать ровно theme.json и theme.yml"));
        }
        if let Some(mode) = member.unix_mode().filter(|mode| mode & 0o170000 == 0o120000) {
            let _ = mode;
            return Err(err("ZIP темы не может содержать символьные ссылки"));
        }
        let limit = if name == "theme.json" { MAX_JSON_BYTES } else { MAX_METADATA_BYTES };
        let declared = member.size();
        if declared > limit {
            return Err(err(&format!("Файл {name} в ZIP темы слишком большой")));
        }
        total_size += declared;
        let mut buffer = Vec::new();
        member.by_ref().take(limit).read_to_end(&mut buffer).map_err(|_| err("Не удалось прочитать ZIP темы"))?;
        if buffer.len() as u64 > limit {
            return Err(err(&format!("Файл {name} в ZIP темы слишком большой")));
        }
        files.insert(name.to_owned(), buffer);
    }
    if files.len() != 2 || total_size > MAX_UNPACKED_BYTES {
        return Err(err("ZIP темы неполный или слишком большой"));
    }
    let raw_json = &files["theme.json"];
    let raw_yaml = &files["theme.yml"];
    let theme: Value = serde_json::from_slice(raw_json).map_err(|_| err("theme.json должен содержать корректный JSON в UTF-8"))?;
    if !theme.is_object() {
        return Err(err("theme.json должен содержать объект темы"));
    }
    Ok(ArchiveTheme { theme, metadata: parse_theme_yml(raw_yaml)? })
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

pub(crate) fn persist(root: &Path, theme: &Value, replace: bool) -> Result<Value, String> {
    let theme_id = theme["id"].as_str().unwrap_or_default().to_owned();
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

fn epoch_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_nanos()).unwrap_or(0)
}

fn decode_archive_field(request: &Value) -> Result<Vec<u8>, String> {
    let raw = request.get("data").and_then(Value::as_str).unwrap_or_default();
    base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|_| err("ZIP темы пустой или превышает 8 MiB"))
}

pub fn theme_list(_request: &Value) -> Result<Value, String> {
    Ok(json!({ "themes": list()? }))
}

pub fn theme_preview(request: &Value) -> Result<Value, String> {
    let content = decode_archive_field(request)?;
    let archive = read_archive(&content)?;
    Ok(json!({ "theme": normalize_theme(&archive.theme, &archive.metadata, "local", "")? }))
}

pub fn theme_save(request: &Value) -> Result<Value, String> {
    let theme = request.get("theme").cloned().unwrap_or(Value::Null);
    let metadata = request.get("metadata").cloned().unwrap_or(Value::Null);
    let replace = request.get("replace").and_then(Value::as_bool).unwrap_or(false);
    let normalized = normalize_theme(&theme, &metadata, "local", "")?;
    let root = themes_root()?;
    let saved = persist(&root, &normalized, replace)?;
    Ok(json!({ "theme": saved, "themes": list()? }))
}

pub fn theme_import(request: &Value) -> Result<Value, String> {
    let content = decode_archive_field(request)?;
    let archive = read_archive(&content)?;
    let normalized = normalize_theme(&archive.theme, &archive.metadata, "local", "")?;
    let root = themes_root()?;
    let saved = persist(&root, &normalized, request.get("replace").and_then(Value::as_bool).unwrap_or(false))?;
    Ok(json!({ "theme": saved, "themes": list()? }))
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
    let theme = read_theme(&existing_directory(root, theme_id)?)?;
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("theme.json", options).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        writer.write_all(pretty_json(&theme_document(&theme)).as_bytes()).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        writer.start_file("theme.yml", options).map_err(|_| err("Не удалось собрать ZIP темы"))?;
        writer.write_all(metadata_yaml(&theme).as_bytes()).map_err(|_| err("Не удалось собрать ZIP темы"))?;
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

    fn sample_yml() -> &'static str {
        "author: Автор # комментарий\ndescription: \"Тёмная тема\"\nversion: 1.0.0+meta\ncreators:\n  - \"Первый\"\n  - Второй\n"
    }

    fn sample_metadata() -> Value {
        parse_theme_yml(sample_yml().as_bytes()).expect("разбор yml")
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
        bad_schema["schema_version"] = json!(2);
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
