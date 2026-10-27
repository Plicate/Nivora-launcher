//! снимки игровых экземпляров: создание, список и восстановление

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::platform_support::is_reparse_point;

const SNAPSHOT_DIR: &str = ".trial-launcher-snapshots";
const MAX_SNAPSHOT_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SNAPSHOT_MANIFESTS_SCANNED: usize = 500;
const MAX_SNAPSHOT_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SNAPSHOT_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SNAPSHOT_ARCHIVE_BYTES: u64 = 640 * 1024 * 1024;
const MAX_SNAPSHOT_FILES: usize = 100_000;
const MAX_SNAPSHOTS_LISTED: usize = 50;

/// категории снимков Java: ключ операции и каталог экземпляра
const SNAPSHOT_CATEGORIES: [(&str, &str); 5] = [
    ("mods", "mods"),
    ("plugins", "plugins"),
    ("resourcepacks", "resourcepacks"),
    ("shaders", "shaderpacks"),
    ("saves", "saves"),
];

/// категории снимков Bedrock и соответствующие каталоги контента
const BEDROCK_SNAPSHOT_CATEGORIES: [(&str, &str); 3] = [
    ("resourcepacks", "resource_packs"),
    ("addons", "behavior_packs"),
    ("worlds", "minecraftWorlds"),
];

fn snapshot_id_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"^[0-9]{1,20}-[0-9a-f]{8}$").expect("snapshot id"))
}

fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn random_hex(byte_count: usize) -> String {
    // на Unix используем `/dev/urandom`; иначе смешиваем время и PID
    #[cfg(unix)]
    {
        use std::io::Read as _;
        if let Ok(mut source) = fs::File::open("/dev/urandom") {
            let mut bytes = vec![0u8; byte_count];
            if source.read_exact(&mut bytes).is_ok() {
                return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            }
        }
    }
    let mut seed = epoch_seconds().to_bits() ^ ((std::process::id() as u64) << 32);
    let mut result = String::new();
    while result.len() < byte_count * 2 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        result.push_str(&format!("{:016x}", seed));
    }
    result.truncate(byte_count * 2);
    result
}

/// создаёт данные для перезаписи файла перед удалением
pub(crate) fn random_bytes(len: usize) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::io::Read as _;
        if let Ok(mut source) = fs::File::open("/dev/urandom") {
            let mut bytes = vec![0u8; len];
            if source.read_exact(&mut bytes).is_ok() {
                return bytes;
            }
        }
    }
    let mut seed = epoch_seconds().to_bits() ^ ((std::process::id() as u64) << 32);
    let mut result = Vec::with_capacity(len);
    while result.len() < len {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        result.extend_from_slice(&seed.to_be_bytes());
    }
    result.truncate(len);
    result
}

/// атомарно записывает JSON с правами 0600 в Unix
fn write_private_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог снапшотов: {error}"))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", random_hex(4)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|error| format!("Не удалось создать манифест снапшота: {error}"))?;
    let body = serde_json::to_string_pretty(value).unwrap_or_default();
    file.write_all(format!("{body}\n").as_bytes())
        .and_then(|_| file.flush())
        .map_err(|error| format!("Не удалось создать манифест снапшота: {error}"))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Не удалось создать манифест снапшота: {error}")
    })
}

pub(crate) struct SnapshotStore {
    root: PathBuf,
    categories: &'static [(&'static str, &'static str)],
}

impl SnapshotStore {
    pub(crate) fn new(root: &Path) -> Result<Self, String> {
        if is_reparse_point(root) || !root.is_dir() {
            return Err("Каталог экземпляра недоступен для снапшотов".to_owned());
        }
        Ok(Self { root: root.to_path_buf(), categories: &SNAPSHOT_CATEGORIES })
    }

    pub(crate) fn bedrock(root: &Path) -> Result<Self, String> {
        if is_reparse_point(root) || !root.is_dir() {
            return Err("Каталог экземпляра недоступен для снапшотов".to_owned());
        }
        Ok(Self { root: root.to_path_buf(), categories: &BEDROCK_SNAPSHOT_CATEGORIES })
    }

