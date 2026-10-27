// импорт миров Minecraft из локальных папок и архивов
use base64::Engine;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::ConfigStore;
use super::instances::{InstanceInfo, InstanceStore};

const STAGING_DIRECTORY: &str = ".nivora-world-import";
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 300_000;
const MAX_SEARCH_DEPTH: usize = 6;

const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENCODED_CHUNK_BYTES: usize = 48 * 1024 * 1024;
const STALE_UPLOAD_SECONDS: u64 = 24 * 60 * 60;
const ARCHIVE_SUFFIXES: [&str; 8] = [".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz"];
const UNSUPPORTED_ARCHIVES: [&str; 2] = [".rar", ".7z"];

pub fn world_import_path(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim();
    let source = request.get("path").and_then(Value::as_str).unwrap_or_default().trim();
    if name.is_empty() || source.is_empty() {
        return Err("Укажите экземпляр и папку мира".to_owned());
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(name)?;
    if info.game_edition == "bedrock" || info.kind == "server" {
        return Err("Импорт миров доступен для клиентских Java-экземпляров".to_owned());
    }
    if info.running {
        return Err("Остановите игру перед импортом мира".to_owned());
    }
    // источник должен находиться в корнях, доступных встроенному проводнику
    let source_path = super::fs_browser::canonical_within_allowed(source)?;
    if !source_path.is_dir() {
        return Err(format!("Это не папка: {source}"));
    }
    let staging = info.path.join(STAGING_DIRECTORY).join(format!("path-{}", std::process::id()));
    if staging.is_symlink() {
        return Err("Каталог импорта миров недоступен".to_owned());
    }
    let _ = fs::remove_dir_all(&staging);
    let result = (|| -> Result<Vec<String>, String> {
        fs::create_dir_all(&staging).map_err(|error| format!("Не удалось создать каталог импорта: {error}"))?;
        let staging_root = copy_tree(&source_path, &staging)?;
        let worlds = find_world_roots(&staging_root);
        if worlds.is_empty() {
            return Err("Мир Minecraft не найден: в папке нет level.dat. Выберите папку конкретного мира из saves/.".to_owned());
        }
        let saves = info.path.join("saves");
        if saves.is_symlink() || (saves.exists() && !saves.is_dir()) {
            return Err("Каталог миров является символьной ссылкой".to_owned());
        }
        fs::create_dir_all(&saves).map_err(|error| format!("Не удалось создать каталог saves: {error}"))?;
        let label = source_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut imported: Vec<String> = Vec::new();
        for world in worlds {
            let name = if world == staging_root {
                if label.is_empty() { "Мир".to_owned() } else { label.clone() }
            } else {
                world.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default()
            };
            let target = unique_world_dir(&saves, &safe_world_name(&name));
            fs::rename(&world, &target).map_err(|error| format!("Не удалось перенести мир в saves/: {error}"))?;
            imported.push(target.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default());
        }
        Ok(imported)
    })();
    let _ = fs::remove_dir_all(&staging);
    let imported = result?;
    super::events::emit_status_event("workspace-changed", &format!("Импортировано миров: {}.", imported.len()));
    Ok(json!({ "imported": imported }))
}

fn staging_root(instance_dir: &Path) -> Result<PathBuf, String> {
    let root = instance_dir.join(STAGING_DIRECTORY);
    let meta = fs::symlink_metadata(&root);
    if matches!(&meta, Ok(meta) if meta.is_symlink())
        || matches!(&meta, Ok(meta) if !meta.is_dir())
    {
        return Err("Каталог импорта миров недоступен".to_owned());
    }
    Ok(root)
}

fn upload_dir(instance_dir: &Path, upload_id: &str) -> Result<PathBuf, String> {
    let valid = upload_id.len() == 32 && upload_id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err("Некорректный идентификатор загрузки".to_owned());
    }
    let directory = staging_root(instance_dir)?.join(upload_id);
    if !directory.is_dir() || directory.join("meta.json").symlink_metadata().is_err() {
        return Err("Загрузка мира не найдена или уже завершена".to_owned());
    }
    Ok(directory)
}

