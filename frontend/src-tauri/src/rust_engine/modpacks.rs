// импорт и установка модпаков
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::net::HttpClient;

pub(crate) const MAX_MODPACK_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MODPACK_ENTRIES: usize = 20_000;
const MAX_MODPACK_UNCOMPRESSED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MODPACK_INDEX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MODPACK_MARKER_BYTES: u64 = 512 * 1024;
const MODPACK_MARKER: &str = ".trial-launcher-modpacks.json";
// архив CurseForge содержит manifest.json с projectID/fileID; моды скачиваются отдельно
const CURSEFORGE_MANIFEST_NAME: &str = "manifest.json";
const CURSEFORGE_MANIFEST_MAX_BYTES: u64 = 8 * 1024 * 1024;
const CURSEFORGE_RESOLVER_BASE: &str = "https://api.cfwidget.com";
const CURSEFORGE_DOWNLOAD_HOST_SUFFIX: &str = "forgecdn.net";
const CURSEFORGE_MAX_REMOTE_FILES: usize = 2000;
const MOD_PREVIEW_LIMIT: usize = 400;
const FTB_MAX_SEARCH_RESULTS: usize = 500;

const LOADER_MANAGED_ROOTS: [&str; 4] = ["versions", "libraries", "natives", "bin"];
// эти каталоги считаются префиксами архива, а не корнем экземпляра
const MODPACK_ROOT_PREFIXES: [&str; 4] = ["overrides", "client-overrides", ".minecraft", "minecraft"];
const MODPACK_METADATA_FILES: [&str; 7] = [
    "manifest.json",
    "modrinth.index.json",
    "modlist.html",
    "minecraftinstance.json",
    ".technicpack",
    "mmc-pack.json",
    "instance.cfg",
];

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

// ------------------------------------------------------------- утилиты ----

fn text_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Object(fields) => text_value(
            fields
                .get("name")
                .or_else(|| fields.get("displayName"))
                .or_else(|| fields.get("text"))
                .or_else(|| fields.get("url"))
                .unwrap_or(&Value::Null),
        ),
        Value::Array(items) => items
            .iter()
            .map(text_value)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => (if *flag { "True" } else { "False" }).to_owned(),
        Value::Null => String::new(),
    }
}

fn int_value(value: &Value) -> i64 {
    let parsed = match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .unwrap_or(0),
        Value::String(text) => text.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    parsed.max(0)
}

fn optional_int(value: &Value) -> Option<u64> {
    match value {
        Value::Null => None,
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .map(|value| value.max(0) as u64),
        Value::String(text) => text.trim().parse::<i64>().ok().map(|value| value.max(0) as u64),
        _ => None,
    }
}

fn game_versions(value: &Value) -> Vec<String> {
    let candidates = value
        .get("game_versions")
        .or_else(|| value.get("gameVersions"))
        .or_else(|| value.get("minecraftVersions"))
        .or_else(|| value.get("minecraft_versions"))
        .or_else(|| value.get("minecraft"));
    match candidates {
        Some(Value::String(text)) => vec![text.trim().to_owned()]
            .into_iter()
            .filter(|item| !item.is_empty())
            .collect(),
        Some(Value::Array(items)) => items.iter().map(text_value).filter(|item| !item.is_empty()).collect(),
        _ => Vec::new(),
    }
}

fn field_text(item: &Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        let text = text_value(item.get(*key).unwrap_or(&Value::Null));
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

fn field_text_or(item: &Map<String, Value>, keys: &[&str], fallback: &str) -> String {
    let value = field_text(item, keys);
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn safe_archive_name(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or_default().to_owned();
    if name.is_empty() || name == "." || name == ".." || name != normalized {
        return "modpack.zip".to_owned();
    }
    name.chars().take(160).collect()
}

/// принимает только SHA-1, SHA-256 и SHA-512 нужной длины в шестнадцатеричном виде
fn publisher_hashes(value: &Value) -> [Option<String>; 3] {
    let lengths = [("sha1", 40usize), ("sha256", 64), ("sha512", 128)];
    let mut hashes: [Option<String>; 3] = [None, None, None];
    for (position, (algorithm, length)) in lengths.iter().enumerate() {
        let digest = text_value(value.get(*algorithm).unwrap_or(&Value::Null)).to_lowercase();
        if digest.len() == *length && digest.chars().all(|character| character.is_ascii_hexdigit()) {
            hashes[position] = Some(digest);
        }
    }
    hashes
}

fn unverified_download_error(message: &str) -> String {
    format!("[unverified-download] {message}. Подтвердите скачивание явно.")
}

/// проверяет HTTPS и отсутствие учётных данных в URL архива
fn validate_archive_url(value: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value).map_err(|_| "URL архива модпака некорректен".to_owned())?;
    if parsed.scheme().to_lowercase() != "https"
        || parsed.host_str().unwrap_or_default().is_empty()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Архив модпака должен загружаться по HTTPS".to_owned());
    }
    Ok(())
}

/// файлы CurseForge загружаются только с официального CDN
fn validate_curseforge_download_url(value: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value).map_err(|_| "URL файла CurseForge некорректен".to_owned())?;
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    if parsed.scheme().to_lowercase() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Файл CurseForge должен загружаться по HTTPS".to_owned());
    }
    let suffix = format!(".{CURSEFORGE_DOWNLOAD_HOST_SUFFIX}");
    if !host.ends_with(&suffix) && host != CURSEFORGE_DOWNLOAD_HOST_SUFFIX {
        return Err("Файл CurseForge можно загружать только с официального CDN (forgecdn.net)".to_owned());
    }
    Ok(())
}

/// отклоняет абсолютные пути и компоненты `.` или `..`
fn archive_name(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    if normalized.contains('\0')
        || normalized.starts_with('/')
        || normalized.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Модпак содержит небезопасный путь".to_owned());
    }
    Ok(normalized.to_owned())
}

fn archive_prefix(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let mut first_parts: HashSet<&str> = HashSet::new();
    for value in names.iter().filter(|value| !value.is_empty()) {
        first_parts.insert(value.split('/').next().unwrap_or_default());
    }
    let prefix = if first_parts.len() == 1 {
        first_parts.into_iter().next().unwrap_or_default().to_owned()
    } else {
        String::new()
    };
    if !prefix.is_empty() {
        let with_slash = format!("{prefix}/");
        if names
            .iter()
            .any(|value| matches!(value.strip_prefix(&with_slash), Some("modrinth.index.json") | Some("manifest.json")))
        {
            return with_slash;
        }
    }
    String::new()
}

fn strip_modpack_prefix(value: &str) -> String {
    let Ok(normalized) = archive_name(value) else {
        return value.replace('\\', "/");
    };
    let mut parts: Vec<&str> = normalized.split('/').collect();
    while let Some(first) = parts.first() {
        if MODPACK_ROOT_PREFIXES.contains(&first.to_lowercase().as_str()) {
            parts.remove(0);
        } else {
            break;
        }
    }
    parts.join("/")
}

fn loader_managed_path(value: &str) -> bool {
    let stripped = strip_modpack_prefix(value);
    let first = stripped.split('/').next().unwrap_or_default();
    !stripped.is_empty() && LOADER_MANAGED_ROOTS.contains(&first)
}

fn archive_destination(value: &str) -> Result<String, String> {
    let normalized = archive_name(value)?;
    let mut parts: Vec<String> = normalized.split('/').map(str::to_owned).collect();
    if parts.iter().any(|part| part.eq_ignore_ascii_case("server-overrides")) {
        return Ok(String::new());
    }
    while let Some(first) = parts.first() {
        if MODPACK_ROOT_PREFIXES.contains(&first.to_lowercase().as_str()) {
            parts.remove(0);
        } else {
            break;
        }
    }
    if parts.is_empty() {
        return Ok(String::new());
    }
    if parts.iter().any(|part| part.starts_with(".trial-launcher")) {
        return Err("Модпак не может заменять служебные файлы лаунчера".to_owned());
    }
    if MODPACK_METADATA_FILES.contains(&parts[0].as_str()) {
        return Ok(String::new());
    }
    if LOADER_MANAGED_ROOTS.contains(&parts[0].as_str()) {
        // эти каталоги принадлежат лаунчеру и не должны заменяться из архива
        return Ok(String::new());
    }
    Ok(parts.join("/"))
}

fn is_server_override(value: &str) -> bool {
    match archive_name(value) {
        Ok(normalized) => normalized.split('/').any(|part| part.eq_ignore_ascii_case("server-overrides")),
        Err(_) => false,
    }
}

/// распознаёт символьные ссылки по Unix-режиму записи ZIP
fn zip_entry_is_symlink(mode: Option<u32>) -> bool {
    mode.map(|mode| mode & 0o170000 == 0o120000).unwrap_or(false)
}