    fn directory(&self) -> Result<PathBuf, String> {
        let path = self.root.join(SNAPSHOT_DIR);
        if is_reparse_point(&path) {
            return Err("Каталог снапшотов является ссылкой или junction".to_owned());
        }
        fs::create_dir_all(&path).map_err(|error| format!("Каталог снапшотов недоступен: {error}"))?;
        Ok(path)
    }

    fn target(&self, category: &str) -> Result<PathBuf, String> {
        let relative = self
            .categories
            .iter()
            .find(|(key, _)| *key == category)
            .map(|(_, value)| *value)
            .ok_or_else(|| "Неизвестная категория снапшота".to_owned())?;
        let target = self.root.join(relative);
        if is_reparse_point(&target) {
            return Err("Каталог снапшота является ссылкой или junction".to_owned());
        }
        Ok(target)
    }

    pub(crate) fn list(&self, category: &str) -> Result<Vec<Value>, String> {
        let target = self.target(category)?;
        let target_name = target.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
        let directory = self.directory()?;
        let mut candidates: Vec<(std::time::SystemTime, String)> = Vec::new();
        let entries = fs::read_dir(&directory).map_err(|_| "Каталог снапшотов недоступен".to_owned())?;
        let mut scanned = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&format!("{category}-")) || !name.ends_with(".json") {
                continue;
            }
            scanned += 1;
            if scanned > MAX_SNAPSHOT_MANIFESTS_SCANNED {
                break;
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
            if !meta.is_file() || meta.len() > MAX_SNAPSHOT_MANIFEST_BYTES {
                continue;
            }
            let Ok(modified) = meta.modified() else { continue };
            candidates.push((modified, name));
        }
        candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        let mut values: Vec<Value> = Vec::new();
        for (_, manifest_name) in candidates {
            let Ok(raw) = fs::read_to_string(directory.join(&manifest_name)) else { continue };
            let Ok(Value::Object(mut value)) = serde_json::from_str::<Value>(&raw) else { continue };
            let snapshot_id = value.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let archive_name = value.get("archive").and_then(Value::as_str).unwrap_or_default().to_owned();
            if value.get("category").and_then(Value::as_str) != Some(category)
                || !snapshot_id_pattern().is_match(&snapshot_id)
                || archive_name != format!("{category}-{snapshot_id}.zip")
            {
                continue;
            }
            let archive = directory.join(&archive_name);
            if is_reparse_point(&archive) || !archive.is_file() {
                continue;
            }
            value.insert("archive".to_owned(), json!(archive_name));
            value.insert("category".to_owned(), json!(category));
            value.insert("target".to_owned(), json!(target_name));
            values.push(Value::Object(value));
            if values.len() >= MAX_SNAPSHOTS_LISTED {
                break;
            }
        }
        Ok(values)
    }

    pub(crate) fn create(&self, category: &str, retention_hours: i64, max_count: i64) -> Result<Map<String, Value>, String> {
        let target = self.target(category)?;
        let snapshot_id = format!("{}-{}", epoch_seconds() as i64, random_hex(4));
        let archive_name = format!("{category}-{snapshot_id}.zip");
        let directory = self.directory()?;
        let archive_path = directory.join(&archive_name);
        if is_reparse_point(&archive_path) {
            return Err("Путь снапшота является ссылкой или junction".to_owned());
        }
        if is_reparse_point(&target) {
            return Err("Каталог снапшота является ссылкой или junction".to_owned());
        }
        // пустой каталог сохраняется как пустой ZIP-архив
        let (file_count, total_size) = match write_directory_archive(&target, &archive_path) {
            Ok(result) => result,
            Err(error) => {
                let _ = fs::remove_file(&archive_path);
                return Err(error);
            }
        };
        let mut manifest = Map::new();
        manifest.insert("id".to_owned(), json!(snapshot_id));
        manifest.insert("category".to_owned(), json!(category));
        manifest.insert("archive".to_owned(), json!(archive_name));
        manifest.insert("created_at".to_owned(), json!(epoch_seconds()));
        manifest.insert("file_count".to_owned(), json!(file_count));
        manifest.insert("source_bytes".to_owned(), json!(total_size));
        manifest.insert("retention_hours".to_owned(), json!(retention_hours.clamp(1, 720)));
        if let Err(error) = write_private_json(&directory.join(format!("{category}-{snapshot_id}.json")), &Value::Object(manifest.clone())) {
            let _ = fs::remove_file(&archive_path);
            return Err(error);
        }
        let retention = manifest.get("retention_hours").and_then(Value::as_i64).unwrap_or(retention_hours);
        self.cleanup(category, retention, max_count);
        Ok(manifest)
    }

    /// распаковывает снимок во временный каталог и заменяет целевую папку
    pub(crate) fn restore(&self, category: &str, snapshot_id: &str) -> Result<Map<String, Value>, String> {
        let value = self
            .list(category)?
            .into_iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(snapshot_id))
            .ok_or_else(|| "Снапшот не найден".to_owned())?;
        let archive_path = self.archive_path(category, &value)?;
        let target = self.target(category)?;
        let temporary = self.root.join(format!(".{category}-restore-{}.part", random_hex(8)));
        if is_reparse_point(&temporary) || temporary.exists() {
            return Err("Временный каталог восстановления уже существует".to_owned());
        }
        fs::create_dir(&temporary).map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
        let result = (|| -> Result<Map<String, Value>, String> {
            extract_archive(&archive_path, &temporary)?;
            replace_directory_contents(&self.root, &target, &temporary)?;
            let mut restored = value.as_object().cloned().unwrap_or_default();
            restored.insert("restored_at".to_owned(), json!(epoch_seconds()));
            Ok(restored)
        })();
        let _ = fs::remove_dir_all(&temporary);
        result
    }

    pub(crate) fn delete(&self, category: &str, snapshot_id: &str) -> bool {
        let Some(value) = self
            .list(category)
            .unwrap_or_default()
            .into_iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(snapshot_id))
        else {
            return false;
        };
        let Ok(archive_path) = self.archive_path(category, &value) else {
            return false;
        };
        for path in [archive_path, self.root.join(SNAPSHOT_DIR).join(format!("{category}-{snapshot_id}.json"))] {
            if is_reparse_point(&path) {
                return false;
            }
            let _ = fs::remove_file(path);
        }
        true
    }

    fn archive_path(&self, category: &str, value: &Value) -> Result<PathBuf, String> {
        let snapshot_id = value.get("id").and_then(Value::as_str).unwrap_or_default();
        let archive_name = value.get("archive").and_then(Value::as_str).unwrap_or_default();
        let expected = format!("{category}-{snapshot_id}.zip");
        if value.get("category").and_then(Value::as_str) != Some(category)
            || !snapshot_id_pattern().is_match(snapshot_id)
            || archive_name != expected
        {
            return Err("Метаданные снапшота повреждены".to_owned());
        }
        let archive = self.root.join(SNAPSHOT_DIR).join(archive_name);
        if is_reparse_point(&archive) || !archive.is_file() {
            return Err("Архив снапшота недоступен".to_owned());
        }
        Ok(archive)
    }

    fn cleanup(&self, category: &str, retention_hours: i64, max_count: i64) {
        let now = epoch_seconds();
        let limit = max_count.clamp(1, 50) as usize;
        for (index, value) in self.list(category).unwrap_or_default().iter().enumerate() {
            let created = value.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
            let expired = created > 0.0 && now - created > retention_hours.max(1) as f64 * 3600.0;
            if expired || index >= limit {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    self.delete(category, id);
                }
            }
        }
    }
}