fn read_meta(directory: &Path) -> Result<Map<String, Value>, String> {
    let path = directory.join("meta.json");
    let meta = fs::symlink_metadata(&path).map_err(|_| "Загрузка мира повреждена".to_owned())?;
    if meta.is_symlink() || !meta.is_file() {
        return Err("Загрузка мира повреждена".to_owned());
    }
    let document: Value = serde_json::from_slice(&fs::read(&path).map_err(|_| "Загрузка мира повреждена".to_owned())?)
        .map_err(|_| "Загрузка мира повреждена".to_owned())?;
    document.as_object().cloned().ok_or_else(|| "Загрузка мира повреждена".to_owned())
}

fn write_meta(directory: &Path, meta: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string(&Value::Object(meta.clone())).map_err(|_| "Загрузка мира повреждена".to_owned())?;
    fs::write(directory.join("meta.json"), text).map_err(|_| "Не удалось обновить загрузку мира".to_owned())
}

fn meta_u64(meta: &Map<String, Value>, key: &str) -> u64 {
    meta.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn safe_relative_path(value: &str) -> Result<Vec<String>, String> {
    let text = value.replace('\\', "/");
    let text = text.trim();
    if text.is_empty()
        || text.contains('\0')
        || text.starts_with('/')
        || (text.len() >= 2 && text.as_bytes()[1] == b':' && text.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(format!("Небезопасный путь файла: {value:?}"));
    }
    let parts: Vec<&str> = text.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    if parts.is_empty()
        || parts.iter().any(|part| {
            *part == ".." || part.bytes().any(|byte| matches!(byte, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*') || byte < 0x20)
        })
    {
        return Err(format!("Небезопасный путь файла: {value:?}"));
    }
    if parts.len() > 32 || text.chars().count() > 1024 {
        return Err("Слишком глубокая вложенность файлов мира".to_owned());
    }
    Ok(parts.into_iter().map(str::to_owned).collect())
}

fn cleanup_stale(instance_dir: &Path) {
    let Ok(root) = staging_root(instance_dir) else { return };
    let Ok(entries) = fs::read_dir(&root) else { return };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0);
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || path.is_symlink() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let age = modified.duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0);
        if now - age > STALE_UPLOAD_SECONDS as f64 {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn begin_upload(instance_dir: &Path, kind: &str, label: &str) -> Result<String, String> {
    let kind = kind.trim().to_lowercase();
    if kind != "archive" && kind != "folder" {
        return Err("Импортировать можно архив или папку мира".to_owned());
    }
    let label: String = label.trim().chars().take(200).collect();
    if kind == "archive" {
        let lowered = label.to_lowercase();
        if UNSUPPORTED_ARCHIVES.iter().any(|suffix| lowered.ends_with(suffix)) {
            return Err("Архивы RAR и 7z не поддерживаются — упакуйте мир в ZIP или выберите папку мира".to_owned());
        }
        if !ARCHIVE_SUFFIXES.iter().any(|suffix| lowered.ends_with(suffix)) {
            return Err("Поддерживаются архивы .zip, .tar, .tar.gz, .tar.bz2 и .tar.xz".to_owned());
        }
    }
    cleanup_stale(instance_dir);
    let root = staging_root(instance_dir)?;
    fs::create_dir_all(&root).map_err(|_| "Каталог импорта миров недоступен".to_owned())?;
    let upload_id = super::snapshots::random_hex(16);
    let directory = root.join(&upload_id);
    fs::create_dir_all(directory.join("payload")).map_err(|_| "Не удалось начать загрузку мира".to_owned())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0);
    let mut meta = Map::new();
    meta.insert("kind".into(), json!(kind));
    meta.insert("label".into(), json!(label));
    meta.insert("bytes".into(), json!(0));
    meta.insert("files".into(), json!(0));
    meta.insert("created".into(), json!(now));
    write_meta(&directory, &meta)?;
    Ok(upload_id)
}

fn chunk_upload(instance_dir: &Path, upload_id: &str, relative_path: &str, data: &[u8], offset: i64) -> Result<Value, String> {
    let directory = upload_dir(instance_dir, upload_id)?;
    let mut meta = read_meta(&directory)?;
    if data.len() > MAX_CHUNK_BYTES {
        return Err("Слишком большой фрагмент загрузки".to_owned());
    }
    let parts = safe_relative_path(relative_path)?;
    let kind = meta.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned();
    if kind == "archive" && parts.len() != 1 {
        return Err("Архив мира передаётся одним файлом".to_owned());
    }
    let payload = directory.join("payload");
    let target = payload.join(parts.join("/"));
    if let Some(parent) = target.parent() {
        let resolved_parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        let resolved_payload = fs::canonicalize(&payload).unwrap_or(payload.clone());
        if !resolved_parent.starts_with(&resolved_payload) {
            return Err("Путь файла выходит за пределы загрузки".to_owned());
        }
    }
    if target.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
        return Err("Файл загрузки является ссылкой".to_owned());
    }
    let exists = target.symlink_metadata().is_ok();
    let current = target.symlink_metadata().as_ref().map(fs::Metadata::len).unwrap_or(0);
    if offset < 0 || offset as u64 != current {
        return Err("Фрагменты мира пришли не по порядку — повторите импорт".to_owned());
    }
    let total = meta_u64(&meta, "bytes").saturating_add(data.len() as u64);
    if total > MAX_UPLOAD_BYTES {
        return Err("Мир слишком большой для импорта (максимум 8 ГиБ)".to_owned());
    }
    let files = meta_u64(&meta, "files") + u64::from(!exists);
    if files > MAX_FILES as u64 {
        return Err("В мире слишком много файлов".to_owned());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|_| "Не удалось сохранить фрагмент мира".to_owned())?;
    }
    let mut handle = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&target)
        .map_err(|_| "Не удалось сохранить фрагмент мира".to_owned())?;
    handle.write_all(data).map_err(|_| "Не удалось сохранить фрагмент мира".to_owned())?;
    meta.insert("bytes".into(), json!(total));
    meta.insert("files".into(), json!(files));
    write_meta(&directory, &meta)?;
    Ok(json!({ "bytes": total, "files": files }))
}

