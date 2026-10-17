// Снапшоты содержимого экземпляра: ZIP-архивы категорий (mods, saves, …)
// с json-манифестами в .nivora-launcher-snapshots. Лимиты защищают от
// zip-бомб и путей наружу каталога.
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::CompressionMethod;
use zip::ZipWriter;

use super::net::atomic_json_write;
use super::state::read_bounded_json;

const SNAPSHOT_DIR: &str = ".nivora-launcher-snapshots";
pub(crate) const SNAPSHOT_RETENTION_HOURS: i64 = 5;
pub(crate) const SNAPSHOT_MAX_COUNT: i64 = 1;
const SNAPSHOT_CATEGORIES: &[(&str, &str)] = &[
    ("mods", "mods"),
    ("plugins", "plugins"),
    ("resourcepacks", "resourcepacks"),
    ("shaders", "shaderpacks"),
    ("saves", "saves"),
];
const SNAPSHOT_MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const SNAPSHOT_MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SNAPSHOT_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SNAPSHOT_MANIFESTS_SCANNED: usize = 500;

fn snapshot_id_re() -> &'static Regex {
    static SNAPSHOT_ID_RE: OnceLock<Regex> = OnceLock::new();
    SNAPSHOT_ID_RE.get_or_init(|| Regex::new(r"^[0-9]{1,20}-[0-9a-f]{8}\z").unwrap())
}

#[derive(Debug)]
pub(crate) struct SnapshotStore {
    root: PathBuf,
}