/// создаёт ZIP без ссылок, с ограничениями размера и количества файлов
fn write_directory_archive(target: &Path, archive_path: &Path) -> Result<(i64, i64), String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)
        .map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut file_count = 0i64;
    let mut total_size = 0i64;
    let mut failure: Option<String> = None;
    visit_directory(target, &mut archive, options, &PathBuf::new(), &mut file_count, &mut total_size, &mut failure);
    match archive.finish() {
        Ok(_) => {}
        Err(error) => {
            let _ = fs::remove_file(archive_path);
            return Err(format!("Не удалось создать снапшот: {error}"));
        }
    }
    if let Some(error) = failure {
        let _ = fs::remove_file(archive_path);
        return Err(error);
    }
    Ok((file_count, total_size))
}

#[allow(clippy::too_many_arguments)]
fn visit_directory(
    directory: &Path,
    archive: &mut ZipWriter<fs::File>,
    options: SimpleFileOptions,
    prefix: &Path,
    file_count: &mut i64,
    total_size: &mut i64,
    failure: &mut Option<String>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            *failure = Some(format!("Не удалось прочитать каталог снапшота: {error}"));
            return;
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => {
                *failure = Some(format!("Не удалось прочитать каталог снапшота: {error}"));
                return;
            }
        }
    }
    paths.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    for path in paths {
        if failure.is_some() {
            return;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) => {
                *failure = Some(format!("Не удалось прочитать файл снапшота: {error}"));
                return;
            }
        };
        if is_reparse_point(&path) {
            *failure = Some("Снапшот не создан: обнаружена ссылка или junction".to_owned());
            return;
        }
        let relative = prefix.join(path.file_name().unwrap_or_default());
        if meta.is_dir() {
            visit_directory(&path, archive, options, &relative, file_count, total_size, failure);
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if *file_count as usize >= MAX_SNAPSHOT_FILES {
            *failure = Some("В снапшоте слишком много файлов".to_owned());
            return;
        }
        if meta.len() > MAX_SNAPSHOT_FILE_BYTES {
            *failure = Some("Файл в снапшоте превышает лимит 256 MiB".to_owned());
            return;
        }
        let mut source = match fs::File::open(&path) {
            Ok(source) => source,
            Err(error) => {
                *failure = Some(format!("Не удалось прочитать файл снапшота: {error}"));
                return;
            }
        };
        let name = relative.to_string_lossy().replace('\\', "/");
        if archive.start_file(name, options).is_err() {
            *failure = Some("Не удалось создать снапшот: архив недоступен".to_owned());
            return;
        }
        let mut written = 0u64;
        let mut chunk = vec![0u8; 1024 * 1024];
        loop {
            let read = match source.read(&mut chunk) {
                Ok(value) => value,
                Err(error) => {
                    *failure = Some(format!("Не удалось создать снапшот: {error}"));
                    return;
                }
            };
            if read == 0 {
                break;
            }
            written += read as u64;
            *total_size += read as i64;
            if written > MAX_SNAPSHOT_FILE_BYTES || *total_size as u64 > MAX_SNAPSHOT_UNPACKED_BYTES {
                *failure = Some("Распакованный снапшот слишком большой".to_owned());
                return;
            }
            if archive.write_all(&chunk[..read]).is_err() {
                *failure = Some("Не удалось создать снапшот: архив недоступен".to_owned());
                return;
            }
        }
        *file_count += 1;
    }
}