fn cancel_upload(instance_dir: &Path, upload_id: &str) {
    if let Ok(directory) = upload_dir(instance_dir, upload_id) {
        let _ = fs::remove_dir_all(&directory);
    }
}

fn finish_upload(instance_dir: &Path, upload_id: &str) -> Result<Vec<String>, String> {
    let directory = upload_dir(instance_dir, upload_id)?;
    let result = (|| -> Result<Vec<String>, String> {
        let meta = read_meta(&directory)?;
        let payload = directory.join("payload");
        let label = meta.get("label").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        let kind = meta.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned();
        let (root, fallback_name) = if kind == "archive" {
            let mut archives: Vec<PathBuf> = Vec::new();
            if let Ok(entries) = fs::read_dir(&payload) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && !path.is_symlink() {
                        archives.push(path);
                    }
                }
            }
            if archives.len() != 1 {
                return Err("Архив мира не загружен".to_owned());
            }
            let root = directory.join("extracted");
            fs::create_dir(&root).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
            extract_archive(&archives[0], &root)?;
            let name = archives[0]
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| label.clone());
            (root, strip_archive_suffix(&name))
        } else {
            (payload.clone(), if label.is_empty() { "Мир".to_owned() } else { label })
        };
        let worlds = find_world_roots(&root);
        if worlds.is_empty() {
            return Err(
                if kind == "archive" {
                    "Мир Minecraft не найден: внутри нет level.dat. Проверьте, что архив содержит папку сохранения.".to_owned()
                } else {
                    "Мир Minecraft не найден: внутри нет level.dat. Выберите папку конкретного мира из saves/.".to_owned()
                },
            );
        }
        let saves = instance_dir.join("saves");
        if saves.symlink_metadata().is_ok_and(|meta| meta.is_symlink())
            || (saves.exists() && !saves.is_dir())
        {
            return Err("Каталог миров является символьной ссылкой".to_owned());
        }
        fs::create_dir_all(&saves).map_err(|_| "Не удалось открыть каталог миров".to_owned())?;
        let mut imported: Vec<String> = Vec::new();
        for world in worlds {
            let name = if world == root {
                fallback_name.clone()
            } else {
                world.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default()
            };
            let target = unique_world_dir(&saves, &safe_world_name(&name));
            fs::rename(&world, &target).map_err(|error| format!("Не удалось перенести мир в saves/: {error}"))?;
            imported.push(target.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default());
        }
        Ok(imported)
    })();
    let _ = fs::remove_dir_all(&directory);
    result
}