impl SnapshotStore {
    pub(crate) fn new(instance_root: &Path) -> Result<Self, String> {
        if is_symlink(instance_root) || !instance_root.is_dir() {
            return Err("Каталог экземпляра недоступен для снапшотов".to_owned());
        }
        Ok(Self { root: instance_root.to_path_buf() })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    fn directory(&self) -> Result<PathBuf, String> {
        let path = self.root.join(SNAPSHOT_DIR);
        if is_symlink(&path) {
            return Err("Каталог снапшотов является ссылкой или junction".to_owned());
        }
        fs::create_dir_all(&path).map_err(|error| format!("{error}"))?;
        super::platform::restrict_dir(&path);
        Ok(path)
    }

    /// Манифесты свежими первыми; каждой записи дозаписываются archive,
    /// category и target.
    pub(crate) fn list(&self, category: &str) -> Result<Vec<Value>, String> {
        let target = self.target(category)?;
        let target_name = file_name_of(&target);
        let directory = self.directory()?;
        let prefix = format!("{category}-");
        let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
        let mut scanned: usize = 0;
        let entries = fs::read_dir(&directory).map_err(|error| format!("{error}"))?;
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(&prefix) || !name.ends_with(".json") {
                continue;
            }
            scanned += 1;
            if scanned > MAX_SNAPSHOT_MANIFESTS_SCANNED {
                break;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_SNAPSHOT_MANIFEST_BYTES {
                continue;
            }
            candidates.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), path));
        }
        candidates.sort_by(|left, right| right.cmp(left));
        let mut values: Vec<Value> = Vec::new();
        for (_, manifest_path) in candidates {
            let Some(fields) = read_bounded_json(&manifest_path, MAX_SNAPSHOT_MANIFEST_BYTES)
                .and_then(|value| value.as_object().cloned())
            else {
                continue;
            };
            let snapshot_id = fields.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let archive_name = fields.get("archive").and_then(Value::as_str).unwrap_or_default().to_owned();
            if fields.get("category").and_then(Value::as_str) != Some(category)
                || !snapshot_id_re().is_match(&snapshot_id)
                || archive_name != format!("{category}-{snapshot_id}.zip")
            {
                continue;
            }
            let archive = directory.join(&archive_name);
            // Сравнение родителя отсекает «../outside.zip» и вложенные пути.
            if archive.parent() != Some(directory.as_path()) || is_symlink(&archive) || !archive.is_file() {
                continue;
            }
            let mut merged = fields;
            merged.insert("archive".into(), json!(archive_name));
            merged.insert("category".into(), json!(category));
            merged.insert("target".into(), json!(target_name));
            values.push(Value::Object(merged));
            if values.len() >= 50 {
                break;
            }
        }
        Ok(values)
    }

    pub(crate) fn create(&self, category: &str, retention_hours: i64, max_count: i64) -> Result<Value, String> {
        let target = self.target(category)?;
        let snapshot_id = format!("{}-{:08x}", epoch_seconds() as i64, unique_suffix());
        let archive_name = format!("{category}-{snapshot_id}.zip");
        let archive_path = self.directory()?.join(&archive_name);
        if is_symlink(&archive_path) {
            return Err("Путь снапшота является ссылкой или junction".to_owned());
        }
        let (file_count, total_size) = match write_directory_archive(&target, &archive_path) {
            Ok(values) => values,
            Err(error) => {
                let _ = fs::remove_file(&archive_path);
                return Err(error);
            }
        };
        let manifest = json!({
            "id": snapshot_id,
            "category": category,
            "archive": archive_name,
            "created_at": epoch_seconds(),
            "file_count": file_count,
            "source_bytes": total_size,
            "retention_hours": retention_hours.clamp(1, 720),
        });
        let manifest_path = self.directory()?.join(format!("{category}-{snapshot_id}.json"));
        atomic_json_write(&manifest_path, &manifest, true)?;
        let retention = manifest.get("retention_hours").and_then(Value::as_i64).unwrap_or(SNAPSHOT_RETENTION_HOURS);
        self.cleanup(category, retention, max_count)?;
        Ok(manifest)
    }

    pub(crate) fn restore(&self, category: &str, snapshot_id: &str) -> Result<Value, String> {
        let value = self
            .list(category)?
            .into_iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(snapshot_id))
            .ok_or_else(|| "Снапшот не найден".to_owned())?;
        let archive_path = self.archive_path(category, &value)?;
        let target = self.target(category)?;
        let temporary = self.root.join(format!(".{category}-restore-{:08x}.part", unique_suffix()));
        let outcome = (|| -> Result<Value, String> {
            fs::create_dir_all(&temporary).map_err(|error| format!("{error}"))?;
            super::platform::restrict_dir(&temporary);
            extract_archive(&archive_path, &temporary)?;
            replace_directory_contents(&self.root, &target, &temporary)?;
            let mut restored = value;
            restored["restored_at"] = json!(epoch_seconds());
            Ok(restored)
        })();
        let _ = remove_tree(&temporary);
        outcome
    }

    pub(crate) fn delete(&self, category: &str, snapshot_id: &str) -> Result<bool, String> {
        let Some(value) = self
            .list(category)?
            .into_iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(snapshot_id))
        else {
            return Ok(false);
        };
        let archive_path = self.archive_path(category, &value)?;
        let names = [
            file_name_of(&archive_path),
            format!("{category}-{snapshot_id}.json"),
        ];
        for name in names {
            let path = self.directory()?.join(name);
            if is_symlink(&path) {
                return Err("Файл снапшота является символьной ссылкой".to_owned());
            }
            let _ = fs::remove_file(&path);
        }
        Ok(true)
    }

    pub(crate) fn latest(&self, category: &str) -> Result<Option<Value>, String> {
        Ok(self.list(category)?.into_iter().next())
    }

    fn target(&self, category: &str) -> Result<PathBuf, String> {
        let relative = SNAPSHOT_CATEGORIES
            .iter()
            .find(|(key, _)| *key == category)
            .map(|(_, value)| *value)
            .ok_or_else(|| "Неизвестная категория снапшота".to_owned())?;
        let target = self.root.join(relative);
        if is_symlink(&target) {
            return Err("Каталог снапшота является ссылкой или junction".to_owned());
        }
        Ok(target)
    }

    fn archive_path(&self, category: &str, value: &Value) -> Result<PathBuf, String> {
        let snapshot_id = value.get("id").and_then(Value::as_str).unwrap_or_default();
        let archive_name = value.get("archive").and_then(Value::as_str).unwrap_or_default();
        let expected = format!("{category}-{snapshot_id}.zip");
        if value.get("category").and_then(Value::as_str) != Some(category)
            || !snapshot_id_re().is_match(snapshot_id)
            || archive_name != expected
        {
            return Err("Метаданные снапшота повреждены".to_owned());
        }
        let directory = self.directory()?;
        let archive = directory.join(archive_name);
        if archive.parent() != Some(directory.as_path()) || is_symlink(&archive) || !archive.is_file() {
            return Err("Архив снапшота недоступен".to_owned());
        }
        Ok(archive)
    }

    /// Удаление просроченных и лишних снапшотов; список берём один раз,
    /// порядок list() — от свежих к старым.
    fn cleanup(&self, category: &str, retention_hours: i64, max_count: i64) -> Result<(), String> {
        let now = epoch_seconds();
        let values = self.list(category)?;
        let limit = max_count.clamp(1, 50) as usize;
        for (index, value) in values.iter().enumerate() {
            let created = value.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
            let expired = created > 0.0 && now - created > (retention_hours.max(1) as f64) * 3600.0;
            if expired || index >= limit {
                let snapshot_id = value.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
                self.delete(category, &snapshot_id)?;
            }
        }
        Ok(())
    }
}