/// распаковывает снимок, отклоняя ссылки, повторные пути и превышение лимитов
fn extract_archive(archive_path: &Path, temporary: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(archive_path).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
    if !metadata.is_file() || is_reparse_point(archive_path) || metadata.len() > MAX_SNAPSHOT_ARCHIVE_BYTES {
        return Err("Архив снапшота слишком большой или недоступен".to_owned());
    }
    let file = fs::File::open(archive_path).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
    if archive.len() > MAX_SNAPSHOT_FILES {
        return Err("В снапшоте слишком много файлов".to_owned());
    }
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut declared_total: u64 = 0;
    let mut actual_total: u64 = 0;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
        // zip 9.x: is_symlink() опирается на ffi-константы и не срабатывает,
        // поэтому тип файла определяем по старшим битам unix_mode напрямую
        if member
            .unix_mode()
            .is_some_and(|mode| (mode & 0o170000) == 0o120000)
        {
            return Err("Снапшот содержит символьную ссылку".to_owned());
        }
        let raw_name = member.name().map(|v| v.to_string()).unwrap_or_default();
        let parts = super::world_import::safe_relative_path(&raw_name)?;
        let normalized = parts.join("/");
        if !seen.insert(normalized.to_lowercase()) {
            return Err("Снапшот содержит повторяющийся путь".to_owned());
        }
        let mut destination = temporary.to_path_buf();
        for part in &parts {
            destination.push(part);
        }
        if member.is_dir() {
            fs::create_dir_all(&destination).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
            continue;
        }
        let declared = member.size();
        if declared > MAX_SNAPSHOT_FILE_BYTES {
            return Err("Файл в снапшоте превышает лимит 256 MiB".to_owned());
        }
        declared_total = declared_total.saturating_add(declared);
        if declared_total > MAX_SNAPSHOT_UNPACKED_BYTES {
            return Err("Распакованный снапшот слишком большой".to_owned());
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
        }
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
        let mut written: u64 = 0;
        let mut chunk = vec![0u8; 1024 * 1024];
        loop {
            let read = member.read(&mut chunk).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
            if read == 0 {
                break;
            }
            written = written.saturating_add(read as u64);
            actual_total = actual_total.saturating_add(read as u64);
            if written > MAX_SNAPSHOT_FILE_BYTES || actual_total > MAX_SNAPSHOT_UNPACKED_BYTES {
                return Err("Распакованный снапшот слишком большой".to_owned());
            }
            target.write_all(&chunk[..read]).map_err(|error| format!("Не удалось открыть снапшот: {error}"))?;
        }
        if written != declared {
            return Err("Снапшот содержит повреждённый файл".to_owned());
        }
    }
    Ok(())
}