fn strip_archive_suffix(name: &str) -> String {
    let lowered = name.to_lowercase();
    let mut suffixes: Vec<&str> = ARCHIVE_SUFFIXES.to_vec();
    suffixes.sort_by_key(|suffix| std::cmp::Reverse(suffix.len()));
    for suffix in suffixes {
        if lowered.ends_with(suffix) {
            return name[..name.len() - suffix.len()].to_owned();
        }
    }
    name.to_owned()
}

fn is_zip_archive(archive: &Path) -> bool {
    let mut magic = [0u8; 4];
    let Ok(mut handle) = fs::File::open(archive) else { return false };
    handle.read_exact(&mut magic).is_ok() && magic[..2] == *b"PK"
}

fn compression_kind(archive: &Path) -> Result<(&'static str, [u8; 6]), String> {
    let mut magic = [0u8; 6];
    let Ok(mut handle) = fs::File::open(archive) else { return Err("Файл не похож на ZIP или TAR-архив либо повреждён".to_owned()) };
    if handle.read_exact(&mut magic).is_err() {
        return Err("Файл не похож на ZIP или TAR-архив либо повреждён".to_owned());
    }
    if &magic[..2] == b"PK" {
        return Ok(("zip", magic));
    }
    if &magic[..3] == b"BZh" {
        return Ok(("tar.bz2", magic));
    }
    if &magic[..6] == [0xfd, b'7', b'z', b'X', b'Z', 0x00] {
        return Ok(("tar.xz", magic));
    }
    let kind = if &magic[..2] == [0x1f, 0x8b] { "tar.gz" } else { "tar" };
    Ok((kind, magic))
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    if is_zip_archive(archive) {
        return extract_zip(archive, destination);
    }
    let (kind, _) = compression_kind(archive)?;
    extract_tar(archive, destination, kind)
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|_| "Файл не похож на ZIP или TAR-архив либо повреждён".to_owned())?;
    let mut source = zip::ZipArchive::new(file).map_err(|_| "Не удалось распаковать архив: повреждённый ZIP".to_owned())?;
    if source.len() as usize > MAX_FILES {
        return Err("В архиве слишком много файлов".to_owned());
    }
    let mut total: u64 = 0;
    for index in 0..source.len() {
        let mut member = source.by_index(index).map_err(|_| "Не удалось распаковать архив: повреждённый ZIP".to_owned())?;
        let mode = (member.unix_mode().unwrap_or(0) & 0o170000) as u32;
        if mode == 0o120000 {
            return Err("Архив мира содержит символьные ссылки".to_owned());
        }
        let name = member.name().map(|v| v.to_string()).unwrap_or_default();
        let parts = safe_relative_path(&name)?;
        let target = destination.join(parts.join("/"));
        if member.is_dir() {
            fs::create_dir_all(&target).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
            continue;
        }
        let declared = member.size();
        total = total.saturating_add(declared);
        if total > MAX_EXTRACTED_BYTES {
            return Err("Распакованный мир слишком большой".to_owned());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
        }
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|_| "В архиве повторяются файлы".to_owned())?;
        let mut written: u64 = 0;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = member.read(&mut buffer).map_err(|_| "Не удалось распаковать архив: повреждённый ZIP".to_owned())?;
            if read == 0 {
                break;
            }
            written += read as u64;
            // проверяем фактический размер: заголовок ZIP может его занизить
            if written > declared + 1024 || total - declared + written > MAX_EXTRACTED_BYTES {
                return Err("Распакованный мир слишком большой".to_owned());
            }
            writer.write_all(&buffer[..read]).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
        }
    }
    Ok(())
}