fn write_directory_archive(target: &Path, archive_path: &Path) -> Result<(usize, u64), String> {
    let file = File::create(archive_path).map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
    let mut writer = ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6));
    let mut state = (0usize, 0u64);
    if is_symlink(target) {
        return Err("Каталог снапшота является ссылкой или junction".to_owned());
    }
    if target.exists() {
        visit_directory(target, "", &mut writer, options, &mut state)?;
    }
    writer.finish().map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
    Ok(state)
}

fn visit_directory(
    directory: &Path,
    prefix: &str,
    writer: &mut ZipWriter<File>,
    options: zip::write::SimpleFileOptions,
    state: &mut (usize, u64),
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map_err(|error| format!("Не удалось создать снапшот: {error}"))?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        if meta.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let relative = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if meta.is_dir() {
            visit_directory(&entry.path(), &relative, writer, options, state)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if meta.len() > SNAPSHOT_MAX_FILE_BYTES {
            return Err("Файл в снапшоте превышает лимит 256 MiB".to_owned());
        }
        writer
            .start_file(relative, options)
            .map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
        let mut source = File::open(entry.path()).map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
        let mut written: u64 = 0;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let count = source
                .read(&mut buffer)
                .map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
            if count == 0 {
                break;
            }
            written += count as u64;
            state.1 += count as u64;
            if written > SNAPSHOT_MAX_FILE_BYTES || state.1 > SNAPSHOT_MAX_UNPACKED_BYTES {
                return Err("Распакованный снапшот слишком большой".to_owned());
            }
            writer
                .write_all(&buffer[..count])
                .map_err(|error| format!("Не удалось создать снапшот: {error}"))?;
        }
        state.0 += 1;
    }
    Ok(())
}