/// проверяет пути и заменяет целевую папку содержимым временного каталога
fn replace_directory_contents(root: &Path, target: &Path, temporary: &Path) -> Result<(), String> {
    let safe_target = target.strip_prefix(root).map_err(|_| "Путь восстановления вне экземпляра".to_owned())?;
    let canonical_root = root.canonicalize().map_err(|error| format!("Каталог экземпляра недоступен: {error}"))?;
    let target = canonical_root.join(safe_target);
    let parent = target.parent().ok_or_else(|| "Каталог восстановления снапшота недоступен".to_owned())?;
    let parent = parent.canonicalize().map_err(|error| format!("Каталог восстановления снапшота недоступен: {error}"))?;
    if !parent.starts_with(&canonical_root) || is_reparse_point(&temporary) {
        return Err("Путь восстановления вне экземпляра".to_owned());
    }
    let target = parent.join(target.file_name().ok_or_else(|| "Каталог восстановления снапшота недоступен".to_owned())?);
    let temporary = temporary.canonicalize().map_err(|error| format!("Каталог восстановления снапшота недоступен: {error}"))?;
    if !temporary.starts_with(&canonical_root) || !temporary.is_dir() || is_reparse_point(&target) {
        return Err("Каталог восстановления снапшота недоступен".to_owned());
    }
    let target_exists = match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
        Ok(_) => return Err("Целевая папка восстановления недоступна".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("Каталог восстановления снапшота недоступен: {error}")),
    };
    if !target_exists {
        return fs::rename(&temporary, &target).map_err(|error| format!("Не удалось восстановить снапшот: {error}"));
    }
    let name = target.file_name().and_then(|value| value.to_str()).unwrap_or("snapshot");
    let backup = parent.join(format!(".{name}-restore-backup-{}", random_hex(8)));
    if is_reparse_point(&backup) || backup.exists() {
        return Err("Каталог резервной копии уже существует".to_owned());
    }
    fs::rename(&target, &backup).map_err(|error| format!("Не удалось подготовить восстановление снапшота: {error}"))?;
    if let Err(error) = fs::rename(&temporary, &target) {
        return match fs::rename(&backup, &target) {
            Ok(()) => Err(format!("Не удалось восстановить снапшот: {error}")),
            Err(rollback_error) => Err(format!(
                "Не удалось восстановить снапшот ({error}); исходная папка сохранена в {} ({rollback_error})",
                backup.display()
            )),
        };
    }
    fs::remove_dir_all(&backup).map_err(|error| {
        format!("Снапшот восстановлен, но резервная папка осталась в {}: {error}", backup.display())
    })
}