/// ограничивает размер распакованных данных XZ, которые буферизуются в памяти
struct CappedWriter {
    buffer: Vec<u8>,
    cap: usize,
}

impl Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buffer.len().saturating_add(data.len()) > self.cap {
            return Err(std::io::Error::other("Распакованный мир слишком большой"));
        }
        self.buffer.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn extract_tar(archive: &Path, destination: &Path, kind: &str) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|_| "Файл не похож на ZIP или TAR-архив либо повреждён".to_owned())?;
    match kind {
        "tar.gz" => {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let entries = archive
                .entries()
                .map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
            extract_tar_entries(entries, destination)
        }
        "tar.bz2" => {
            let mut archive = tar::Archive::new(bzip2_rs::DecoderReader::new(file));
            let entries = archive
                .entries()
                .map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
            extract_tar_entries(entries, destination)
        }
        "tar.xz" => {
            let mut compressed = Vec::new();
            let mut reader = std::io::BufReader::new(file);
            reader
                .read_to_end(&mut compressed)
                .map_err(|_| "Файл не похож на ZIP или TAR-архив либо повреждён".to_owned())?;
            let mut writer = CappedWriter {
                buffer: Vec::new(),
                cap: MAX_EXTRACTED_BYTES.min(u32::MAX as u64) as usize,
            };
            lzma_rs::xz_decompress(&mut reader, &mut writer)
                .map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
            let mut archive = tar::Archive::new(std::io::Cursor::new(writer.buffer));
            let entries = archive
                .entries()
                .map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
            extract_tar_entries(entries, destination)
        }
        _ => {
            let mut archive = tar::Archive::new(file);
            let entries = archive
                .entries()
                .map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
            extract_tar_entries(entries, destination)
        }
    }
}

fn extract_tar_entries<R: Read>(entries: tar::Entries<R>, destination: &Path) -> Result<(), String> {
    let mut total: u64 = 0;
    let mut count: usize = 0;
    let mut entries = entries;
    for entry in entries.by_ref() {
        let mut member = entry.map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
        count += 1;
        if count > MAX_FILES {
            return Err("В архиве слишком много файлов".to_owned());
        }
        let header = member.header();
        let entry_type = header.entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err("Архив мира содержит ссылки".to_owned());
        }
        if !(entry_type.is_dir() || entry_type.is_file()) {
            continue;
        }
        let parts = safe_relative_path(&member.path().map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?.to_string_lossy())?;
        let target = destination.join(parts.join("/"));
        if entry_type.is_dir() {
            fs::create_dir_all(&target).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
            continue;
        }
        let declared = header.size().map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
        total = total.saturating_add(declared);
        if total > MAX_EXTRACTED_BYTES {
            return Err("Распакованный мир слишком большой".to_owned());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
        }
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|_| "В архиве повторяются файлы".to_owned())?;
        let mut written: u64 = 0;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = member.read(&mut buffer).map_err(|_| "Не удалось распаковать архив: повреждённый TAR".to_owned())?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > declared + 1024 || total - declared + written > MAX_EXTRACTED_BYTES {
                return Err("Распакованный мир слишком большой".to_owned());
            }
            writer.write_all(&buffer[..read]).map_err(|_| "Не удалось распаковать архив мира".to_owned())?;
        }
    }
    Ok(())
}


