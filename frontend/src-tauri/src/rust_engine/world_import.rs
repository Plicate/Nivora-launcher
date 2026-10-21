// импорт мира из локальной папки прямо на диске: источник выбирается во
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::config::ConfigStore;
use super::instances::InstanceStore;

const STAGING_DIRECTORY: &str = ".nivora-world-import";
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 300_000;
const MAX_SEARCH_DEPTH: usize = 6;

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
    // путь обязан быть обозреваемым: встроенный проводник пускает только в
    // дом, папку игры и данные лаунчера
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

// копия дерева с лимитами; возвращает корень, из которого ищем миры
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

// папки с level.dat, миры не вкладываются друг в друга
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
        assert!(!safe_world_name(&"x".repeat(200)).len() > 80);
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
}