fn extract_archive(archive_path: &Path, temporary: &Path) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|_| "Архив снапшота повреждён".to_owned())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "Архив снапшота повреждён".to_owned())?;
    let mut seen: HashSet<String> = HashSet::new();
    // Заявленные и фактические байты копим раздельно: заголовок может врать.
    let mut declared_total: u64 = 0;
    let mut written_total: u64 = 0;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).map_err(|_| "Архив снапшота повреждён".to_owned())?;
        let parts = archive_parts(member.name())?;
        let normalized = parts.join("/");
        if !seen.insert(normalized) {
            return Err("Снапшот содержит повторяющийся путь".to_owned());
        }
        if member.is_symlink() {
            return Err("Снапшот содержит символьную ссылку".to_owned());
        }
        let relative: PathBuf = parts.iter().collect();
        let destination = temporary.join(relative);
        if member.is_dir() {
            fs::create_dir_all(&destination).map_err(|error| format!("{error}"))?;
            continue;
        }
        let size = member.size();
        if size > SNAPSHOT_MAX_FILE_BYTES {
            return Err("Файл в снапшоте превышает лимит 256 MiB".to_owned());
        }
        declared_total += size;
        if declared_total > SNAPSHOT_MAX_UNPACKED_BYTES {
            return Err("Распакованный снапшот слишком большой".to_owned());
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{error}"))?;
        }
        // O_EXCL из Python: файл создаётся только новым — дубликаты путей
        // уже отвергнуты выше.
        let mut output = super::platform::create_private_new(&destination)
            .map_err(|error| format!("{error}"))?;
        let mut written: u64 = 0;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let count = member.read(&mut buffer).map_err(|error| format!("{error}"))?;
            if count == 0 {
                break;
            }
            written += count as u64;
            // Заявленный size может врать — считаем фактические байты.
            if written > SNAPSHOT_MAX_FILE_BYTES || written_total + written > SNAPSHOT_MAX_UNPACKED_BYTES {
                return Err("Распакованный снапшот слишком большой".to_owned());
            }
            output.write_all(&buffer[..count]).map_err(|error| format!("{error}"))?;
        }
        written_total += written;
    }
    Ok(())
}

/// Очистить цель и перенести туда содержимое временного каталога.
fn replace_directory_contents(root: &Path, target: &Path, temporary: &Path) -> Result<(), String> {
    debug_assert!(target.starts_with(root) && temporary.starts_with(root));
    fs::create_dir_all(target).map_err(|error| format!("{error}"))?;
    super::platform::restrict_dir(target);
    if !target.is_dir() || !temporary.is_dir() {
        return Err("Каталог восстановления снапшота недоступен".to_owned());
    }
    let entries = fs::read_dir(target).map_err(|error| format!("{error}"))?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        remove_entry(&entry.path())?;
    }
    let entries = fs::read_dir(temporary).map_err(|error| format!("{error}"))?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if is_symlink(&path) {
            return Err("Снапшот содержит символьную ссылку".to_owned());
        }
        let destination = target.join(entry.file_name());
        fs::rename(&path, &destination).map_err(|error| format!("{error}"))?;
    }
    Ok(())
}

fn archive_parts(member_name: &str) -> Result<Vec<String>, String> {
    let value = member_name.replace('\\', "/");
    // Пустые и «.»-фрагменты схлопываются, как у pathlib.PurePath.parts.
    let parts: Vec<String> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    if value.starts_with('/') || parts.is_empty() || parts.iter().any(|part| part == "..") {
        return Err("Снапшот содержит небезопасный путь".to_owned());
    }
    Ok(parts)
}

fn remove_entry(path: &Path) -> Result<(), String> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_symlink() || meta.is_file() {
        fs::remove_file(path).map_err(|error| format!("{error}"))?;
        return Ok(());
    }
    if meta.is_dir() {
        let entries = fs::read_dir(path).map_err(|error| format!("{error}"))?;
        for child in entries.filter_map(|entry| entry.ok()) {
            remove_entry(&child.path())?;
        }
        fs::remove_dir(path).map_err(|error| format!("{error}"))?;
        return Ok(());
    }
    Err("Снапшот содержит неподдерживаемый тип файла".to_owned())
}

fn remove_tree(path: &Path) -> Result<(), String> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_symlink() || meta.is_file() {
        fs::remove_file(path).map_err(|error| format!("{error}"))?;
        return Ok(());
    }
    if meta.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for child in entries.filter_map(|entry| entry.ok()) {
                remove_tree(&child.path())?;
            }
        }
        fs::remove_dir(path).map_err(|error| format!("{error}"))?;
    }
    Ok(())
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