/// проверяет, что запрос относится к клиентскому Java-экземпляру
fn import_instance(request: &Value) -> Result<InstanceInfo, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.kind != "client" || info.game_edition != "java" {
        return Err("Эта рабочая область предназначена только для Minecraft Java Edition".to_owned());
    }
    Ok(info)
}

pub fn world_import_begin(request: &Value) -> Result<Value, String> {
    let info = import_instance(request)?;
    if info.running {
        return Err("Остановите игру перед импортом мира".to_owned());
    }
    let kind = request.get("kind").and_then(Value::as_str).unwrap_or_default();
    let label = request.get("label").and_then(Value::as_str).unwrap_or_default();
    let upload_id = begin_upload(&info.path, kind, label)?;
    Ok(json!({ "upload_id": upload_id, "chunk_bytes": 8 * 1024 * 1024 }))
}

pub fn world_import_chunk(request: &Value) -> Result<Value, String> {
    let info = import_instance(request)?;
    if info.running {
        return Err("Остановите игру перед импортом мира".to_owned());
    }
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or_default();
    if encoded.len() > MAX_ENCODED_CHUNK_BYTES {
        return Err("Слишком большой фрагмент загрузки".to_owned());
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Фрагмент мира повреждён".to_owned())?;
    let offset = match request.get("offset").and_then(Value::as_i64) {
        Some(value) => value,
        None => return Err("Некорректное смещение фрагмента".to_owned()),
    };
    let upload_id = request.get("upload_id").and_then(Value::as_str).unwrap_or_default();
    let relative = request.get("path").and_then(Value::as_str).unwrap_or_default();
    chunk_upload(&info.path, upload_id, relative, &content, offset)
}

pub fn world_import_cancel(request: &Value) -> Result<Value, String> {
    let info = import_instance(request)?;
    let upload_id = request.get("upload_id").and_then(Value::as_str).unwrap_or_default();
    cancel_upload(&info.path, upload_id);
    Ok(json!({ "cancelled": true }))
}

pub fn world_import_finish(request: &Value) -> Result<Value, String> {
    let info = import_instance(request)?;
    if info.running {
        return Err("Остановите игру перед импортом мира".to_owned());
    }
    let upload_id = request.get("upload_id").and_then(Value::as_str).unwrap_or_default();
    let imported = finish_upload(&info.path, upload_id)?;
    super::events::emit_status_event("workspace-changed", &format!("Импортировано миров: {}.", imported.len()));
    let details = super::instance_workspace::InstanceWorkspace::new()?.snapshot(&info.name)?;
    Ok(json!({ "imported": imported, "details": details }))
}

fn copy_tree(source: &Path, staging: &Path) -> Result<PathBuf, String> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    copy_inner(source, staging, &mut files, &mut bytes)?;
    Ok(staging.to_path_buf())
}

fn copy_inner(source: &Path, destination: &Path, files: &mut usize, bytes: &mut u64) -> Result<(), String> {
    let entries = fs::read_dir(source).map_err(|error| format!("Не удалось прочитать {}: {error}", source.display()))?;
    fs::create_dir_all(destination).map_err(|error| format!("Не удалось создать {}: {error}", destination.display()))?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let target = destination.join(&name);
        if path.is_dir() {
            copy_inner(&path, &target, files, bytes)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        *files += 1;
        if *files > MAX_FILES {
            return Err("В мире слишком много файлов".to_owned());
        }
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        *bytes = bytes.saturating_add(size);
        if *bytes > MAX_TOTAL_BYTES {
            return Err("Мир слишком большой для импорта (максимум 32 ГиБ)".to_owned());
        }
        fs::copy(&path, &target).map_err(|error| format!("Не удалось скопировать файл {}: {error}", path.display()))?;
    }
    Ok(())
}

// не вкладываем найденные папки миров друг в друга
fn find_world_roots(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk_worlds(root, 0, &mut found);
    found
}