fn reject_symlink_parents(root: &Path, path: &Path) -> Result<(), String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let mut current = path.to_owned();
    loop {
        if is_symlink(&current) {
            return Err(format!("Путь модпака является символьной ссылкой: {}", path.display()));
        }
        if current == canonical_root {
            return Ok(());
        }
        let Some(parent) = current.parent() else {
            return Err(format!("Путь модпака выходит за пределы экземпляра: {}", path.display()));
        };
        if parent == current {
            return Err(format!("Путь модпака выходит за пределы экземпляра: {}", path.display()));
        }
        current = parent.to_owned();
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path).map_err(|error| format!("Не удалось прочитать архив модпака: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Не удалось прочитать архив модпака: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn staging_directory(prefix: &str) -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join(format!("{prefix}{}", super::snapshots::random_hex(16)));
    fs::create_dir(&directory).map_err(|error| format!("Не удалось создать временный каталог: {error}"))?;
    Ok(directory)
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir(path).map_err(|error| format!("Не удалось создать каталог модпака: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn assert_modpack_not_installed(
    target: &Path,
    source: &str,
    project_id: &str,
    catalog_id: &str,
    archive_sha256: &str,
) -> Result<(), String> {
    if is_symlink(target) || !target.is_dir() {
        return Err("Папка экземпляра модпака недоступна".to_owned());
    }
    let marker = target.join(MODPACK_MARKER);
    if is_symlink(&marker) {
        return Err("Метаданные модпаков являются символьной ссылкой".to_owned());
    }
    if !marker.exists() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(&marker).map_err(|_| "Метаданные модпаков повреждены".to_owned())?;
    if !meta.is_file() || meta.len() > MAX_MODPACK_MARKER_BYTES {
        return Err("Метаданные модпаков повреждены или слишком велики".to_owned());
    }
    let raw = fs::read_to_string(&marker).map_err(|_| "Метаданные модпаков повреждены".to_owned())?;
    let records: Value = serde_json::from_str(&raw).map_err(|_| "Метаданные модпаков повреждены".to_owned())?;
    let Some(records) = records.as_array() else {
        return Err("Метаданные модпаков имеют неожиданный формат".to_owned());
    };
    let expected_catalog = if catalog_id.trim().to_lowercase().is_empty() {
        source.trim().to_lowercase()
    } else {
        catalog_id.trim().to_lowercase()
    };
    for item in records.iter().filter_map(Value::as_object) {
        if !archive_sha256.is_empty()
            && item.get("archive_sha256").and_then(Value::as_str).unwrap_or_default() == archive_sha256
        {
            return Err("Этот архив модпака уже импортирован в выбранный экземпляр".to_owned());
        }
        let stored_source = item.get("source").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase();
        let stored_catalog = item
            .get("catalog_id")
            .or_else(|| item.get("source"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if !project_id.is_empty()
            && item.get("project_id").and_then(Value::as_str).unwrap_or_default() == project_id
            && stored_source == source.trim().to_lowercase()
            && stored_catalog == expected_catalog
        {
            return Err("Этот модпак уже установлен в выбранный экземпляр".to_owned());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_modpack(
    target: &Path,
    source: &str,
    project_id: &str,
    name: &str,
    version: &str,
    files: &[String],
    catalog_id: &str,
    archive_sha256: &str,
) -> Result<(), String> {
    let marker = target.join(MODPACK_MARKER);
    if is_symlink(&marker) {
        return Err("Метаданные модпаков являются символьной ссылкой".to_owned());
    }
    let mut records: Vec<Map<String, Value>> = Vec::new();
    if marker.exists() {
        let meta = fs::symlink_metadata(&marker).map_err(|_| "Метаданные модпаков повреждены".to_owned())?;
        if meta.len() > MAX_MODPACK_MARKER_BYTES {
            return Err("Метаданные модпаков слишком велики".to_owned());
        }
        let raw = fs::read_to_string(&marker).map_err(|_| "Метаданные модпаков повреждены".to_owned())?;
        let current: Value = serde_json::from_str(&raw).map_err(|_| "Метаданные модпаков повреждены".to_owned())?;
        if let Some(items) = current.as_array() {
            records = items.iter().filter_map(Value::as_object).cloned().collect();
        }
    }
    let normalized_catalog_id = if catalog_id.trim().to_lowercase().is_empty() {
        source.trim().to_lowercase()
    } else {
        catalog_id.trim().to_lowercase()
    };
    records.retain(|item| {
        let same_archive = !archive_sha256.is_empty()
            && item.get("archive_sha256").and_then(Value::as_str).unwrap_or_default() == archive_sha256;
        let same_project = !project_id.is_empty()
            && item.get("project_id").and_then(Value::as_str).unwrap_or_default() == project_id
            && item.get("source").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase()
                == source.trim().to_lowercase()
            && item
                .get("catalog_id")
                .or_else(|| item.get("source"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_lowercase()
                == normalized_catalog_id;
        !(same_archive || same_project)
    });
    let mut record = Map::new();
    record.insert("source".to_owned(), json!(source));
    record.insert("catalog_id".to_owned(), json!(normalized_catalog_id));
    record.insert("project_id".to_owned(), json!(project_id));
    record.insert("name".to_owned(), json!(name.chars().take(160).collect::<String>()));
    record.insert("version".to_owned(), json!(version.chars().take(80).collect::<String>()));
    record.insert("archive_sha256".to_owned(), json!(archive_sha256));
    record.insert("file_count".to_owned(), json!(files.len().min(MAX_MODPACK_ENTRIES)));
    records.push(record);
    let trimmed: Vec<Value> = records.iter().rev().take(200).rev().cloned().map(Value::Object).collect();
    super::net::atomic_json_write(&marker, &Value::Array(trimmed), true)
}

// ---------------------------------------------------------- результат ----

#[derive(Clone, Default)]
pub(crate) struct ModpackResult {
    pub(crate) source: String,
    pub(crate) project_id: String,
    title: String,
    description: String,
    author: String,
    icon_url: String,
    downloads: i64,
    version: String,
    download_url: String,
    file_name: String,
    game_versions: Vec<String>,
    released_at: i64,
    updated_at: i64,
    manifest_files: Vec<Map<String, Value>>,
}

impl ModpackResult {
    #[allow(clippy::too_many_arguments)]
    fn new(
        source: &str,
        project_id: &str,
        title: String,
        description: String,
        author: String,
        icon_url: String,
        downloads: i64,
        version: String,
        download_url: String,
        file_name: String,
        game_versions: Vec<String>,
        released_at: i64,
        updated_at: i64,
    ) -> Self {
        Self {
            source: source.to_owned(),
            project_id: project_id.to_owned(),
            title,
            description,
            author,
            icon_url,
            downloads,
            version,
            download_url,
            file_name,
            game_versions,
            released_at,
            updated_at,
            manifest_files: Vec::new(),
        }
    }

    pub(crate) fn as_dict(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("source".to_owned(), json!(self.source));
        fields.insert("project_id".to_owned(), json!(self.project_id));
        fields.insert("title".to_owned(), json!(self.title));
        fields.insert("description".to_owned(), json!(self.description));
        fields.insert("author".to_owned(), json!(self.author));
        fields.insert("icon_url".to_owned(), json!(self.icon_url));
        fields.insert("downloads".to_owned(), json!(self.downloads));
        fields.insert("version".to_owned(), json!(self.version));
        fields.insert("download_url".to_owned(), json!(self.download_url));
        fields.insert("file_name".to_owned(), json!(self.file_name));
        fields.insert("game_versions".to_owned(), json!(self.game_versions));
        fields.insert("project_type".to_owned(), json!("modpack"));
        fields
    }
}

fn rows(value: &Value, keys: &[&str]) -> Vec<Map<String, Value>> {
    if let Some(items) = value.as_array() {
        return items.iter().filter_map(Value::as_object).cloned().collect();
    }
    let Some(fields) = value.as_object() else {
        return Vec::new();
    };
    for key in keys {
        if let Some(Value::Array(items)) = fields.get(*key) {
            return items.iter().filter_map(Value::as_object).cloned().collect();
        }
    }
    Vec::new()
}

fn find_archive(value: &Value) -> Map<String, Value> {
    let Some(fields) = value.as_object() else {
        return Map::new();
    };
    let direct_keys = ["download_url", "downloadUrl", "download", "archive_url", "archiveUrl", "url"];
    for key in direct_keys {
        if let Some(Value::String(candidate)) = fields.get(key) {
            let lowered = candidate.to_lowercase();
            if lowered.ends_with(".zip") || lowered.ends_with(".mrpack") {
                let mut found = Map::new();
                found.insert("url".to_owned(), json!(candidate));
                for name in ["sha1", "sha256", "sha512", "size"] {
                    if let Some(item) = fields.get(name) {
                        found.insert(name.to_owned(), item.clone());
                    }
                }
                return found;
            }
        }
    }
    for key in ["files", "downloads", "artifacts", "builds", "versions"] {
        match fields.get(key) {
            Some(Value::Array(items)) => {
                for row in items {
                    let found = find_archive(row);
                    if !found.is_empty() {
                        return found;
                    }
                }
            }
            Some(nested @ Value::Object(_)) => {
                let found = find_archive(nested);
                if !found.is_empty() {
                    return found;
                }
            }
            _ => {}
        }
    }
    Map::new()
}

fn pick_version(rows: &[Map<String, Value>], game_version: &str, version: &str) -> Map<String, Value> {
    if !version.is_empty() {
        for row in rows {
            if field_text(row, &["id", "version_id", "version"]) == version {
                return row.clone();
            }
        }
    }
    if !game_version.is_empty() {
        for row in rows {
            if game_versions(&Value::Object(row.clone())).iter().any(|item| item == game_version) {
                return row.clone();
            }
        }
    }
    rows.first().cloned().unwrap_or_default()
}

fn timestamp_value(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse::<i64>().ok()))
        .unwrap_or(0)
        .max(0)
}

fn ftb_game_versions(item: &Map<String, Value>) -> Vec<String> {
    let mut versions = game_versions(&Value::Object(item.clone()));
    if let Some(releases) = item.get("versions").and_then(Value::as_array) {
        for release in releases {
            let Some(targets) = release.get("targets").and_then(Value::as_array) else { continue };
            for target in targets {
                let platform = text_value(
                    target
                        .get("name")
                        .or_else(|| target.get("platform"))
                        .unwrap_or(&Value::Null),
                )
                .to_lowercase();
                let target_type = target.get("type").and_then(Value::as_str).unwrap_or_default().to_lowercase();
                if platform == "minecraft" || target_type == "game" {
                    let version = text_value(target.get("version").unwrap_or(&Value::Null));
                    if !version.is_empty() && !versions.contains(&version) {
                        versions.push(version);
                    }
                }
            }
        }
    }
    versions
}

fn technic_update_timestamp(item: &Map<String, Value>) -> i64 {
    item.get("feed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| timestamp_value(entry.get("date").unwrap_or(&Value::Null)))
        .max()
        .unwrap_or(0)
}

fn ftb_result(item: &Map<String, Value>) -> ModpackResult {
    let archive = find_archive(&Value::Object(item.clone()));
    ModpackResult::new(
        "ftb",
        &field_text_or(item, &["id", "project_id", "slug"], ""),
        field_text_or(item, &["name", "title", "displayName"], "FTB Modpack"),
        field_text(item, &["synopsis", "description", "summary"]),
        field_text(item, &["author", "authors"]),
        field_text(item, &["icon", "logo", "logoUrl", "icon_url"]),
        int_value(item
            .get("downloads")
            .or_else(|| item.get("downloadCount"))
            .or_else(|| item.get("installs"))
            .or_else(|| item.get("install_count"))
            .unwrap_or(&Value::Null)),
        field_text(item, &["version", "version_id", "latestVersion"]),
        field_text(&archive, &["url", "download_url"]),
        safe_archive_name(&field_text_or(&archive, &["file_name", "filename"], "ftb-modpack.zip")),
        ftb_game_versions(item),
        timestamp_value(item.get("released").unwrap_or(&Value::Null)),
        timestamp_value(item.get("updated").unwrap_or(&Value::Null)),
    )
}

fn technic_result(item: &Map<String, Value>) -> ModpackResult {
    let archive = find_archive(&Value::Object(item.clone()));
    ModpackResult::new(
        "technic",
        &field_text_or(item, &["slug", "id", "name"], ""),
        field_text_or(item, &["displayName", "display_name", "name"], "Technic Modpack"),
        field_text(item, &["description", "synopsis"]),
        field_text(item, &["author", "username", "user"]),
        field_text(item, &["iconUrl", "icon_url", "logoUrl", "icon", "logo"]),
        int_value(item
            .get("downloads")
            .or_else(|| item.get("downloadCount"))
            .or_else(|| item.get("installs"))
            .unwrap_or(&Value::Null)),
        field_text(item, &["version", "build", "recommendedBuild"]),
        field_text(&archive, &["url", "download_url"]),
        safe_archive_name(&field_text_or(&archive, &["file_name", "filename"], "technic-modpack.zip")),
        game_versions(&Value::Object(item.clone())),
        0,
        technic_update_timestamp(item),
    )
}

fn ftb_manifest_files(value: &Value) -> Vec<Map<String, Value>> {
    value
        .get("files")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_object).cloned().collect())
        .unwrap_or_default()
}

fn ftb_manifest_destination(entry: &Map<String, Value>) -> Result<String, String> {
    let path = text_value(entry.get("path").unwrap_or(&Value::Null))
        .replace('\\', "/");
    let path = path.trim().to_owned();
    let name = field_text(entry, &["name", "file_name", "filename"]).replace('\\', "/");
    let name = name.trim().to_owned();
    let mut combined = if !name.is_empty() && !path.is_empty() {
        format!("{}/{}", path.trim_end_matches('/'), name)
    } else {
        format!("{}{}", path, name)
    };
    while let Some(stripped) = combined.strip_prefix("./") {
        combined = stripped.to_owned();
    }
    if name.is_empty() && combined.ends_with('/') {
        return Ok(String::new());
    }
    archive_destination(&combined)
}

fn merge_objects(base: &Map<String, Value>, overlay: &Map<String, Value>) -> Map<String, Value> {
    let mut merged = base.clone();
    for (key, value) in overlay {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

// ------------------------------------------------------------- каталог ----

fn modpack_sort_mode(sort_by: &str) -> Result<&'static str, String> {
    match sort_by.trim().to_lowercase().as_str() {
        "" | "relevance" => Ok("relevance"),
        "downloads" => Ok("downloads"),
        "newest" => Ok("newest"),
        "updated" => Ok("updated"),
        other => Err(format!("Неизвестный порядок сортировки модпаков: {other}")),
    }
}

fn modrinth_sort_index(sort_by: &str) -> Result<&'static str, String> {
    modpack_sort_mode(sort_by)
        .map_err(|error| error.replace("модпаков", "Modrinth"))
}

fn validate_provider_sort(kind: &str, sort_by: &str) -> Result<(), String> {
    if kind == "technic" && sort_by == "newest" {
        return Err("Каталог Technic не предоставляет дату создания модпака для сортировки по новизне".to_owned());
    }
    Ok(())
}

fn sort_modpack_results(results: &mut [ModpackResult], sort_by: &str) {
    match sort_by {
        "downloads" => results.sort_by(|left, right| right.downloads.cmp(&left.downloads)),
        "newest" => results.sort_by(|left, right| right.released_at.cmp(&left.released_at)),
        "updated" => results.sort_by(|left, right| right.updated_at.cmp(&left.updated_at)),
        _ => {}
    }
}

fn ftb_search_url(base_url: &str, query: &str, limit: i64) -> String {
    format!(
        "{}/modpack/search/{}?{}",
        base_url.trim_end_matches('/'),
        limit.max(1),
        super::mods::form_urlencode(&[("term", query.trim().to_owned())]),
    )
}

fn ftb_listing_url(base_url: &str, sort_by: &str, limit: i64) -> Result<String, String> {
    let route = match sort_by {
        "downloads" => "modpack/popular/installs",
        "updated" => "modpack/updated",
        "relevance" => "modpack/featured",
        "newest" => return Err("FTB API не предоставляет полный каталог, отсортированный по дате создания".to_owned()),
        other => return Err(format!("Неизвестный порядок сортировки модпаков FTB: {other}")),
    };
    Ok(format!("{}/{route}/{}", base_url.trim_end_matches('/'), limit.max(1)))
}

fn ftb_pack_ids(value: &Value) -> Vec<String> {
    value
        .get("packs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(text_value)
        .filter(|id| !id.is_empty())
        .collect()
}

fn ftb_search_total(value: &Value, requested_count: usize) -> Option<i64> {
    let count = ftb_pack_ids(value).len();
    (count < requested_count).then_some(count as i64)
}

fn page_slice<T>(items: Vec<T>, offset: i64, limit: i64) -> Vec<T> {
    let start = usize::try_from(offset.max(0)).unwrap_or(usize::MAX);
    let count = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    items.into_iter().skip(start).take(count).collect()
}

pub(crate) struct ModpackProvider {
    http: Arc<HttpClient>,
    kind: String,
    catalog_id: String,
    base_url: String,
    last_total: Cell<Option<i64>>,
}

impl ModpackProvider {
    pub(crate) fn new(http: Arc<HttpClient>, catalog: &Value) -> Result<Self, String> {
        let kind = catalog
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("modrinth")
            .trim()
            .to_lowercase();
        let catalog_id = {
            let value = catalog.get("id").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
            if value.is_empty() {
                kind.clone()
            } else {
                value
            }
        };
        let base_url = catalog
            .get("api_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_owned();
        if !matches!(kind.as_str(), "modrinth" | "ftb" | "technic") {
            return Err(format!("Каталог {kind} не поддерживает модпаки"));
        }
        Ok(Self {
            http,
            kind,
            catalog_id,
            base_url,
            last_total: Cell::new(None),
        })
    }

    pub(crate) fn search(
        &self,
        query: &str,
        game_version: &str,
        limit: i64,
        offset: i64,
        sort_by: &str,
    ) -> Result<Vec<ModpackResult>, String> {
        let limit = limit.clamp(1, 30);
        let offset = offset.clamp(0, 10_000);
        let sort_by = modpack_sort_mode(sort_by)?;
        validate_provider_sort(&self.kind, sort_by)?;
        self.last_total.set(None);
        match self.kind.as_str() {
            "modrinth" => self.search_modrinth(query, game_version, limit, offset, sort_by),
            "ftb" => {
                let results = self.search_ftb(query, game_version, offset + limit, sort_by)?;
                Ok(page_slice(results, offset, limit))
            }
            "technic" => {
                let mut results = self.search_technic(query, game_version)?;
                sort_modpack_results(&mut results, sort_by);
                self.last_total.set(Some(results.len() as i64));
                Ok(page_slice(results, offset, limit))
            }
            _ => unreachable!("catalog kind is validated by ModpackProvider::new"),
        }
    }

    pub(crate) fn last_total(&self) -> Option<i64> {
        self.last_total.get()
    }

    pub(crate) fn project_details(&self, project_id: &str) -> Result<Value, String> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err("Не указан ID модпака".to_owned());
        }
        let encoded = super::mods::percent_encode(project_id);
        match self.kind.as_str() {
            "modrinth" => {
                let value = self.http.get_json(&format!("{}/project/{encoded}", self.base_url), None)?;
                if !value.is_object()
                    || value.get("project_type").and_then(Value::as_str).unwrap_or_default() != "modpack"
                {
                    return Err("Каталог не вернул подробности модпака".to_owned());
                }
                Ok(value)
            }
            "ftb" => {
                let value = self.http.get_json(&format!("{}/modpack/{encoded}", self.base_url), None)?;
                if !value.is_object() {
                    return Err("Каталог FTB не вернул подробности модпака".to_owned());
                }
                let fields = value.as_object().cloned().unwrap_or_default();
                Ok(Value::Object(merge_objects(&fields, &ftb_result(&fields).as_dict())))
            }
            _ => {
                let query = super::mods::form_urlencode(&[("build", "multimc".to_owned())]);
                let value = self
                    .http
                    .get_json(&format!("{}/modpack/{encoded}?{query}", self.base_url), None)?;
                if !value.is_object() {
                    return Err("Каталог Technic не вернул подробности модпака".to_owned());
                }
                let fields = value.as_object().cloned().unwrap_or_default();
                Ok(Value::Object(merge_objects(&fields, &technic_result(&fields).as_dict())))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install(
        &self,
        project_id: &str,
        target: &Path,
        game_version: &str,
        version: &str,
        display_name: &str,
        loader: &str,
        loader_version: Option<&str>,
        confirm_unverified: bool,
    ) -> Result<Map<String, Value>, String> {
        let project_id = project_id.trim();
        assert_modpack_not_installed(target, &self.kind, project_id, &self.catalog_id, "")?;
        let (result, hashes, size) = self.resolve_archive(project_id, game_version, version)?;
        let publisher = publisher_hashes(&Value::Object(hashes));
        if self.kind == "ftb" && !result.manifest_files.is_empty() {
            let display = if display_name.trim().is_empty() {
                result.title.clone()
            } else {
                display_name.trim().to_owned()
            };
            return ModpackImporter::new(self.http.clone()).import_manifest(
                &result.manifest_files,
                target,
                &self.kind,
                &result.project_id,
                &display,
                &result.version,
                &self.catalog_id,
                confirm_unverified,
            );
        }
        if result.download_url.is_empty() {
            return Err("Каталог не предоставил архив модпака для скачивания".to_owned());
        }
        if let Some(size) = size {
            if size > MAX_MODPACK_BYTES {
                return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
            }
        }
        if publisher.iter().all(Option::is_none) && !confirm_unverified {
            return Err(unverified_download_error(
                "Каталог модпаков не предоставил доверенный хеш архива",
            ));
        }
        validate_archive_url(&result.download_url)?;
        let directory = staging_directory("trial-launcher-modpack-")?;
        let outcome = (|| -> Result<Map<String, Value>, String> {
            let archive_name = if result.file_name.is_empty() {
                "modpack.zip".to_owned()
            } else {
                result.file_name.clone()
            };
            let archive_path = directory.join(archive_name);
            self.http.download(
                &result.download_url,
                &archive_path,
                publisher[0].as_deref(),
                publisher[1].as_deref(),
                publisher[2].as_deref(),
                size,
                None,
                None,
                None,
            )?;
            let display = if display_name.trim().is_empty() {
                result.title.clone()
            } else {
                display_name.trim().to_owned()
            };
            let mut imported = ModpackImporter::new(self.http.clone()).import_file(
                &archive_path,
                target,
                &self.kind,
                &result.project_id,
                &display,
                &result.version,
                &self.catalog_id,
                confirm_unverified,
                game_version,
                loader,
                loader_version,
            )?;
            let trusted_before = imported.get("trusted_hash").and_then(Value::as_bool).unwrap_or(false);
            imported.insert(
                "trusted_hash".to_owned(),
                json!(publisher.iter().any(Option::is_some) && trusted_before),
            );
            Ok(imported)
        })();
        let _ = fs::remove_dir_all(&directory);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn preflight(
        &self,
        project_id: &str,
        game_version: &str,
        version: &str,
        loader: &str,
        loader_version: Option<&str>,
        confirm_unverified: bool,
    ) -> Result<Value, String> {
        let (result, hashes, size) = self.resolve_archive(project_id, game_version, version)?;
        if self.kind == "ftb" && !result.manifest_files.is_empty() {
            let importer = ModpackImporter::new(self.http.clone());
            importer.preflight_manifest(&result.manifest_files, confirm_unverified)?;
            let plan = importer.manifest_download_plan(&result.manifest_files, true)?;
            let mods: Vec<Value> = plan
                .iter()
                .map(|item| {
                    let name = if item.preview_name.is_empty() {
                        Path::new(&item.destination)
                            .file_name()
                            .map(|value| value.to_string_lossy().to_string())
                            .unwrap_or_default()
                    } else {
                        item.preview_name.clone()
                    };
                    json!({ "name": name, "source": "ftb", "optional": false })
                })
                .collect();
            return Ok(json!({
                "minecraft": "",
                "loader": "",
                "loader_version": "",
                "compatible": true,
                "issues": [],
                "mods": {
                    "total": mods.len(),
                    "mods": mods.iter().take(MOD_PREVIEW_LIMIT).cloned().collect::<Vec<_>>(),
                    "truncated": mods.len() > MOD_PREVIEW_LIMIT,
                },
            }));
        }
        if result.download_url.is_empty() {
            return Err("Каталог не предоставил архив модпака для скачивания".to_owned());
        }
        if let Some(size) = size {
            if size > MAX_MODPACK_BYTES {
                return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
            }
        }
        let publisher = publisher_hashes(&Value::Object(hashes));
        if publisher.iter().all(Option::is_none) && !confirm_unverified {
            return Err(unverified_download_error(
                "Каталог модпаков не предоставил доверенный хеш архива",
            ));
        }
        validate_archive_url(&result.download_url)?;
        let is_mrpack = result.file_name.to_lowercase().ends_with(".mrpack")
            || url::Url::parse(&result.download_url)
                .map(|parsed| parsed.path().to_lowercase().ends_with(".mrpack"))
                .unwrap_or(false);
        if !is_mrpack {
            return Ok(json!({}));
        }
        let directory = staging_directory("trial-launcher-modpack-preflight-")?;
        let outcome = (|| -> Result<Value, String> {
            let archive_name = if result.file_name.is_empty() {
                "modpack.mrpack".to_owned()
            } else {
                result.file_name.clone()
            };
            let archive_path = directory.join(archive_name);
            self.http.download(
                &result.download_url,
                &archive_path,
                publisher[0].as_deref(),
                publisher[1].as_deref(),
                publisher[2].as_deref(),
                size,
                None,
                None,
                None,
            )?;
            let importer = ModpackImporter::new(self.http.clone());
            let mut dependencies = importer.preflight_file(
                &archive_path,
                confirm_unverified,
                game_version,
                loader,
                loader_version,
                false,
            )?;
            if let Ok(preview) = importer.mod_preview_file(&archive_path) {
                dependencies.insert("mods".to_owned(), Value::Object(preview));
            }
            Ok(Value::Object(dependencies))
        })();
        let _ = fs::remove_dir_all(&directory);
        outcome
    }

    fn search_modrinth(
        &self,
        query: &str,
        game_version: &str,
        limit: i64,
        offset: i64,
        sort_by: &str,
    ) -> Result<Vec<ModpackResult>, String> {
        let mut facets: Vec<Vec<String>> = vec![vec!["project_type:modpack".to_owned()]];
        if !game_version.trim().is_empty() {
            facets.push(vec![format!("versions:{}", game_version.trim())]);
        }
        let sort_index = modrinth_sort_index(sort_by)?;
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.to_string()),
            ("index", sort_index.to_owned()),
            ("facets", serde_json::to_string(&facets).map_err(|error| error.to_string())?),
        ];
        if offset > 0 {
            params.push(("offset", offset.to_string()));
        }
        let trimmed = query.trim();
        if !trimmed.is_empty() {
            params.push(("query", trimmed.to_owned()));
        }
        let url = format!("{}/search?{}", self.base_url, super::mods::form_urlencode(&params));
        let value = self.http.get_json(&url, None)?;
        let hits = value.get("hits").and_then(Value::as_array).cloned().unwrap_or_default();
        let total = value.get("total_hits").and_then(Value::as_i64);
        self.last_total.set(total.filter(|value| *value >= 0));
        Ok(hits
            .iter()
            .filter_map(Value::as_object)
            .map(|item| {
                ModpackResult::new(
                    "modrinth",
                    &field_text_or(item, &["project_id", "id", "slug"], ""),
                    field_text_or(item, &["title", "name"], "Modpack"),
                    field_text(item, &["description"]),
                    field_text(item, &["author"]),
                    field_text(item, &["icon_url", "icon"]),
                    int_value(item.get("downloads").unwrap_or(&Value::Null)),
                    String::new(),
                    String::new(),
                    String::new(),
                    game_versions(&Value::Object(item.clone())),
                    0,
                    0,
                )
            })
            .collect())
    }

    fn search_ftb(
        &self,
        query: &str,
        game_version: &str,
        fetch_limit: i64,
        sort_by: &str,
    ) -> Result<Vec<ModpackResult>, String> {
        // смещение не поддерживается API FTB, поэтому запрашиваем начало списка с запасом
        let query = query.trim();
        let target_count = fetch_limit.max(1) as usize;
        if target_count > FTB_MAX_SEARCH_RESULTS {
            return Err("FTB pagination is limited to the first 500 results because its API has no offset".to_owned());
        }
        let mut candidate_limit = target_count.max(30).min(FTB_MAX_SEARCH_RESULTS);
        let mut seen = HashSet::new();
        let mut results: Vec<ModpackResult> = Vec::new();
        let mut exhausted = false;

        loop {
            let value = if query.is_empty() {
                let url = ftb_listing_url(&self.base_url, sort_by, candidate_limit as i64)?;
                self.http.get_json(&url, None)?
            } else {
                let url = ftb_search_url(&self.base_url, query, candidate_limit as i64);
                self.http.get_json(&url, None)?
            };
            let ids = ftb_pack_ids(&value);
            // API FTB не имеет offset: если сервер отдал меньше страницы, дальше искать нечего
            if ftb_search_total(&value, candidate_limit).is_some() {
                exhausted = true;
            }
            for id in ids {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let encoded = super::mods::percent_encode(&id);
                let detail = self
                    .http
                    .get_json(&format!("{}/modpack/{encoded}", self.base_url), None)?;
                let Some(fields) = detail.as_object() else { continue };
                let result = ftb_result(fields);
                if !game_version.is_empty()
                    && !result.game_versions.is_empty()
                    && !result.game_versions.iter().any(|value| value == game_version)
                {
                    continue;
                }
                results.push(result);
            }

            let needs_full_set = !query.is_empty() && sort_by != "relevance";
            if exhausted || (!needs_full_set && results.len() >= target_count) {
                break;
            }
            if candidate_limit >= FTB_MAX_SEARCH_RESULTS {
                if needs_full_set {
                    return Err("Слишком много результатов FTB для точной сортировки по запросу".to_owned());
                }
                break;
            }
            candidate_limit = candidate_limit.saturating_mul(2).min(FTB_MAX_SEARCH_RESULTS);
        }

        self.last_total.set(if exhausted { Some(results.len() as i64) } else { None });
        if !query.is_empty() {
            sort_modpack_results(&mut results, sort_by);
        }
        Ok(results)
    }

    fn search_technic(&self, query: &str, game_version: &str) -> Result<Vec<ModpackResult>, String> {
        let needle = query.trim();
        let params = super::mods::form_urlencode(&[
            ("build", "multimc".to_owned()),
            ("q", if needle.is_empty() { "modpack".to_owned() } else { needle.to_owned() }),
        ]);
        let value = self.http.get_json(&format!("{}/search?{params}", self.base_url), None)?;
        let items = rows(&value, &["modpacks", "results", "packs"]);
        let detail_query = super::mods::form_urlencode(&[("build", "multimc".to_owned())]);
        let mut results: Vec<ModpackResult> = Vec::new();
        for search_item in items {
            let slug = field_text(&search_item, &["slug", "id", "name"]);
            if slug.is_empty() {
                continue;
            }
            let encoded = super::mods::percent_encode(&slug);
            let detail = self.http.get_json(
                &format!("{}/modpack/{encoded}?{detail_query}", self.base_url),
                None,
            )?;
            let merged = detail
                .as_object()
                .map(|fields| merge_objects(&search_item, fields))
                .unwrap_or(search_item);
            let result = technic_result(&merged);
            if !game_version.is_empty()
                && !result.game_versions.is_empty()
                && !result.game_versions.iter().any(|value| value == game_version)
            {
                continue;
            }
            results.push(result);
        }
        Ok(results)
    }

    fn resolve_archive(
        &self,
        project_id: &str,
        game_version: &str,
        version: &str,
    ) -> Result<(ModpackResult, Map<String, Value>, Option<u64>), String> {
        match self.kind.as_str() {
            "modrinth" => self.resolve_modrinth(project_id, game_version, version),
            "ftb" => self.resolve_ftb(project_id, game_version, version),
            _ => self.resolve_technic(project_id, game_version, version),
        }
    }

    fn resolve_modrinth(
        &self,
        project_id: &str,
        game_version: &str,
        version: &str,
    ) -> Result<(ModpackResult, Map<String, Value>, Option<u64>), String> {
        let encoded = super::mods::percent_encode(project_id.trim());
        let mut params: Vec<(&str, String)> = Vec::new();
        if !game_version.trim().is_empty() {
            params.push((
                "game_versions",
                serde_json::to_string(&vec![game_version.trim()]).map_err(|error| error.to_string())?,
            ));
        }
        let endpoint = format!("{}/project/{encoded}/version", self.base_url);
        let url = if params.is_empty() {
            endpoint
        } else {
            format!("{endpoint}?{}", super::mods::form_urlencode(&params))
        };
        let value = self.http.get_json(&url, None)?;
        let versions = value.as_array().cloned().unwrap_or_default();
        let selected = if !version.is_empty() {
            versions
                .iter()
                .find(|item| item.get("version_number").and_then(Value::as_str).unwrap_or_default() == version)
                .cloned()
                .or_else(|| versions.first().cloned())
                .unwrap_or_default()
        } else {
            versions.first().cloned().unwrap_or_default()
        };
        let files = selected.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
        let primary = files
            .iter()
            .find(|item| item.get("primary").and_then(Value::as_bool).unwrap_or(false))
            .or_else(|| files.first())
            .cloned()
            .unwrap_or_default();
        let hashes = primary
            .get("hashes")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let metadata = self
            .http
            .get_json(&format!("{}/project/{encoded}", self.base_url), None)?;
        let metadata = if metadata.is_object() { metadata } else { json!({}) };
        let game_versions: Vec<String> = selected
            .get("game_versions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let file_name = {
            let name = text_value(primary.get("filename").unwrap_or(&Value::Null));
            if name.is_empty() {
                format!("{}.mrpack", project_id.trim())
            } else {
                name
            }
        };
        let result = ModpackResult::new(
            "modrinth",
            project_id.trim(),
            {
                let title = text_value(
                    metadata
                        .get("title")
                        .or_else(|| metadata.get("name"))
                        .unwrap_or(&Value::Null),
                );
                if title.is_empty() {
                    project_id.trim().to_owned()
                } else {
                    title
                }
            },
            text_value(metadata.get("description").unwrap_or(&Value::Null)),
            text_value(metadata.get("author").unwrap_or(&Value::Null)),
            text_value(metadata.get("icon_url").or_else(|| metadata.get("icon")).unwrap_or(&Value::Null)),
            int_value(metadata.get("downloads").unwrap_or(&Value::Null)),
            text_value(selected.get("version_number").unwrap_or(&Value::Null)),
            text_value(primary.get("url").unwrap_or(&Value::Null)),
            safe_archive_name(&file_name),
            game_versions,
            0,
            0,
        );
        let mut hash_map = Map::new();
        for key in ["sha1", "sha256", "sha512"] {
            hash_map.insert(key.to_owned(), json!(text_value(hashes.get(key).unwrap_or(&Value::Null))));
        }
        Ok((result, hash_map, optional_int(primary.get("size").unwrap_or(&Value::Null))))
    }

    fn resolve_ftb(
        &self,
        project_id: &str,
        game_version: &str,
        version: &str,
    ) -> Result<(ModpackResult, Map<String, Value>, Option<u64>), String> {
        let encoded = super::mods::percent_encode(project_id.trim());
        let value = self
            .http
            .get_json(&format!("{}/modpack/{encoded}", self.base_url), None)?;
        let item = value.as_object().cloned().unwrap_or_default();
        let versions = rows(&Value::Object(item.clone()), &["versions", "releases", "builds"]);
        let selected = pick_version(&versions, game_version, version);
        let mut version_id = field_text(&selected, &["id", "version_id", "version"]);
        if version_id.is_empty() {
            version_id = text_value(item.get("version_id").unwrap_or(&Value::Null));
        }
        let mut detail = item.clone();
        if !version_id.is_empty() {
            let detail_value = self.http.get_json(
                &format!(
                    "{}/modpack/{encoded}/{}",
                    self.base_url,
                    super::mods::percent_encode(&version_id)
                ),
                None,
            )?;
            if let Some(fields) = detail_value.as_object() {
                detail = fields.clone();
            }
        }
        let archive = find_archive(&Value::Object(detail.clone()));
        let mut merged = merge_objects(&item, &detail);
        merged.insert("id".to_owned(), json!(project_id.trim()));
        merged.insert("version".to_owned(), json!(version_id));
        for (key, value) in archive.iter() {
            merged.insert(key.clone(), value.clone());
        }
        let mut result = ftb_result(&merged);
        result.manifest_files = ftb_manifest_files(&Value::Object(detail));
        let mut hash_map = Map::new();
        for key in ["sha1", "sha256", "sha512"] {
            hash_map.insert(key.to_owned(), json!(text_value(archive.get(key).unwrap_or(&Value::Null))));
        }
        Ok((result, hash_map, optional_int(archive.get("size").unwrap_or(&Value::Null))))
    }

    fn resolve_technic(
        &self,
        project_id: &str,
        game_version: &str,
        version: &str,
    ) -> Result<(ModpackResult, Map<String, Value>, Option<u64>), String> {
        let encoded = super::mods::percent_encode(project_id.trim());
        let query = super::mods::form_urlencode(&[("build", "multimc".to_owned())]);
        let value = self
            .http
            .get_json(&format!("{}/modpack/{encoded}?{query}", self.base_url), None)?;
        let item = value.as_object().cloned().unwrap_or_default();
        let builds = rows(&Value::Object(item.clone()), &["builds", "versions", "releases"]);
        let selected = pick_version(&builds, game_version, version);
        let detail = merge_objects(&item, &selected);
        let archive = find_archive(&Value::Object(detail.clone()));
        let mut merged = detail;
        merged.insert("slug".to_owned(), json!(project_id.trim()));
        for (key, value) in archive.iter() {
            merged.insert(key.clone(), value.clone());
        }
        let result = technic_result(&merged);
        let mut hash_map = Map::new();
        for key in ["sha1", "sha256", "sha512"] {
            hash_map.insert(key.to_owned(), json!(text_value(archive.get(key).unwrap_or(&Value::Null))));
        }
        Ok((result, hash_map, optional_int(archive.get("size").unwrap_or(&Value::Null))))
    }
}

// импорт архивов модпаков

#[derive(Clone)]
struct StagedFile {
    destination: String,
    url: String,
    hashes: [Option<String>; 3],
    size: Option<u64>,
    trusted: bool,
    preview_name: String,
    optional: bool,
}

pub(crate) struct ModpackImporter {
    http: Arc<HttpClient>,
    /// кеш запросов api.cfwidget.com (projectID/fileID → CDN-ссылка)
    cfwidget_cache: RefCell<HashMap<String, Option<(String, String, Option<u64>)>>>,
}

impl ModpackImporter {
    pub(crate) fn new(http: Arc<HttpClient>) -> Self {
        Self {
            http,
            cfwidget_cache: RefCell::new(HashMap::new()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import_bytes(
        &self,
        content: &[u8],
        target: &Path,
        file_name: &str,
        source: &str,
        project_id: &str,
        catalog_id: &str,
        display_name: &str,
        version: &str,
        confirm_unverified: bool,
        expected_game_version: &str,
        expected_loader: &str,
        expected_loader_version: Option<&str>,
    ) -> Result<Map<String, Value>, String> {
        if content.len() as u64 > MAX_MODPACK_BYTES {
            return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
        }
        let safe_name = safe_archive_name(file_name);
        let directory = staging_directory("trial-launcher-modpack-import-")?;
        let outcome = (|| -> Result<Map<String, Value>, String> {
            let archive_path = directory.join(&safe_name);
            fs::write(&archive_path, content)
                .map_err(|error| format!("Не удалось сохранить архив модпака: {error}"))?;
            let stem = Path::new(&safe_name)
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| safe_name.clone());
            let display = if display_name.trim().is_empty() {
                stem
            } else {
                display_name.trim().to_owned()
            };
            self.import_file(
                &archive_path,
                target,
                source,
                project_id,
                &display,
                version,
                catalog_id,
                confirm_unverified,
                expected_game_version,
                expected_loader,
                expected_loader_version,
            )
        })();
        let _ = fs::remove_dir_all(&directory);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn preflight_bytes(
        &self,
        content: &[u8],
        file_name: &str,
        confirm_unverified: bool,
        expected_game_version: &str,
        expected_loader: &str,
        expected_loader_version: Option<&str>,
    ) -> Result<Map<String, Value>, String> {
        if content.len() as u64 > MAX_MODPACK_BYTES {
            return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
        }
        let safe_name = safe_archive_name(file_name);
        let directory = staging_directory("trial-launcher-modpack-preflight-")?;
        let outcome = (|| -> Result<Map<String, Value>, String> {
            let archive_path = directory.join(&safe_name);
            fs::write(&archive_path, content)
                .map_err(|error| format!("Не удалось сохранить архив модпака: {error}"))?;
            self.preflight_file(
                &archive_path,
                confirm_unverified,
                expected_game_version,
                expected_loader,
                expected_loader_version,
                true,
            )
        })();
        let _ = fs::remove_dir_all(&directory);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn preflight_file(
        &self,
        archive_path: &Path,
        confirm_unverified: bool,
        expected_game_version: &str,
        expected_loader: &str,
        expected_loader_version: Option<&str>,
        strict_compatibility: bool,
    ) -> Result<Map<String, Value>, String> {
        if is_symlink(archive_path) || !archive_path.is_file() {
            return Err("Архив модпака не найден".to_owned());
        }
        if fs::metadata(archive_path).map(|meta| meta.len()).unwrap_or(0) > MAX_MODPACK_BYTES {
            return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
        }
        let mut archive = open_zip(archive_path)?;
        let plan = self.archive_plan(&mut archive)?;
        if !plan.manifest.is_empty() {
            let value = validate_curseforge_compatibility(
                &plan.manifest,
                expected_game_version,
                expected_loader,
                expected_loader_version,
                strict_compatibility,
            )?;
            self.curseforge_download_plan(&plan.manifest, confirm_unverified, true)?;
            Ok(value)
        } else {
            let value = validate_mrpack_compatibility(
                &plan.index,
                expected_game_version,
                expected_loader,
                expected_loader_version,
                strict_compatibility,
            )?;
            self.modrinth_download_plan(&plan.index, confirm_unverified)?;
            Ok(value)
        }
    }

    /// сводка модов архива для предпросмотра
    pub(crate) fn mod_preview_file(&self, archive_path: &Path) -> Result<Map<String, Value>, String> {
        if is_symlink(archive_path) || !archive_path.is_file() {
            return Err("Архив модпака не найден".to_owned());
        }
        if fs::metadata(archive_path).map(|meta| meta.len()).unwrap_or(0) > MAX_MODPACK_BYTES {
            return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
        }
        let mut archive = open_zip(archive_path)?;
        let plan = self.archive_plan(&mut archive)?;
        let mut mods: Vec<Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        fn add(mods: &mut Vec<Value>, seen: &mut HashSet<String>, name: &str, source: &str, optional: bool) {
            let clean = name.trim();
            if clean.is_empty() || seen.contains(&clean.to_lowercase()) {
                return;
            }
            seen.insert(clean.to_lowercase());
            mods.push(json!({
                "name": clean.chars().take(160).collect::<String>(),
                "source": source,
                "optional": optional,
            }));
        }

        if let Some(entries) = plan.index.get("files").and_then(Value::as_array) {
            for entry in entries.iter().filter_map(Value::as_object) {
                let path = text_value(entry.get("path").unwrap_or(&Value::Null)).replace('\\', "/");
                let lowered = path.to_lowercase();
                if lowered.starts_with("mods/") && lowered.ends_with(".jar") {
                    add(&mut mods, &mut seen, path.rsplit('/').next().unwrap_or_default(), "modrinth", false);
                }
            }
        }
        for destination in plan.selected.iter().map(|item| item.1.as_str()) {
            let lowered = destination.to_lowercase();
            if lowered.starts_with("mods/") && lowered.ends_with(".jar") {
                add(&mut mods, &mut seen, destination.rsplit('/').next().unwrap_or_default(), "archive", false);
            }
        }
        if !plan.manifest.is_empty() {
            if let Ok(items) = self.curseforge_download_plan(&plan.manifest, true, true) {
                for item in items {
                    let name = if item.preview_name.is_empty() {
                        item.destination.clone()
                    } else {
                        item.preview_name.clone()
                    };
                    add(&mut mods, &mut seen, &name, "curseforge", item.optional);
                }
            }
        }
        let mut result = Map::new();
        result.insert("total".to_owned(), json!(mods.len()));
        result.insert(
            "mods".to_owned(),
            Value::Array(mods.iter().take(MOD_PREVIEW_LIMIT).cloned().collect::<Vec<_>>()),
        );
        result.insert("truncated".to_owned(), json!(mods.len() > MOD_PREVIEW_LIMIT));
        Ok(result)
    }

    pub(crate) fn preflight_manifest(
        &self,
        entries: &[Map<String, Value>],
        confirm_unverified: bool,
    ) -> Result<(), String> {
        self.manifest_download_plan(entries, confirm_unverified)?;
        Ok(())
    }

    pub(crate) fn mod_preview_bytes(&self, content: &[u8], file_name: &str) -> Result<Map<String, Value>, String> {
        if content.len() as u64 > MAX_MODPACK_BYTES {
            return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
        }
        let safe_name = safe_archive_name(file_name);
        let directory = staging_directory("trial-launcher-modpack-preview-")?;
        let outcome = (|| -> Result<Map<String, Value>, String> {
            let archive_path = directory.join(&safe_name);
            fs::write(&archive_path, content)
                .map_err(|error| format!("Не удалось сохранить архив модпака: {error}"))?;
            self.mod_preview_file(&archive_path)
        })();
        let _ = fs::remove_dir_all(&directory);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import_file(
        &self,
        archive_path: &Path,
        target: &Path,
        source: &str,
        project_id: &str,
        display_name: &str,
        version: &str,
        catalog_id: &str,
        confirm_unverified: bool,
        expected_game_version: &str,
        expected_loader: &str,
        expected_loader_version: Option<&str>,
    ) -> Result<Map<String, Value>, String> {
        if is_symlink(archive_path) || !archive_path.is_file() {
            return Err("Архив модпака не найден".to_owned());
        }
        if fs::metadata(archive_path).map(|meta| meta.len()).unwrap_or(0) > MAX_MODPACK_BYTES {
            return Err("Архив модпака слишком большой (максимум 512 MiB)".to_owned());
        }
        if is_symlink(target) || !target.is_dir() {
            return Err("Папка экземпляра модпака недоступна".to_owned());
        }
        let archive_sha256 = sha256_file(archive_path)?;
        let assert_sha = if source == "local" || project_id.is_empty() {
            archive_sha256.as_str()
        } else {
            ""
        };
        assert_modpack_not_installed(target, source, project_id, catalog_id, assert_sha)?;
        self.preflight_file(
            archive_path,
            confirm_unverified,
            expected_game_version,
            expected_loader,
            expected_loader_version,
            true,
        )?;
        let stage = target.join(format!(".trial-launcher-modpack-{}.part", super::snapshots::random_hex(16)));
        if stage.symlink_metadata().is_ok() {
            return Err("Временная папка модпака уже существует".to_owned());
        }
        create_private_dir(&stage)?;
        let outcome = (|| -> Result<Map<String, Value>, String> {
            let staged = self.stage_archive(
                archive_path,
                &stage,
                confirm_unverified,
                expected_game_version,
                expected_loader,
                expected_loader_version,
            )?;
            let written = merge_stage(
                &stage,
                target,
                source,
                project_id,
                display_name,
                version,
                catalog_id,
                &archive_sha256,
            )?;
            let short_name = {
                let value: String = display_name.chars().take(160).collect();
                if value.is_empty() {
                    "Modpack".to_owned()
                } else {
                    value
                }
            };
            let mut result = Map::new();
            result.insert("source".to_owned(), json!(source));
            result.insert("project_id".to_owned(), json!(project_id));
            result.insert("name".to_owned(), json!(short_name));
            result.insert("version".to_owned(), json!(version.chars().take(80).collect::<String>()));
            result.insert(
                "file_name".to_owned(),
                json!(archive_path.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default()),
            );
            result.insert("path".to_owned(), json!(target.to_string_lossy()));
            result.insert("files".to_owned(), json!(written));
            result.insert("trusted_hash".to_owned(), json!(staged.trusted_hash));
            result.insert("skipped_loader_files".to_owned(), json!(staged.skipped_loader_files));
            result.insert("marker".to_owned(), json!(target.join(MODPACK_MARKER).to_string_lossy()));
            Ok(result)
        })();
        let _ = fs::remove_dir_all(&stage);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import_manifest(
        &self,
        entries: &[Map<String, Value>],
        target: &Path,
        source: &str,
        project_id: &str,
        display_name: &str,
        version: &str,
        catalog_id: &str,
        confirm_unverified: bool,
    ) -> Result<Map<String, Value>, String> {
        if entries.is_empty() {
            return Err("Манифест модпака не содержит файлов".to_owned());
        }
        if is_symlink(target) || !target.is_dir() {
            return Err("Папка экземпляра модпака недоступна".to_owned());
        }
        assert_modpack_not_installed(target, source, project_id, catalog_id, "")?;
        let plan = self.manifest_download_plan(entries, confirm_unverified)?;
        let stage = target.join(format!(".trial-launcher-modpack-{}.part", super::snapshots::random_hex(16)));
        if stage.symlink_metadata().is_ok() {
            return Err("Временная папка модпака уже существует".to_owned());
        }
        create_private_dir(&stage)?;
        let outcome = (|| -> Result<Map<String, Value>, String> {
            let trusted = stage_plan_files(&self.http, &plan, &stage, 0)?;
            let written = merge_stage(&stage, target, source, project_id, display_name, version, catalog_id, "")?;
            let short_name = {
                let value: String = display_name.chars().take(160).collect();
                if value.is_empty() {
                    "Modpack".to_owned()
                } else {
                    value
                }
            };
            let mut result = Map::new();
            result.insert("source".to_owned(), json!(source));
            result.insert("project_id".to_owned(), json!(project_id));
            result.insert("name".to_owned(), json!(short_name));
            result.insert("version".to_owned(), json!(version.chars().take(80).collect::<String>()));
            result.insert("file_name".to_owned(), json!("ftb-manifest"));
            result.insert("path".to_owned(), json!(target.to_string_lossy()));
            result.insert("files".to_owned(), json!(written));
            result.insert("trusted_hash".to_owned(), json!(trusted));
            result.insert("marker".to_owned(), json!(target.join(MODPACK_MARKER).to_string_lossy()));
            Ok(result)
        })();
        let _ = fs::remove_dir_all(&stage);
        outcome
    }

    // ------------------------------------------------------- план архива ---

    fn archive_plan(&self, archive: &mut zip::ZipArchive<fs::File>) -> Result<ArchivePlan, String> {
        let mut entries: Vec<(String, u64, bool, Option<u32>)> = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            let info = archive
                .by_index(index)
                .map_err(|_| "Файл не является корректным ZIP-модпаком".to_owned())?;
            entries.push((
                info.name().map(|v| v.to_string()).unwrap_or_default(),
                info.size(),
                info.is_dir(),
                info.unix_mode(),
            ));
        }
        if entries.len() > MAX_MODPACK_ENTRIES {
            return Err("В модпаке слишком много файлов".to_owned());
        }
        let names: Vec<String> = entries
            .iter()
            .map(|(name, _, _, _)| archive_name(name))
            .collect::<Result<Vec<_>, _>>()?;
        let mut unique = HashSet::new();
        for name in &names {
            if !unique.insert(name.clone()) {
                return Err("Модпак содержит повторные пути".to_owned());
            }
        }
        if entries.iter().any(|(_, _, _, mode)| zip_entry_is_symlink(*mode)) {
            return Err("Символьные ссылки в модпаке запрещены".to_owned());
        }
        let prefix = archive_prefix(&names);
        let index_name = names
            .iter()
            .find(|name| name.strip_prefix(&prefix).unwrap_or(name.as_str()) == "modrinth.index.json")
            .cloned()
            .unwrap_or_default();
        let mut index = Map::new();
        if !index_name.is_empty() {
            let position = names
                .iter()
                .position(|name| *name == index_name)
                .unwrap_or_default();
            let size = entries[position].1;
            if size > MAX_MODPACK_INDEX_BYTES {
                return Err("modrinth.index.json слишком большой".to_owned());
            }
            let mut file = archive
                .by_index(position)
                .map_err(|_| "modrinth.index.json повреждён".to_owned())?;
            let mut buffer = Vec::new();
            file.by_ref()
                .take(MAX_MODPACK_INDEX_BYTES.saturating_add(1))
                .read_to_end(&mut buffer)
                .map_err(|_| "modrinth.index.json повреждён".to_owned())?;
            if buffer.len() as u64 > MAX_MODPACK_INDEX_BYTES || buffer.len() as u64 != size {
                return Err("modrinth.index.json слишком большой или повреждён".to_owned());
            }
            let value: Value =
                serde_json::from_slice(&buffer).map_err(|_| "modrinth.index.json повреждён".to_owned())?;
            let Some(fields) = value.as_object() else {
                return Err("Индекс модпака имеет неожиданный формат".to_owned());
            };
            index = fields.clone();
        }

        let mut manifest = Map::new();
        let manifest_name = names
            .iter()
            .find(|name| name.strip_prefix(&prefix).unwrap_or(name.as_str()) == CURSEFORGE_MANIFEST_NAME)
            .cloned()
            .unwrap_or_default();
        if !manifest_name.is_empty() && index_name.is_empty() {
            let position = names
                .iter()
                .position(|name| *name == manifest_name)
                .unwrap_or_default();
            let size = entries[position].1;
            if size > CURSEFORGE_MANIFEST_MAX_BYTES {
                return Err("manifest.json слишком большой".to_owned());
            }
            let mut file = archive
                .by_index(position)
                .map_err(|_| "manifest.json CurseForge повреждён".to_owned())?;
            let mut buffer = Vec::new();
            file.by_ref()
                .take(CURSEFORGE_MANIFEST_MAX_BYTES.saturating_add(1))
                .read_to_end(&mut buffer)
                .map_err(|_| "manifest.json CurseForge повреждён".to_owned())?;
            if buffer.len() as u64 > CURSEFORGE_MANIFEST_MAX_BYTES || buffer.len() as u64 != size {
                return Err("manifest.json слишком большой или повреждён".to_owned());
            }
            let value: Value =
                serde_json::from_slice(&buffer).map_err(|_| "manifest.json CurseForge повреждён".to_owned())?;
            let Some(fields) = value.as_object() else {
                return Err("manifest.json CurseForge имеет неожиданный формат".to_owned());
            };
            if let Some(files) = fields.get("files") {
                let too_many = !files.is_array()
                    || files.as_array().map(|items| items.len()).unwrap_or(0) > CURSEFORGE_MAX_REMOTE_FILES;
                if too_many {
                    return Err("manifest.json CurseForge содержит слишком много файлов".to_owned());
                }
            }
            manifest = fields.clone();
        }

        let mut selected: Vec<(usize, String)> = Vec::new();
        let mut destinations: HashSet<String> = HashSet::new();
        let mut skipped_loader: Vec<String> = Vec::new();
        for (position, name) in names.iter().enumerate() {
            let relative = name.strip_prefix(&prefix).unwrap_or(name.as_str()).to_owned();
            let is_dir = entries[position].2;
            if relative.is_empty() || is_dir || relative == index_name || relative == manifest_name {
                continue;
            }
            let destination = archive_destination(&relative)?;
            if destination.is_empty() {
                if loader_managed_path(&relative) && skipped_loader.len() < 100 {
                    skipped_loader.push(strip_modpack_prefix(&relative));
                }
                continue;
            }
            if !destinations.insert(destination.clone()) {
                return Err(format!("Модпак содержит повторный путь: {destination}"));
            }
            selected.push((position, destination));
        }
        Ok(ArchivePlan {
            index,
            manifest,
            selected,
            skipped_loader,
        })
    }

    fn modrinth_download_plan(
        &self,
        index: &Map<String, Value>,
        confirm_unverified: bool,
    ) -> Result<Vec<StagedFile>, String> {
        let entries = match index.get("files") {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(items)) => items.clone(),
            Some(_) => return Err("Индекс модпака содержит слишком много файлов".to_owned()),
        };
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        if entries.len() > MAX_MODPACK_ENTRIES {
            return Err("Индекс модпака содержит слишком много файлов".to_owned());
        }
        let mut plan: Vec<StagedFile> = Vec::new();
        let mut destinations: HashSet<String> = HashSet::new();
        for entry in entries.iter() {
            let Some(entry) = entry.as_object() else {
                return Err("Индекс модпака содержит некорректный файл".to_owned());
            };
            let raw_path = text_value(entry.get("path").unwrap_or(&Value::Null));
            if is_server_override(&raw_path) {
                continue;
            }
            if loader_managed_path(&raw_path) {
                // лаунчер сам управляет versions/ и libraries/: удалённый
                // файл оттуда мог бы перезаписать модлоадер
                continue;
            }
            let relative = archive_destination(&raw_path)?;
            let Some(downloads) = entry.get("downloads").and_then(Value::as_array) else {
                return Err("Индекс модпака содержит небезопасный или неполный файл".to_owned());
            };
            if relative.is_empty() || downloads.is_empty() {
                return Err("Индекс модпака содержит небезопасный или неполный файл".to_owned());
            }
            if !destinations.insert(relative.clone()) {
                return Err(format!("Индекс модпака содержит повторный путь: {relative}"));
            }
            let url = downloads
                .first()
                .map(|value| text_value(value))
                .unwrap_or_default();
            validate_archive_url(&url)?;
            let hashes = entry
                .get("hashes")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let publisher = publisher_hashes(&hashes);
            let trusted = publisher.iter().any(Option::is_some);
            if !trusted && !confirm_unverified {
                return Err(unverified_download_error("Файл модпака не имеет доверенного хеша"));
            }
            let size = optional_int(entry.get("fileSize").unwrap_or(&Value::Null));
            if let Some(size) = size {
                if size > MAX_MODPACK_UNCOMPRESSED_BYTES {
                    return Err("Распакованный модпак слишком большой".to_owned());
                }
            }
            plan.push(StagedFile {
                destination: relative,
                url,
                hashes: publisher,
                size,
                trusted,
                preview_name: String::new(),
                optional: false,
            });
        }
        Ok(plan)
    }

    fn manifest_download_plan(
        &self,
        entries: &[Map<String, Value>],
        confirm_unverified: bool,
    ) -> Result<Vec<StagedFile>, String> {
        if entries.is_empty() {
            return Err("Манифест модпака не содержит файлов".to_owned());
        }
        if entries.len() > MAX_MODPACK_ENTRIES {
            return Err("Манифест модпака содержит слишком много файлов".to_owned());
        }
        let mut plan: Vec<StagedFile> = Vec::new();
        let mut destinations: HashSet<String> = HashSet::new();
        for entry in entries {
            if entry.get("serveronly").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let destination = ftb_manifest_destination(entry)?;
            let url = field_text(entry, &["url", "download_url", "downloadUrl"]);
            if destination.is_empty() || url.is_empty() {
                let raw_path = [
                    text_value(entry.get("path").unwrap_or(&Value::Null))
                        .replace('\\', "/")
                        .trim()
                        .trim_matches('/')
                        .to_owned(),
                    field_text(entry, &["name", "file_name", "filename"])
                        .replace('\\', "/")
                        .trim()
                        .trim_matches('/')
                        .to_owned(),
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/");
                if loader_managed_path(&raw_path) {
                    continue;
                }
                return Err("Манифест FTB содержит неполный файл".to_owned());
            }
            if !destinations.insert(destination.clone()) {
                return Err(format!("Манифест FTB содержит повторный путь: {destination}"));
            }
            validate_archive_url(&url)?;
            let raw_hashes = entry
                .get("hashes")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut combined = Map::new();
            for key in ["sha1", "sha256", "sha512"] {
                combined.insert(
                    key.to_owned(),
                    raw_hashes
                        .get(key)
                        .filter(|value| !value.is_null())
                        .cloned()
                        .unwrap_or_else(|| entry.get(key).cloned().unwrap_or(Value::Null)),
                );
            }
            let publisher = publisher_hashes(&Value::Object(combined));
            let trusted = publisher.iter().any(Option::is_some);
            if !trusted && !confirm_unverified {
                return Err(unverified_download_error(
                    "Файл манифеста FTB не имеет доверенного хеша",
                ));
            }
            let size = optional_int(
                entry
                    .get("size")
                    .or_else(|| entry.get("fileSize"))
                    .unwrap_or(&Value::Null),
            );
            if let Some(size) = size {
                if size > MAX_MODPACK_UNCOMPRESSED_BYTES {
                    return Err("Распакованный модпак слишком большой".to_owned());
                }
            }
            let preview_name = {
                let name = field_text(entry, &["name", "file_name", "filename"]);
                if name.is_empty() {
                    Path::new(&destination)
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default()
                } else {
                    name
                }
            };
            plan.push(StagedFile {
                destination,
                url,
                hashes: publisher,
                size,
                trusted,
                preview_name,
                optional: false,
            });
        }
        Ok(plan)
    }

    fn curseforge_download_plan(
        &self,
        manifest: &Map<String, Value>,
        confirm_unverified: bool,
        resolve: bool,
    ) -> Result<Vec<StagedFile>, String> {
        let Some(files) = manifest.get("files").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        // curseForge-манифесты никогда не содержат хешей: если вообще есть
        // для обязательных файлов требуется явное подтверждение
        let required_files = files
            .iter()
            .filter(|entry| {
                entry
                    .as_object()
                    .map(|fields| !fields.get("serveronly").and_then(Value::as_bool).unwrap_or(false))
                    .unwrap_or(false)
            })
            .count();
        if required_files > 0 && !confirm_unverified {
            return Err(unverified_download_error(
                "Файл модпака CurseForge не имеет издательского хеша",
            ));
        }
        let mut plan: Vec<StagedFile> = Vec::new();
        let mut destinations: HashSet<String> = HashSet::new();
        for entry in files.iter().filter_map(Value::as_object) {
            if entry.get("serveronly").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let project_id = optional_int(entry.get("projectID").unwrap_or(&Value::Null));
            let file_id = optional_int(entry.get("fileID").unwrap_or(&Value::Null));
            let (Some(project_id), Some(file_id)) = (project_id, file_id) else {
                return Err("manifest.json CurseForge содержит неполный файл".to_owned());
            };
            if plan.len() >= CURSEFORGE_MAX_REMOTE_FILES {
                return Err("manifest.json CurseForge содержит слишком много файлов".to_owned());
            }
            let resolved = if resolve {
                self.resolve_curseforge_file(project_id, file_id)
            } else {
                None
            };
            if resolve && resolved.is_none() {
                // необязательный файл можно пропустить, обязательный нельзя
                if entry.get("required") == Some(&Value::Bool(false)) {
                    continue;
                }
                return Err(format!(
                    "Не удалось определить файл CurseForge {project_id}/{file_id}: каталог ещё обрабатывает проект. Повторите импорт через минуту."
                ));
            }
            let Some((url, display_name, size)) = resolved else {
                // идентификаторы без сети используются только в сводке превью
                plan.push(StagedFile {
                    destination: String::new(),
                    url: String::new(),
                    hashes: [None, None, None],
                    size: None,
                    trusted: false,
                    preview_name: format!("CurseForge project {project_id}"),
                    optional: entry.get("required") == Some(&Value::Bool(false)),
                });
                continue;
            };
            validate_curseforge_download_url(&url)?;
            let mut file_name = url.rsplit('/').next().unwrap_or_default().to_owned();
            if file_name.is_empty() {
                file_name = display_name.clone();
            }
            if !file_name.to_lowercase().ends_with(".jar") {
                let fallback = if display_name.is_empty() {
                    file_id.to_string()
                } else {
                    display_name.clone()
                };
                file_name = format!("{fallback}.jar");
            }
            let destination = archive_destination(&format!("mods/{file_name}"))?;
            if destination.is_empty() {
                return Err("manifest.json CurseForge содержит небезопасный путь файла".to_owned());
            }
            if !destinations.insert(destination.clone()) {
                return Err(format!("manifest.json CurseForge содержит повторный путь: {destination}"));
            }
            plan.push(StagedFile {
                destination,
                url,
                hashes: [None, None, None],
                size,
                trusted: false,
                preview_name: if display_name.is_empty() {
                    file_name.clone()
                } else {
                    display_name
                },
                optional: entry.get("required") == Some(&Value::Bool(false)),
            });
        }
        Ok(plan)
    }

    fn resolve_curseforge_file(&self, project_id: u64, file_id: u64) -> Option<(String, String, Option<u64>)> {
        let key = format!("{project_id}/{file_id}");
        if let Some(cached) = self.cfwidget_cache.borrow().get(&key) {
            return cached.clone();
        }
        let payload = match self
            .http
            .get_json(&format!("{CURSEFORGE_RESOLVER_BASE}/{project_id}/files/{file_id}"), None)
        {
            Ok(value) => value,
            Err(_) => {
                // cfwidget отвечает 202/ошибкой для проектов, которые ещё не
                // в его очереди: не вешаем импорт, отмечаем как неразрешённый
                self.cfwidget_cache.borrow_mut().insert(key, None);
                return None;
            }
        };
        if !payload.is_object() {
            self.cfwidget_cache.borrow_mut().insert(key, None);
            return None;
        }
        let queued = payload
            .get("queue")
            .map(|value| value.is_string() || value.is_number())
            .unwrap_or(false)
            || payload.get("queued").and_then(Value::as_bool).unwrap_or(false);
        if queued {
            self.cfwidget_cache.borrow_mut().insert(key, None);
            return None;
        }
        let url = text_value(
            payload
                .get("download_url")
                .or_else(|| payload.get("url"))
                .unwrap_or(&Value::Null),
        );
        let display = text_value(
            payload
                .get("display")
                .or_else(|| payload.get("name"))
                .unwrap_or(&Value::Null),
        );
        let size = optional_int(
            payload
                .get("size")
                .or_else(|| payload.get("filesize"))
                .unwrap_or(&Value::Null),
        );
        if url.is_empty() {
            self.cfwidget_cache.borrow_mut().insert(key, None);
            return None;
        }
        let value = Some((url, display, size));
        self.cfwidget_cache.borrow_mut().insert(key, value.clone());
        value
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_archive(
        &self,
        archive_path: &Path,
        stage: &Path,
        confirm_unverified: bool,
        expected_game_version: &str,
        expected_loader: &str,
        expected_loader_version: Option<&str>,
    ) -> Result<StagedSummary, String> {
        let mut archive = open_zip(archive_path)?;
        let plan = self.archive_plan(&mut archive)?;
        let downloads = self.modrinth_download_plan(&plan.index, confirm_unverified)?;
        let curseforge = if plan.manifest.is_empty() {
            Vec::new()
        } else {
            self.curseforge_download_plan(&plan.manifest, confirm_unverified, true)?
        };
        let curseforge_downloads: Vec<StagedFile> = curseforge
            .iter()
            .filter(|item| !item.url.is_empty())
            .cloned()
            .collect();
        if !plan.manifest.is_empty() {
            validate_curseforge_compatibility(
                &plan.manifest,
                expected_game_version,
                expected_loader,
                expected_loader_version,
                true,
            )?;
        } else {
            validate_mrpack_compatibility(
                &plan.index,
                expected_game_version,
                expected_loader,
                expected_loader_version,
                true,
            )?;
        }
        let archive_destinations: HashSet<&str> =
            plan.selected.iter().map(|(_, destination)| destination.as_str()).collect();
        let remote_destinations: HashSet<&str> = downloads
            .iter()
            .chain(curseforge_downloads.iter())
            .map(|item| item.destination.as_str())
            .collect();
        if let Some(overlap) = archive_destinations
            .intersection(&remote_destinations)
            .min()
        {
            return Err(format!("Архив и манифест модпака повторяют путь: {overlap}"));
        }
        let mut staged_bytes: u64 = 0;
        for (position, destination) in &plan.selected {
            let remaining = MAX_MODPACK_UNCOMPRESSED_BYTES.saturating_sub(staged_bytes);
            staged_bytes += extract_zip_file(&mut archive, *position, &stage.join(destination), remaining)?;
        }
        let trusted_modrinth = stage_plan_files(&self.http, &downloads, stage, staged_bytes)?;
        staged_bytes = staged_bytes.saturating_add(
            downloads
                .iter()
                .filter_map(|item| item.size)
                .fold(0u64, |accumulator, value| accumulator.saturating_add(value)),
        );
        let trusted_curseforge = if curseforge_downloads.is_empty() {
            true
        } else {
            stage_plan_files(&self.http, &curseforge_downloads, stage, staged_bytes)?
        };
        Ok(StagedSummary {
            trusted_hash: trusted_modrinth && trusted_curseforge,
            skipped_loader_files: plan.skipped_loader,
        })
    }
}

struct ArchivePlan {
    index: Map<String, Value>,
    manifest: Map<String, Value>,
    selected: Vec<(usize, String)>,
    skipped_loader: Vec<String>,
}

struct StagedSummary {
    trusted_hash: bool,
    skipped_loader_files: Vec<String>,
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let file = fs::File::open(path).map_err(|_| "Файл не является корректным ZIP-модпаком".to_owned())?;
    zip::ZipArchive::new(file).map_err(|_| "Файл не является корректным ZIP-модпаком".to_owned())
}

/// не извлекает символьные ссылки и ограничивает распакованный размер
fn extract_zip_file(
    archive: &mut zip::ZipArchive<fs::File>,
    position: usize,
    destination: &Path,
    remaining_bytes: u64,
) -> Result<u64, String> {
    if is_symlink(destination) {
        return Err(format!(
            "Файл модпака является символьной ссылкой: {}",
            destination
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default()
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог модпака: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut source = archive
        .by_index(position)
        .map_err(|_| "Файл не является корректным ZIP-модпаком".to_owned())?;
    let mut file = open_exclusive_file(destination)?;
    let mut written: u64 = 0;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|error| format!("Не удалось распаковать модпак: {error}"))?;
        if count == 0 {
            break;
        }
        if count as u64 > remaining_bytes.saturating_sub(written) {
            return Err("Распакованный модпак слишком большой".to_owned());
        }
        file.write_all(&buffer[..count])
            .map_err(|error| format!("Не удалось распаковать модпак: {error}"))?;
        written += count as u64;
    }
    Ok(written)
}

fn open_exclusive_file(destination: &Path) -> Result<fs::File, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(destination)
            .map_err(|error| format!("Не удалось распаковать модпак: {error}"))
    }
    #[cfg(not(unix))]
    {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|error| format!("Не удалось распаковать модпак: {error}"))
    }
}

fn stage_plan_files(http: &Arc<HttpClient>, entries: &[StagedFile], stage: &Path, initial_bytes: u64) -> Result<bool, String> {
    let mut staged_bytes = initial_bytes;
    for entry in entries {
        let remaining = MAX_MODPACK_UNCOMPRESSED_BYTES.saturating_sub(staged_bytes);
        staged_bytes += download_staged_file(http, entry, stage, remaining)?;
    }
    Ok(entries.iter().all(|entry| entry.trusted))
}

fn download_staged_file(http: &Arc<HttpClient>, entry: &StagedFile, stage: &Path, remaining: u64) -> Result<u64, String> {
    if remaining == 0 {
        return Err("Распакованный модпак слишком большой".to_owned());
    }
    if let Some(size) = entry.size {
        if size > remaining {
            return Err("Распакованный модпак слишком большой".to_owned());
        }
    }
    let path = stage.join(&entry.destination);
    if path.symlink_metadata().is_ok() {
        return Err(format!("Путь модпака уже существует: {}", entry.destination));
    }
    http.download(
        &entry.url,
        &path,
        entry.hashes[0].as_deref(),
        entry.hashes[1].as_deref(),
        entry.hashes[2].as_deref(),
        entry.size,
        None,
        None,
        None,
    )?;
    if is_symlink(&path) || !path.is_file() {
        return Err(format!("Не удалось подготовить файл модпака: {}", entry.destination));
    }
    let actual_size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    if actual_size > remaining {
        let _ = fs::remove_file(&path);
        return Err("Распакованный модпак слишком большой".to_owned());
    }
    Ok(actual_size)
}

/// переносит файлы с резервной копией и откатывает изменения при ошибке
#[allow(clippy::too_many_arguments)]
fn merge_stage(
    stage: &Path,
    target: &Path,
    source: &str,
    project_id: &str,
    display_name: &str,
    version: &str,
    catalog_id: &str,
    archive_sha256: &str,
) -> Result<Vec<String>, String> {
    let mut written: Vec<String> = Vec::new();
    let mut changed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut created_directories: Vec<PathBuf> = Vec::new();
    let backup_root = target.join(format!(
        ".trial-launcher-modpack-backup-{}",
        super::snapshots::random_hex(16)
    ));
    create_private_dir(&backup_root)?;

    fn ensure_directory(target: &Path, directory: &Path, created: &mut Vec<PathBuf>) -> Result<(), String> {
        reject_symlink_parents(target, directory)?;
        if is_symlink(directory) {
            return Err(format!("Путь модпака является символьной ссылкой: {}", directory.display()));
        }
        if let Ok(meta) = directory.symlink_metadata() {
            if !meta.is_dir() {
                return Err(format!("Путь модпака не является папкой: {}", directory.display()));
            }
            return Ok(());
        }
        let mut missing: Vec<PathBuf> = Vec::new();
        let mut current = directory.to_owned();
        while current != target && current.symlink_metadata().is_err() {
            if is_symlink(&current) {
                return Err(format!("Путь модпака является символьной ссылкой: {}", current.display()));
            }
            missing.push(current.clone());
            match current.parent() {
                Some(parent) => current = parent.to_owned(),
                None => break,
            }
        }
        fs::create_dir_all(directory).map_err(|error| format!("Не удалось создать каталог модпака: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o700));
        }
        created.extend(missing.into_iter().rev());
        Ok(())
    }

    fn rollback(changed: &[(PathBuf, Option<PathBuf>)], created_directories: &[PathBuf]) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        for (destination, backup) in changed.iter().rev() {
            let outcome = (|| -> Result<(), String> {
                if let Some(backup) = backup {
                    fs::rename(backup, destination).map_err(|error| error.to_string())?;
                } else if destination.is_file() {
                    fs::remove_file(destination).map_err(|error| error.to_string())?;
                } else if destination.symlink_metadata().is_ok() {
                    return Err(format!("Неожиданная папка вместо файла: {}", destination.display()));
                }
                Ok(())
            })();
            if let Err(error) = outcome {
                errors.push(error);
            }
        }
        for directory in created_directories.iter().rev() {
            let _ = fs::remove_dir(directory);
        }
        errors
    }

    // сначала обходятся каталоги, затем файлы; каждая группа сортируется по пути
    fn walk_stage(stage: &Path, relative: &str, entries: &mut Vec<(bool, String)>) {
        let Ok(items) = fs::read_dir(stage) else {
            return;
        };
        let mut children: Vec<_> = items.filter_map(|entry| entry.ok()).collect();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().to_string();
            let child_relative = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let is_dir = !child.path().is_symlink()
                && child.file_type().map(|value| value.is_dir()).unwrap_or(false);
            if is_dir {
                entries.push((true, child_relative.clone()));
                walk_stage(&child.path(), &child_relative, entries);
            } else {
                entries.push((false, child_relative));
            }
        }
    }
    let mut entries: Vec<(bool, String)> = Vec::new();
    walk_stage(stage, "", &mut entries);
    entries.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));

    let outcome = (|| -> Result<(), String> {
        for (is_dir, relative) in &entries {
            let destination = target.join(relative);
            if *is_dir {
                ensure_directory(target, &destination, &mut created_directories)?;
                continue;
            }
            let item = stage.join(relative);
            let meta = item
                .symlink_metadata()
                .map_err(|_| format!("Некорректный файл модпака: {relative}"))?;
            if meta.is_dir() || meta.file_type().is_symlink() {
                return Err(format!("Некорректный файл модпака: {relative}"));
            }
            if let Some(parent) = destination.parent() {
                reject_symlink_parents(target, parent)?;
            }
            if is_symlink(&destination) {
                return Err(format!("Файл модпака является символьной ссылкой: {relative}"));
            }
            if let Some(parent) = destination.parent() {
                ensure_directory(target, parent, &mut created_directories)?;
            }
            if destination.symlink_metadata().map(|meta| !meta.is_file()).unwrap_or(false) {
                return Err(format!("Путь модпака не является файлом: {relative}"));
            }
            let mut backup: Option<PathBuf> = None;
            if destination.symlink_metadata().is_ok() {
                let backup_path = backup_root.join(relative);
                if let Some(parent) = backup_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("Не удалось подготовить резервную копию: {error}"))?;
                }
                fs::copy(&destination, &backup_path)
                    .map_err(|error| format!("Не удалось подготовить резервную копию: {error}"))?;
                backup = Some(backup_path);
            }
            changed.push((destination.clone(), backup));
            fs::copy(&item, &destination)
                .map_err(|error| format!("Не удалось скопировать файл модпака: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&destination, fs::Permissions::from_mode(0o600));
            }
            written.push(relative.clone());
        }
        record_modpack(
            target,
            source,
            project_id,
            display_name,
            version,
            &written,
            catalog_id,
            archive_sha256,
        )?;
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            let _ = fs::remove_dir_all(&backup_root);
            Ok(written)
        }
        Err(error) => {
            let errors = rollback(&changed, &created_directories);
            // при неудачном откате бэкап остаётся на месте для восстановления
            if !errors.is_empty() {
                return Err(
                    "Импорт модпака завершился ошибкой; резервная копия оставлена для восстановления"
                        .to_owned(),
                );
            }
            let _ = fs::remove_dir_all(&backup_root);
            Err(error)
        }
    }
}

// ----------------------------------------------- совместимость манифестов --

fn compatibility_payload(
    minecraft: &str,
    loader: &str,
    loader_version: &str,
    issues: Vec<String>,
) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("minecraft".to_owned(), json!(minecraft));
    payload.insert("loader".to_owned(), json!(loader));
    payload.insert("loader_version".to_owned(), json!(loader_version));
    payload.insert("compatible".to_owned(), json!(issues.is_empty()));
    payload.insert("issues".to_owned(), json!(issues));
    payload
}

fn validate_curseforge_compatibility(
    manifest: &Map<String, Value>,
    game_version: &str,
    loader: &str,
    loader_version: Option<&str>,
    strict: bool,
) -> Result<Map<String, Value>, String> {
    let Some(minecraft) = manifest.get("minecraft").filter(|value| value.is_object()) else {
        return Ok(compatibility_payload("", "", "", Vec::new()));
    };
    let required_game = text_value(minecraft.get("version").unwrap_or(&Value::Null));
    let selected_game = game_version.trim();

    let loader_labels = ["forge", "fabric", "fabric-loader", "quilt", "neoforge"];
    let mut required_loader = String::new();
    let mut required_loader_version = String::new();
    if let Some(mod_loaders) = minecraft.get("modLoaders").and_then(Value::as_array) {
        let primary = mod_loaders
            .iter()
            .filter_map(Value::as_object)
            .find(|item| item.get("primary").and_then(Value::as_bool).unwrap_or(false))
            .or_else(|| mod_loaders.iter().filter_map(Value::as_object).next());
        if let Some(primary) = primary {
            let identifier = text_value(primary.get("id").unwrap_or(&Value::Null));
            let (family, version) = match identifier.split_once('-') {
                Some((family, version)) => (family.trim().to_lowercase(), version.trim().to_owned()),
                None => (identifier.trim().to_lowercase(), String::new()),
            };
            if !loader_labels.contains(&family.as_str()) {
                let message = format!("Модпак требует незнакомый загрузчик «{identifier}»");
                if strict {
                    return Err(message);
                }
                return Ok(compatibility_payload(&required_game, "", "", vec![message]));
            }
            required_loader = if family == "fabric-loader" {
                "fabric".to_owned()
            } else {
                family
            };
            required_loader_version = version;
        }
    }

    let mut mismatches: Vec<String> = Vec::new();
    if !required_game.is_empty() && !selected_game.is_empty() && required_game != selected_game {
        mismatches.push(format!(
            "Модпак требует Minecraft {required_game}; выбран Minecraft {selected_game}"
        ));
    }
    let selected_loader = loader.trim().to_lowercase();
    if !required_loader.is_empty() && !selected_loader.is_empty() && selected_loader != required_loader {
        let version_detail = if required_loader_version.is_empty() {
            String::new()
        } else {
            format!(" версии {required_loader_version}")
        };
        mismatches.push(format!(
            "Модпак требует загрузчик {required_loader}{version_detail}; выбран {selected_loader}"
        ));
    } else if !required_loader_version.is_empty() {
        if let Some(selected) = loader_version.filter(|value| !value.trim().is_empty()) {
            if selected.trim() != required_loader_version {
                mismatches.push(format!(
                    "Модпак требует {required_loader} {required_loader_version}; выбран {selected}"
                ));
            }
        }
    }
    Ok(compatibility_payload(
        &required_game,
        &required_loader,
        &required_loader_version,
        mismatches,
    ))
}

fn validate_mrpack_compatibility(
    index: &Map<String, Value>,
    game_version: &str,
    loader: &str,
    loader_version: Option<&str>,
    strict: bool,
) -> Result<Map<String, Value>, String> {
    let Some(dependencies) = index
        .get("dependencies")
        .and_then(Value::as_object)
    else {
        return Ok(compatibility_payload("", "", "", Vec::new()));
    };

    let required_game = text_value(dependencies.get("minecraft").unwrap_or(&Value::Null));
    let selected_game = game_version.trim();
    let mut mismatches: Vec<String> = Vec::new();
    if !required_game.is_empty() && !selected_game.is_empty() && required_game != selected_game {
        mismatches.push(format!(
            "Модпак требует Minecraft {required_game}; выбран Minecraft {selected_game}"
        ));
    }

    let dependency_loaders = [
        ("fabric-loader", "fabric", "Fabric"),
        ("forge", "forge", "Forge"),
        ("neoforge", "neoforge", "NeoForge"),
        ("quilt-loader", "quilt", "Quilt"),
    ];
    let required: Vec<(&str, &str, String)> = dependency_loaders
        .iter()
        .filter_map(|(dependency, family, label)| {
            if let Some(value) = dependencies.get(*dependency) {
                let version = text_value(value);
                if !version.is_empty() {
                    return Some((*family, *label, version));
                }
            }
            None
        })
        .collect();
    if required.len() > 1 {
        let message = "Манифест модпака требует несколько несовместимых загрузчиков";
        if strict {
            return Err(message.to_owned());
        }
        return Ok(compatibility_payload(&required_game, "", "", vec![message.to_owned()]));
    }
    let (required_loader, loader_label, required_loader_version) = required
        .first()
        .map(|(family, label, version)| (*family, *label, version.clone()))
        .unwrap_or(("", "", String::new()));
    let selected_loader = loader.trim().to_lowercase();
    if !required_loader.is_empty() && !selected_loader.is_empty() && selected_loader != required_loader {
        let version_detail = if required_loader_version.is_empty() {
            String::new()
        } else {
            format!(" версии {required_loader_version}")
        };
        mismatches.push(format!(
            "Модпак требует загрузчик {required_loader}{version_detail}; выбран {selected_loader}"
        ));
    }

    if !required_loader_version.is_empty() {
        if let Some(selected) = loader_version {
            if selected.trim().is_empty() {
                mismatches.push(format!(
                    "Модпак требует {loader_label} версии {required_loader_version}; версия загрузчика не выбрана"
                ));
            } else if selected.trim() != required_loader_version {
                mismatches.push(format!(
                    "Модпак требует {loader_label} версии {required_loader_version}; выбрана версия {selected}"
                ));
            }
        }
    }
    if !mismatches.is_empty() && strict {
        return Err(mismatches.join("; "));
    }
    Ok(compatibility_payload(
        &required_game,
        &required_loader,
        &required_loader_version,
        mismatches,
    ))
}

// ------------------------------------------------------- операции GUI -----

fn http_client_for(request: &Value) -> Result<Arc<HttpClient>, String> {
    let settings = super::config::ConfigStore::load()?.settings().clone();
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    super::net::client_from_settings(&settings, operation_id)
}

fn decode_base64_limit(encoded: &str, limit: u64, label: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    if encoded.len() as u64 > limit.saturating_mul(4).div_ceil(3).saturating_add(1024) {
        return Err(format!("{label} слишком большой"));
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| format!("{label} повреждён"))?;
    if content.len() as u64 > limit {
        return Err(format!("{label} слишком большой"));
    }
    Ok(content)
}

pub fn modpack_import(request: &Value) -> Result<Value, String> {
    let importer = ModpackImporter::new(http_client_for(request)?);
    let file_name = request.get("file_name").and_then(Value::as_str).unwrap_or("modpack.zip");
    let content = decode_base64_limit(
        request.get("data").and_then(Value::as_str).unwrap_or_default(),
        MAX_MODPACK_BYTES,
        "Архив модпака",
    )?;
    let confirm_unverified = request.get("confirm_unverified").and_then(Value::as_bool).unwrap_or(false);
    let expected_game_version = request
        .get("game_version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let expected_loader = request
        .get("loader")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let expected_loader_version = request
        .get("loader_version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();

    if request.get("preflight_only").and_then(Value::as_bool).unwrap_or(false) {
        let mut dependencies = importer.preflight_bytes(
            &content,
            file_name,
            confirm_unverified,
            &expected_game_version,
            &expected_loader,
            Some(expected_loader_version.as_str()),
        )?;
        // предпросмотр модов: что именно окажется в папке mods после
        // импорта; ошибка превью не должна блокировать сам импорт
        if let Ok(preview) = importer.mod_preview_bytes(&content, file_name) {
            dependencies.insert("mods".to_owned(), Value::Object(preview));
        }
        return Ok(json!({ "preflight": true, "dependencies": dependencies }));
    }
    let name = request
        .get("instance_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let config = super::config::ConfigStore::load()?;
    let store = super::instances::InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.kind == "server" {
        return Err("Импорт модпака доступен только для клиентского экземпляра".to_owned());
    }
    let (instance_version, instance_loader) = super::modcat::instance_mod_context(&info)?;
    let loader_version = super::instances::loader_version_of(
        &info.path,
        &info.version_id,
        &info.minecraft_version,
        &info.loader,
    );
    let lock = super::server::InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let result = importer.import_bytes(
        &content,
        &info.path,
        file_name,
        "local",
        request.get("project_id").and_then(Value::as_str).unwrap_or_default(),
        "",
        request.get("display_name").and_then(Value::as_str).unwrap_or_default(),
        request.get("version").and_then(Value::as_str).unwrap_or_default(),
        confirm_unverified,
        &instance_version,
        &instance_loader.unwrap_or_else(|| "vanilla".to_owned()),
        if loader_version.is_empty() {
            None
        } else {
            Some(loader_version.as_str())
        },
    );
    drop(lock);
    let result = result?;
    super::events::emit_status_event("workspace-changed", "Модпак импортирован.");
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    let mut response = result;
    response.insert("instance_name".to_owned(), json!(name));
    response.insert("details".to_owned(), workspace.snapshot(&name)?);
    Ok(Value::Object(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-modpacks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn result(
        source: &str,
        id: &str,
        downloads: i64,
        released_at: i64,
        updated_at: i64,
    ) -> ModpackResult {
        ModpackResult::new(
            source,
            id,
            id.to_owned(),
            String::new(),
            String::new(),
            String::new(),
            downloads,
            String::new(),
            String::new(),
            String::new(),
            Vec::new(),
            released_at,
            updated_at,
        )
    }

    #[test]
    fn archive_destination_strips_prefixes_and_blocks_metadata() {
        assert_eq!(archive_destination("overrides/configs/a.toml").unwrap(), "configs/a.toml");
        assert_eq!(archive_destination(".minecraft/mods/x.jar").unwrap(), "mods/x.jar");
        assert_eq!(archive_destination("minecraft/mods/x.jar").unwrap(), "mods/x.jar");
        assert_eq!(archive_destination("manifest.json").unwrap(), "");
        assert_eq!(archive_destination("versions/1.20.1.json").unwrap(), "");
        assert_eq!(archive_destination("libraries/net/x.jar").unwrap(), "");
        assert_eq!(archive_destination("server-overrides/run.sh").unwrap(), "");
        assert!(archive_destination(".trial-launcher-live.json").is_err());
        assert!(archive_destination("../escape.jar").is_err());
    }

    #[test]
    fn publisher_hashes_reject_wrong_length() {
        // sha1 из трёх символов и sha256 ровно из 64 hex-символов
        let value = json!({
            "sha1": "abc",
            "sha256": "b6d3c1c3e1f6a3d7e1c9b6d3c1c3e1f6a3d7e1c9b6d3c1c3e1f6a3d7e1c9b6d3"
        });
        let hashes = publisher_hashes(&value);
        assert!(hashes[0].is_none());
        assert!(hashes[1].is_some());
        assert!(hashes[2].is_none());
    }

    #[test]
    fn local_modpack_pagination_reaches_results_after_first_thirty() {
        let items: Vec<usize> = (0..55).collect();
        assert_eq!(page_slice(items, 40, 10), (40..50).collect::<Vec<_>>());
    }

    #[test]
    fn modrinth_sort_modes_are_forwarded_as_global_api_ordering() {
        assert_eq!(modrinth_sort_index("relevance").unwrap(), "relevance");
        assert_eq!(modrinth_sort_index("downloads").unwrap(), "downloads");
        assert_eq!(modrinth_sort_index("newest").unwrap(), "newest");
        assert_eq!(modrinth_sort_index("updated").unwrap(), "updated");
        assert!(modrinth_sort_index("unknown").is_err());
    }

    #[test]
    fn ftb_search_uses_documented_id_search_and_listing_routes() {
        let base = "https://api.feed-the-beast.com/v1/modpacks/public/";
        let url = ftb_search_url(base, "Better MC", 42);
        assert!(url.starts_with(
            "https://api.feed-the-beast.com/v1/modpacks/public/modpack/search/42?"
        ));
        assert!(url.ends_with("term=Better+MC"));
        assert_eq!(
            ftb_listing_url(base, "downloads", 20).unwrap(),
            "https://api.feed-the-beast.com/v1/modpacks/public/modpack/popular/installs/20"
        );
        assert_eq!(
            ftb_listing_url(base, "updated", 20).unwrap(),
            "https://api.feed-the-beast.com/v1/modpacks/public/modpack/updated/20"
        );
        assert_eq!(
            ftb_listing_url(base, "relevance", 20).unwrap(),
            "https://api.feed-the-beast.com/v1/modpacks/public/modpack/featured/20"
        );
        assert!(ftb_listing_url(base, "newest", 20).is_err());
    }

    #[test]
    fn ftb_search_total_uses_ftb_pack_ids_not_combined_count() {
        let response = json!({ "packs": [135, 134], "curseforge": [1001, 1002, 1003], "count": 5 });
        assert_eq!(ftb_pack_ids(&response), vec!["135".to_owned(), "134".to_owned()]);
        assert_eq!(ftb_search_total(&response, 10), Some(2));
        assert_eq!(ftb_search_total(&response, 2), None);
    }

    #[test]
    fn modpack_result_parsers_use_current_provider_fields() {
        let ftb = json!({
            "id": 135,
            "name": "FTB Example",
            "installs": 42,
            "released": 100,
            "updated": 200,
            "versions": [{ "targets": [
                { "name": "minecraft", "version": "1.21.1", "type": "game" },
                { "name": "fabric", "version": "0.16.0", "type": "modloader" }
            ] }]
        });
        let ftb = ftb_result(ftb.as_object().unwrap());
        assert_eq!(ftb.downloads, 42);
        assert_eq!(ftb.released_at, 100);
        assert_eq!(ftb.updated_at, 200);
        assert_eq!(ftb.game_versions, vec!["1.21.1".to_owned()]);

        let technic = json!({
            "slug": "technic-example",
            "displayName": "Technic Example",
            "user": "author",
            "installs": 84,
            "minecraft": "1.20.1",
            "icon": { "url": "https://cdn.technicpack.net/icon.png" },
            "feed": [{ "date": 300 }, { "date": 250 }]
        });
        let technic = technic_result(technic.as_object().unwrap());
        assert_eq!(technic.downloads, 84);
        assert_eq!(technic.updated_at, 300);
        assert_eq!(technic.game_versions, vec!["1.20.1".to_owned()]);
        assert_eq!(technic.icon_url, "https://cdn.technicpack.net/icon.png");
        assert_eq!(technic.author, "author");
    }

    #[test]
    fn modpack_sorting_precedes_page_slicing_for_non_modrinth_providers() {
        let mut results = vec![
            result("technic", "a", 5, 100, 200),
            result("technic", "b", 20, 300, 100),
            result("technic", "c", 10, 200, 400),
        ];
        sort_modpack_results(&mut results, "downloads");
        assert_eq!(page_slice(results, 1, 1)[0].project_id.as_str(), "c");

        let mut results = vec![result("ftb", "a", 5, 100, 200), result("ftb", "b", 20, 300, 100)];
        sort_modpack_results(&mut results, "newest");
        assert_eq!(results[0].project_id.as_str(), "b");
        sort_modpack_results(&mut results, "updated");
        assert_eq!(results[0].project_id.as_str(), "a");
    }

    #[test]
    fn technic_rejects_newest_sort_without_creation_timestamps() {
        assert!(validate_provider_sort("technic", "newest").is_err());
        assert!(validate_provider_sort("technic", "updated").is_ok());
        assert!(validate_provider_sort("ftb", "newest").is_ok());
    }

    #[test]
    fn safe_archive_name_falls_back_to_modpack() {
        assert_eq!(safe_archive_name("../evil.zip"), "modpack.zip");
        assert_eq!(safe_archive_name("pack.mrpack"), "pack.mrpack");
        assert_eq!(safe_archive_name("dir/pack.zip"), "modpack.zip");
    }

    #[test]
    fn record_and_assert_prevent_double_install() {
        let dir = tmpdir("marker");
        record_modpack(&dir, "modrinth", "proj-1", "Pack", "1.0", &["mods/a.jar".to_owned()], "modrinth", "aabbcc").unwrap();
        assert!(assert_modpack_not_installed(&dir, "modrinth", "proj-1", "modrinth", "").is_err());
        assert!(assert_modpack_not_installed(&dir, "local", "", "", "aabbcc").is_err());
        assert!(assert_modpack_not_installed(&dir, "modrinth", "proj-2", "modrinth", "").is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mrpack_compatibility_reports_issues() {
        let index = json!({ "dependencies": { "minecraft": "1.20.1", "fabric-loader": "0.15.0" } });
        let fields = index.as_object().cloned().unwrap();
        assert!(validate_mrpack_compatibility(&fields, "1.20.1", "fabric", Some("0.15.0"), true).is_ok());
        let outcome = validate_mrpack_compatibility(&fields, "1.19.4", "forge", None, false).unwrap();
        assert_eq!(outcome.get("compatible").and_then(Value::as_bool), Some(false));
        assert_eq!(outcome.get("issues").and_then(Value::as_array).map(Vec::len), Some(2));
    }

    #[test]
    fn curseforge_compatibility_unknown_loader_is_strict_error() {
        let manifest = json!({
            "minecraft": { "version": "1.20.1", "modLoaders": [ { "id": "lite-loader-1.0", "primary": true } ] }
        });
        let fields = manifest.as_object().cloned().unwrap();
        assert!(validate_curseforge_compatibility(&fields, "1.20.1", "fabric", None, true).is_err());
        let soft = validate_curseforge_compatibility(&fields, "1.20.1", "fabric", None, false).unwrap();
        assert_eq!(soft.get("compatible").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn archive_prefix_detects_wrapper_directory() {
        let names = vec!["pack/modrinth.index.json".to_owned(), "pack/overrides/x".to_owned()];
        assert_eq!(archive_prefix(&names), "pack/");
        let flat = vec!["modrinth.index.json".to_owned(), "overrides/x".to_owned()];
        assert_eq!(archive_prefix(&flat), "");
    }

    #[test]
    fn import_bytes_installs_override_files_with_marker() {
        use std::io::Write as _;
        let target = tmpdir("import");
        let directory = staging_directory("test-archive-").unwrap();
        let archive_path = directory.join("pack.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("overrides/configs/common.toml", options).unwrap();
        zip.write_all(b"enabled=true\n").unwrap();
        zip.start_file("overrides/mods/sodium.jar", options).unwrap();
        zip.write_all(b"jar-bytes").unwrap();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(br#"{"minecraft": {"version": "1.20.1"}}"#).unwrap();
        zip.start_file("versions/1.20.1.json", options).unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();
        let content = fs::read(&archive_path).unwrap();
        let _ = fs::remove_dir_all(&directory);

        let client = Arc::new(HttpClient::new(30, 1, "", None).expect("клиент"));
        let importer = ModpackImporter::new(client);
        let result = importer
            .import_bytes(&content, &target, "pack.zip", "local", "", "", "Pack", "1.0", false, "1.20.1", "vanilla", None)
            .unwrap();
        assert_eq!(result.get("name").and_then(Value::as_str), Some("Pack"));
        let files = result.get("files").and_then(Value::as_array).unwrap();
        let names: Vec<&str> = files.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&"configs/common.toml"));
        assert!(names.contains(&"mods/sodium.jar"));
        assert!(!names.iter().any(|name| name.contains("versions") || name.contains("manifest")));
        // служебные каталоги лаунчера не тронуты
        assert!(!target.join("versions").exists());
        let marker = target.join(MODPACK_MARKER);
        assert!(marker.is_file());
        // повторный импорт того же архива отклоняется
        assert!(importer
            .import_bytes(&content, &target, "pack.zip", "local", "", "", "Pack", "1.0", false, "1.20.1", "vanilla", None)
            .is_err());
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn mod_preview_lists_modrinth_and_archive_jars() {
        use std::io::Write as _;
        let directory = staging_directory("test-preview-").unwrap();
        let archive_path = directory.join("pack.mrpack");
        let file = fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("modrinth.index.json", options).unwrap();
        zip.write_all(
            br#"{"files": [{"path": "mods/remote.jar", "downloads": ["https://cdn.modrinth.com/x.jar"], "hashes": {"sha512": "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"}, "fileSize": 3}]}"#,
        )
        .unwrap();
        zip.start_file("overrides/mods/local.jar", options).unwrap();
        zip.write_all(b"jar").unwrap();
        zip.finish().unwrap();
        let content = fs::read(&archive_path).unwrap();
        let _ = fs::remove_dir_all(&directory);
        let client = Arc::new(HttpClient::new(30, 1, "", None).expect("клиент"));
        let importer = ModpackImporter::new(client);
        let preview = importer.mod_preview_bytes(&content, "pack.mrpack").unwrap();
        assert_eq!(preview.get("total").and_then(Value::as_i64), Some(2));
        let mods = preview.get("mods").and_then(Value::as_array).unwrap();
        let names: Vec<&str> = mods.iter().filter_map(|item| item.get("name").and_then(Value::as_str)).collect();
        assert!(names.contains(&"remote.jar"));
        assert!(names.contains(&"local.jar"));
    }
}