pub(crate) fn store_for(name: &str) -> Result<SnapshotStore, String> {
    let config = super::config::ConfigStore::load()?;
    let store = super::instances::InstanceStore::new(&config)?;
    let info = store.get(name)?;
    if info.game_edition == "bedrock" {
        let root = super::bedrock::snapshot_root_for(&info)?;
        SnapshotStore::bedrock(&root)
    } else {
        SnapshotStore::new(&info.path)
    }
}


pub(crate) fn instance_snapshot_list(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let category = request.get("category").and_then(Value::as_str).unwrap_or("mods");
    let store = store_for(name)?;
    Ok(json!({"category": category, "snapshots": store.list(category)?}))
}

pub(crate) fn instance_snapshot_create(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let category = request.get("category").and_then(Value::as_str).unwrap_or("mods");
    let store = store_for(name)?;
    let snapshot = store.create(category, 168, 20)?;
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    Ok(json!({
        "snapshot": snapshot,
        "snapshots": store.list(category)?,
        "details": workspace.snapshot(name)?,
    }))
}

pub(crate) fn instance_snapshot_restore(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let category = request.get("category").and_then(Value::as_str).unwrap_or("mods");
    let snapshot_id = request.get("snapshot_id").and_then(Value::as_str).unwrap_or_default();
    let config = super::config::ConfigStore::load()?;
    let store_instances = super::instances::InstanceStore::new(&config)?;
    let info = store_instances.get(name)?;
    if info.running {
        return Err("Остановите экземпляр перед восстановлением снапшота".to_owned());
    }
    let store = store_for(name)?;
    let restored = store.restore(category, snapshot_id)?;
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    Ok(json!({
        "restored": restored,
        "snapshots": store.list(category)?,
        "details": workspace.snapshot(name)?,
    }))
}