fn walk_worlds(directory: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let level = directory.join("level.dat");
    if level.is_file() {
        found.push(directory.to_path_buf());
        return;
    }
    if depth >= MAX_SEARCH_DEPTH {
        return;
    }
    let mut children: Vec<PathBuf> = match fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir() && !path.is_symlink())
            .collect(),
        Err(_) => return,
    };
    children.sort();
    for child in children {
        if child.file_name().map(|name| name == "__MACOSX").unwrap_or(false) {
            continue;
        }
        walk_worlds(&child, depth + 1, found);
    }
}

fn safe_world_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c })
        .collect::<String>()
        .replace(['/', '\\'], "_")
        .trim()
        .trim_matches('.')
        .to_owned();
    let upper = cleaned.to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .map(|name| name.to_string())
        .chain((1..10).flat_map(|index| [format!("COM{index}"), format!("LPT{index}")]))
        .any(|name| name == upper);
    let name = if reserved { format!("{cleaned}_") } else { cleaned };
    name.chars().take(80).collect::<String>()
}

fn unique_world_dir(saves: &Path, name: &str) -> PathBuf {
    let mut candidate = saves.join(name);
    let mut index = 2;
    while candidate.symlink_metadata().is_ok() {
        candidate = saves.join(format!("{name} ({index})"));
        index += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_name_is_sanitized() {
        assert_eq!(safe_world_name("Мой мир"), "Мой мир");
        assert_eq!(safe_world_name("bad:name?"), "bad_name_");
        assert_eq!(safe_world_name("con"), "con_");
        assert_eq!(safe_world_name("  ..New World..  "), "New World");
        assert!(safe_world_name(&"x".repeat(200)).len() <= 80);
    }

    #[test]
    fn world_roots_found_by_level_dat() {
        let root = std::env::temp_dir().join(format!("nivora-world-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("world1")).unwrap();
        fs::create_dir_all(root.join("sub/world2")).unwrap();
        fs::write(root.join("world1/level.dat"), b"nbt").unwrap();
        fs::write(root.join("sub/world2/level.dat"), b"nbt").unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();
        let worlds = find_world_roots(&root);
        assert_eq!(worlds.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_path_is_validated() {
        assert_eq!(safe_relative_path("saves/world/level.dat").unwrap(), vec!["saves", "world", "level.dat"]);
        assert_eq!(safe_relative_path("a//b/./c").unwrap(), vec!["a", "b", "c"]);
        assert!(safe_relative_path("../escape").is_err());
        assert!(safe_relative_path("/absolute").is_err());
        assert!(safe_relative_path("C:/win").is_err());
        assert!(safe_relative_path(r"..\..\escape").is_err());
        assert!(safe_relative_path(r"\\server\share\escape").is_err());
        assert!(safe_relative_path("bad<name>").is_err());
        assert!(safe_relative_path("").is_err());
        let deep = vec!["d"; 33].join("/");
        assert!(safe_relative_path(&deep).is_err());
    }

    fn zip_with_world(path: &Path, content: &[u8]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("My World/level.dat", options).unwrap();
        writer.write_all(content).unwrap();
        writer.start_file("My World/level.dat_old", options).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn upload_roundtrip_with_zip_archive() {
        let root = std::env::temp_dir().join(format!("nivora-upload-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(begin_upload(&root, "rar", "x.rar").is_err());
        let upload_id = begin_upload(&root, "archive", "world.zip").unwrap();
        assert_eq!(upload_id.len(), 32);
        let payload = root.join(STAGING_DIRECTORY).join(&upload_id).join("payload");
        zip_with_world(&payload.join("world.zip"), b"nbt-bytes");
        let imported = finish_upload(&root, &upload_id).unwrap();
        assert_eq!(imported, vec!["My World".to_owned()]);
        assert!(root.join("saves/My World/level.dat").is_file());
        assert!(finish_upload(&root, &upload_id).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn upload_chunk_appends_in_order() {
        let root = std::env::temp_dir().join(format!("nivora-chunk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let upload_id = begin_upload(&root, "folder", "").unwrap();
        let first = chunk_upload(&root, &upload_id, "world/level.dat", b"hello ", 0).unwrap();
        assert_eq!(first["bytes"], json!(6));
        assert_eq!(first["files"], json!(1));
        assert!(chunk_upload(&root, &upload_id, "world/level.dat", b"world", 3).is_err());
        let second = chunk_upload(&root, &upload_id, "world/level.dat", b"world", 6).unwrap();
        assert_eq!(second["bytes"], json!(11));
        cancel_upload(&root, &upload_id);
        assert!(chunk_upload(&root, &upload_id, "world/level.dat", b"!", 11).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tar_gz_archive_upload_is_extracted() {
        let root = std::env::temp_dir().join(format!("nivora-tar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let upload_id = begin_upload(&root, "archive", "Cool.World.tar.gz").unwrap();
        let payload = root.join(STAGING_DIRECTORY).join(&upload_id).join("payload");
        let tar_path = payload.join("Cool.World.tar.gz");
        let file = fs::File::create(&tar_path).unwrap();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::default()));
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "Cool World/level.dat", &b"nbt"[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap().flush().unwrap();
        let imported = finish_upload(&root, &upload_id).unwrap();
        assert_eq!(imported, vec!["Cool World".to_owned()]);
        assert!(root.join("saves/Cool World/level.dat").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tar_archive_at_root_uses_file_name() {
        let root = std::env::temp_dir().join(format!("nivora-tar-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let upload_id = begin_upload(&root, "archive", "world.tar").unwrap();
        let payload = root.join(STAGING_DIRECTORY).join(&upload_id).join("payload");
        let tar_path = payload.join("world.tar");
        let file = fs::File::create(&tar_path).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "level.dat", &b"nbt"[..]).unwrap();
        builder.into_inner().unwrap();
        let imported = finish_upload(&root, &upload_id).unwrap();
        assert_eq!(imported, vec!["world".to_owned()]);
        assert!(root.join("saves/world/level.dat").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_upload_ignores_nested_archives() {
        let root = std::env::temp_dir().join(format!("nivora-folder-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let upload_id = begin_upload(&root, "folder", "").unwrap();
        let payload = root.join(STAGING_DIRECTORY).join(&upload_id).join("payload");
        let world = payload.join("Cool World");
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), b"nbt").unwrap();
        fs::write(payload.join("bundle.tar"), b"not really a tar").unwrap();
        let imported = finish_upload(&root, &upload_id).unwrap();
        assert_eq!(imported, vec!["Cool World".to_owned()]);
        assert!(root.join("saves/Cool World/level.dat").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_compression_is_detected_by_magic() {
        let dir = std::env::temp_dir().join(format!("nivora-archive-magic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("w.zip"), b"PK\x03\x04rest").unwrap();
        fs::write(dir.join("w.tar.gz"), [0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00].as_slice()).unwrap();
        fs::write(dir.join("w.tar.bz2"), b"BZh91AY&SY").unwrap();
        fs::write(dir.join("w.tar.xz"), [0xfd, b'7', b'z', b'X', b'Z', 0x00].as_slice()).unwrap();
        assert_eq!(compression_kind(&dir.join("w.zip")).unwrap().0, "zip");
        assert_eq!(compression_kind(&dir.join("w.tar.gz")).unwrap().0, "tar.gz");
        assert_eq!(compression_kind(&dir.join("w.tar.bz2")).unwrap().0, "tar.bz2");
        assert_eq!(compression_kind(&dir.join("w.tar.xz")).unwrap().0, "tar.xz");
        assert_eq!(strip_archive_suffix("My.World.tar.gz"), "My.World");
        assert_eq!(strip_archive_suffix("plain.zip"), "plain");
        let _ = fs::remove_dir_all(&dir);
    }
}