/// Псевдослучайные 8 hex-символов (у uuid в сборке только v5).
fn unique_suffix() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0);
    let mut mixed = nanos ^ (u64::from(std::process::id()) << 32);
    mixed = mixed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (mixed ^ (mixed >> 31)) as u32
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn instance_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-snapshots-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_list_restore_roundtrip() {
        let root = instance_root("roundtrip");
        fs::create_dir_all(root.join("mods").join("nested")).unwrap();
        fs::write(root.join("mods").join("a.jar"), b"alpha").unwrap();
        fs::write(root.join("mods").join("nested").join("b.jar"), b"beta").unwrap();

        let store = SnapshotStore::new(&root).unwrap();
        let manifest = store.create("mods", SNAPSHOT_RETENTION_HOURS, 10).unwrap();
        assert_eq!(manifest["category"], json!("mods"));
        assert_eq!(manifest["file_count"], json!(2));
        assert_eq!(manifest["source_bytes"], json!(9));

        let listed = store.list("mods").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["target"], json!("mods"));
        assert!(store.latest("mods").unwrap().is_some());

        // Ломаем мир и восстанавливаем: содержимое обязано вернуться.
        fs::remove_file(root.join("mods").join("a.jar")).unwrap();
        fs::write(root.join("mods").join("junk.jar"), b"junk").unwrap();
        let snapshot_id = manifest["id"].as_str().unwrap().to_owned();
        let restored = store.restore("mods", &snapshot_id).unwrap();
        assert!(restored.get("restored_at").and_then(Value::as_f64).unwrap_or(0.0) > 0.0);
        assert_eq!(fs::read(root.join("mods").join("a.jar")).unwrap(), b"alpha");
        assert_eq!(fs::read(root.join("mods").join("nested").join("b.jar")).unwrap(), b"beta");
        assert!(!root.join("mods").join("junk.jar").exists());
        // временный каталог восстановления удалён
        let leftovers = fs::read_dir(&root)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .map(|entry| entry.file_name().to_string_lossy().contains("-restore-"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(leftovers, 0);

        assert!(store.delete("mods", &snapshot_id).unwrap());
        assert!(!store.delete("mods", &snapshot_id).unwrap());
        assert!(store.list("mods").unwrap().is_empty());
    }

    #[test]
    fn cleanup_keeps_only_max_count() {
        let root = instance_root("cleanup");
        fs::create_dir_all(root.join("saves").join("world")).unwrap();
        fs::write(root.join("saves").join("world").join("level.dat"), b"data").unwrap();
        let store = SnapshotStore::new(&root).unwrap();
        store.create("saves", 720, 1).unwrap();
        // mtime манифестов должен разниться, чтобы порядок был честным
        std::thread::sleep(std::time::Duration::from_millis(20));
        store.create("saves", 720, 1).unwrap();
        let listed = store.list("saves").unwrap();
        assert_eq!(listed.len(), 1);
        // второй снапшот вытеснил первый: остался один zip и один json
        let directory = root.join(SNAPSHOT_DIR);
        let archives = fs::read_dir(&directory).unwrap().filter(|entry| entry.as_ref().map(|e| e.file_name().to_string_lossy().ends_with(".zip")).unwrap_or(false)).count();
        assert_eq!(archives, 1);
    }

    #[test]
    fn manifest_cannot_reference_external_archive() {
        let root = instance_root("external");
        fs::create_dir_all(root.join("plugins")).unwrap();
        let snapshot_dir = root.join(SNAPSHOT_DIR);
        fs::create_dir_all(&snapshot_dir).unwrap();
        let snapshot_id = "1700000000-deadbeef";
        fs::write(
            snapshot_dir.join(format!("plugins-{snapshot_id}.json")),
            json!({ "id": snapshot_id, "category": "plugins", "archive": "../outside.zip" }).to_string(),
        )
        .unwrap();
        let store = SnapshotStore::new(&root).unwrap();
        assert!(store.list("plugins").unwrap().is_empty());
        let error = store.restore("plugins", snapshot_id).unwrap_err();
        assert_eq!(error, "Снапшот не найден");
    }

    #[test]
    #[cfg(unix)] // проверка завязана на dangling symlink
    fn listing_skips_dangling_symlinks_and_oversized_manifests() {
        let root = instance_root("listing");
        let snapshot_dir = root.join(SNAPSHOT_DIR);
        fs::create_dir_all(&snapshot_dir).unwrap();
        symlink(snapshot_dir.join("missing.json"), snapshot_dir.join("plugins-1700000000-deadbeef.json")).unwrap();
        fs::write(
            snapshot_dir.join("plugins-1700000001-cafebabe.json"),
            vec![b' '; (MAX_SNAPSHOT_MANIFEST_BYTES + 1) as usize],
        )
        .unwrap();
        let store = SnapshotStore::new(&root).unwrap();
        assert!(store.list("plugins").unwrap().is_empty());
    }

    #[test]
    fn restore_rejects_archive_traversal_and_duplicates() {
        let root = instance_root("traversal");
        fs::create_dir_all(root.join("plugins")).unwrap();
        let store = SnapshotStore::new(&root).unwrap();
        let snapshot_id = "1700000000-cafebabe";
        let snapshot_dir = root.join(SNAPSHOT_DIR);
        fs::create_dir_all(&snapshot_dir).unwrap();
        let archive_path = snapshot_dir.join(format!("plugins-{snapshot_id}.zip"));
        {
            let mut archive = ZipWriter::new(File::create(&archive_path).unwrap());
            archive.start_file("../escape.txt", zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(b"blocked").unwrap();
            archive.finish().unwrap();
        }
        fs::write(
            snapshot_dir.join(format!("plugins-{snapshot_id}.json")),
            json!({ "id": snapshot_id, "category": "plugins", "archive": archive_path.file_name().unwrap().to_string_lossy() }).to_string(),
        )
        .unwrap();
        let error = store.restore("plugins", snapshot_id).unwrap_err();
        assert_eq!(error, "Снапшот содержит небезопасный путь");
        assert!(!root.parent().unwrap().join("escape.txt").exists());

        // Дубликаты путей в архиве тоже отвергаются. zip не даёт записать
        // два одинаковых имени, поэтому дубликат — через «./»-префикс:
        // после нормализации это тот же путь.
        let duplicate_id = "1700000001-cafebabe";
        let duplicate_path = snapshot_dir.join(format!("plugins-{duplicate_id}.zip"));
        {
            let mut archive = ZipWriter::new(File::create(&duplicate_path).unwrap());
            archive.start_file("same.txt", zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(b"x").unwrap();
            archive.start_file("./same.txt", zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(b"y").unwrap();
            archive.finish().unwrap();
        }
        fs::write(
            snapshot_dir.join(format!("plugins-{duplicate_id}.json")),
            json!({ "id": duplicate_id, "category": "plugins", "archive": duplicate_path.file_name().unwrap().to_string_lossy() }).to_string(),
        )
        .unwrap();
        let error = store.restore("plugins", duplicate_id).unwrap_err();
        assert_eq!(error, "Снапшот содержит повторяющийся путь");
    }

    #[test]
    fn unknown_category_and_missing_instance_are_rejected() {
        let root = instance_root("unknown");
        let store = SnapshotStore::new(&root).unwrap();
        assert_eq!(store.list("nope").unwrap_err(), "Неизвестная категория снапшота");
        assert_eq!(
            SnapshotStore::new(&root.join("missing")).unwrap_err(),
            "Каталог экземпляра недоступен для снапшотов"
        );
    }
}