pub(crate) fn instance_snapshot_delete(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let category = request.get("category").and_then(Value::as_str).unwrap_or("mods");
    let snapshot_id = request.get("snapshot_id").and_then(Value::as_str).unwrap_or_default();
    let store = store_for(name)?;
    let removed = store.delete(category, snapshot_id);
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    Ok(json!({
        "removed": removed,
        "snapshots": store.list(category)?,
        "details": workspace.snapshot(name)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("nivora-snapshots-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("mods")).expect("создать каталог");
        directory
    }

    #[test]
    fn create_list_restore_delete_roundtrip() {
        let root = temp_root("roundtrip");
        fs::write(root.join("mods/a.jar"), b"aaa").unwrap();
        fs::write(root.join("mods/b.jar"), b"bbb").unwrap();
        let store = SnapshotStore::new(&root).unwrap();

        let snapshot = store.create("mods", 168, 20).unwrap();
        assert_eq!(snapshot["file_count"], json!(2));
        assert!(snapshot["source_bytes"].as_i64().unwrap() > 0);
        let listed = store.list("mods").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], snapshot["id"]);
        assert_eq!(listed[0]["target"], json!("mods"));

        fs::remove_file(root.join("mods/a.jar")).unwrap();
        fs::write(root.join("mods/c.jar"), b"ccc").unwrap();
        let id = snapshot["id"].as_str().unwrap().to_owned();
        let restored = store.restore("mods", &id).unwrap();
        assert_eq!(restored["file_count"], json!(2));
        let mut names: Vec<String> = fs::read_dir(root.join("mods"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a.jar", "b.jar"]);

        assert!(store.delete("mods", &id));
        assert!(store.list("mods").unwrap().is_empty());
        assert!(!store.delete("mods", &id));
        assert!(!root.join(".trial-launcher-snapshots").join(format!("mods-{id}.zip")).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn create_rejects_symlinks_without_leaving_partial_archive() {
        use std::os::unix::fs::symlink;

        let root = temp_root("create-symlink");
        fs::write(root.join("outside.jar"), b"outside").unwrap();
        symlink(root.join("outside.jar"), root.join("mods/link.jar")).unwrap();
        let store = SnapshotStore::new(&root).unwrap();
        assert!(store.create("mods", 168, 20).is_err());
        assert!(store.list("mods").unwrap().is_empty());
        assert_eq!(fs::read_dir(root.join(SNAPSHOT_DIR)).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn create_missing_directory_makes_empty_snapshot() {
        let root = temp_root("empty");
        let store = SnapshotStore::new(&root).unwrap();
        let snapshot = store.create("shaders", 5, 1).unwrap();
        assert_eq!(snapshot["file_count"], json!(0));
        assert_eq!(snapshot["source_bytes"], json!(0));
        assert_eq!(store.list("shaders").unwrap()[0]["target"], json!("shaderpacks"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cleanup_keeps_recent_and_drops_expired() {
        let root = temp_root("cleanup");
        let store = SnapshotStore::new(&root).unwrap();
        fs::write(root.join("mods/a.jar"), b"a").unwrap();
        store.create("mods", 1, 20).unwrap();
        store.create("mods", 1, 20).unwrap();
        assert_eq!(store.list("mods").unwrap().len(), 2);
        let directory = root.join(SNAPSHOT_DIR);
        let mut manifests: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().map(|value| value == "json").unwrap_or(false))
            .collect();
        manifests.sort();
        let mut stale = serde_json::from_str::<Value>(&fs::read_to_string(&manifests[0]).unwrap()).unwrap();
        let stale_id = stale["id"].as_str().unwrap().to_owned();
        stale["created_at"] = json!(epoch_seconds() - 7200.0);
        fs::write(&manifests[0], serde_json::to_string(&stale).unwrap()).unwrap();
        store.create("mods", 1, 20).unwrap();
        let ids: Vec<String> = store
            .list("mods")
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_owned())
            .collect();
        assert!(!ids.contains(&stale_id), "просроченный снапшот удаляется чисткой");
        assert_eq!(ids.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_rejects_duplicate_paths() {
        let root = temp_root("extract-duplicate");
        let archive_path = root.join("evil.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive.start_file("a.txt", options).unwrap();
        archive.write_all(b"x").unwrap();
        archive.start_file("./a.txt", options).unwrap();
        archive.write_all(b"y").unwrap();
        archive.finish().unwrap();
        let temporary = root.join("tmp");
        fs::create_dir(&temporary).unwrap();
        assert_eq!(extract_archive(&archive_path, &temporary).unwrap_err(), "Снапшот содержит повторяющийся путь");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_rejects_windows_backslash_traversal() {
        let root = temp_root("extract-traversal");
        let archive_path = root.join("evil.zip");
        let escaped_name = format!("nivora-snapshot-escape-{}", std::process::id());
        let outside = root.parent().unwrap().join(&escaped_name);
        let _ = fs::remove_file(&outside);
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive.start_file(format!(r"..\..\{escaped_name}"), SimpleFileOptions::default()).unwrap();
        archive.write_all(b"escape").unwrap();
        archive.finish().unwrap();
        let temporary = root.join("tmp");
        fs::create_dir(&temporary).unwrap();
        assert!(extract_archive(&archive_path, &temporary).is_err());
        assert!(!outside.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_rejects_symlinks() {
        let root = temp_root("extract-symlink");
        let archive_path = root.join("evil.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        // unix_permissions() отбрасывает старшие биты, поэтому симлинк
        // создаётся штатным add_symlink — иначе тест ничего не проверял
        archive
            .add_symlink("link", "../outside", SimpleFileOptions::default())
            .unwrap();
        archive.finish().unwrap();
        let temporary = root.join("tmp");
        fs::create_dir(&temporary).unwrap();
        assert!(extract_archive(&archive_path, &temporary).unwrap_err().contains("символьную ссылку"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_restore_preparation_keeps_existing_files() {
        let root = temp_root("restore-rollback");
        let target = root.join("mods");
        fs::write(target.join("original.jar"), b"original").unwrap();
        let missing = root.join("missing-temp");
        assert!(replace_directory_contents(&root, &target, &missing).is_err());
        assert_eq!(fs::read(target.join("original.jar")).unwrap(), b"original");
        let _ = fs::remove_dir_all(&root);
    }
}
