// операции над мирами: переименование, копия, удаление
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::events::emit_status_event;
use super::instance_workspace::InstanceWorkspace;

const MAX_LEVEL_DAT_BYTES: usize = 8 * 1024 * 1024;
const MAX_LEVEL_DAT_UNCOMPRESSED: usize = 64 * 1024 * 1024;
const MAX_WORLD_FILES: u64 = 400_000;
const MAX_LEVEL_NAME_CHARS: usize = 64;
const LEVEL_NAME_TAG: &[u8] = b"LevelName";

fn request_str<'a>(request: &'a Value, field: &str) -> &'a str {
    request
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn clamp_name(value: &str) -> String {
    value.trim().chars().take(MAX_LEVEL_NAME_CHARS).collect()
}

// индекс длины значения TAG_String "LevelName" в распакованном NBT
fn level_name_value_pos(data: &[u8]) -> Option<usize> {
    let mut index = 0usize;
    while index + 3 + LEVEL_NAME_TAG.len() + 2 <= data.len() {
        if data[index] == 8
            && data[index + 1] == 0
            && data[index + 2] == LEVEL_NAME_TAG.len() as u8
            && &data[index + 3..index + 3 + LEVEL_NAME_TAG.len()] == LEVEL_NAME_TAG
        {
            return Some(index + 3 + LEVEL_NAME_TAG.len());
        }
        index += 1;
    }
    None
}

fn value_bounds(data: &[u8], pos: usize) -> Option<(usize, usize)> {
    let len = u16::from_be_bytes([*data.get(pos)?, *data.get(pos + 1)?]) as usize;
    let start = pos + 2;
    let end = start.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some((start, end))
}

pub(crate) fn read_level_name(world: &Path) -> Option<String> {
    let raw = fs::read(world.join("level.dat")).ok()?;
    let data = if raw.starts_with(&[0x1f, 0x8b]) {
        gunzip_bounded(&raw).ok()?
    } else {
        raw
    };
    let pos = level_name_value_pos(&data)?;
    let (start, end) = value_bounds(&data, pos)?;
    Some(String::from_utf8_lossy(&data[start..end]).to_string())
}

fn gunzip_bounded(raw: &[u8]) -> Result<Vec<u8>, String> {
    let decoder = flate2::read::GzDecoder::new(raw);
    let mut out = Vec::new();
    decoder
        .take((MAX_LEVEL_DAT_UNCOMPRESSED + 1) as u64)
        .read_to_end(&mut out)
        .map_err(|error| format!("Не удалось распаковать level.dat: {error}"))?;
    if out.len() > MAX_LEVEL_DAT_UNCOMPRESSED {
        return Err("level.dat слишком большой".to_owned());
    }
    Ok(out)
}

fn gzip_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(data)
        .map_err(|error| format!("Не удалось упаковать level.dat: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("Не удалось упаковать level.dat: {error}"))
}

// точечная правка Data.LevelName без полного перебора NBT: сплайс строки
fn patch_level_name_bytes(data: &mut Vec<u8>, new_name: &str) -> bool {
    let new_bytes = new_name.as_bytes();
    if new_bytes.len() > u16::MAX as usize {
        return false;
    }
    let Some(pos) = level_name_value_pos(data) else {
        return false;
    };
    let Some((start, end)) = value_bounds(data, pos) else {
        return false;
    };
    data.splice(start..end, new_bytes.iter().copied());
    data[pos] = (new_bytes.len() >> 8) as u8;
    data[pos + 1] = (new_bytes.len() & 0xFF) as u8;
    true
}

// Ok(false) — метки LevelName в файле нет, имя каталога определяет мир
pub(crate) fn patch_level_name(world: &Path, new_name: &str) -> Result<bool, String> {
    let path = world.join("level.dat");
    let raw =
        fs::read(&path).map_err(|error| format!("Не удалось прочитать level.dat: {error}"))?;
    if raw.len() > MAX_LEVEL_DAT_BYTES {
        return Err("level.dat слишком большой".to_owned());
    }
    let gzipped = raw.starts_with(&[0x1f, 0x8b]);
    let mut data = if gzipped { gunzip_bounded(&raw)? } else { raw };
    if !patch_level_name_bytes(&mut data, new_name) {
        return Ok(false);
    }
    let payload = if gzipped { gzip_bytes(&data)? } else { data };
    let temp = world.join("level.dat.tmp");
    fs::write(&temp, &payload)
        .map_err(|error| format!("Не удалось записать level.dat: {error}"))?;
    fs::rename(&temp, &path).map_err(|error| format!("Не удалось обновить level.dat: {error}"))?;
    Ok(true)
}

fn skip_top_level(src: &Path, root: &Path, name: &str) -> bool {
    src == root && name == "session.lock"
}

fn copy_world_dir(from: &Path, to: &Path) -> Result<u64, String> {
    fs::create_dir_all(to).map_err(|error| format!("Не удалось создать копию мира: {error}"))?;
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    let mut files = 0u64;
    let mut bytes = 0u64;
    while let Some((src, dst)) = stack.pop() {
        let entries =
            fs::read_dir(&src).map_err(|error| format!("Не удалось прочитать мир: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("Не удалось прочитать мир: {error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip_top_level(&src, from, &name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let target = dst.join(entry.file_name());
            if file_type.is_dir() {
                fs::create_dir_all(&target)
                    .map_err(|error| format!("Не удалось создать копию мира: {error}"))?;
                stack.push((entry.path(), target));
            } else {
                files += 1;
                if files > MAX_WORLD_FILES {
                    return Err("Мир слишком большой для копирования".to_owned());
                }
                let size = fs::metadata(entry.path())
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                fs::copy(entry.path(), &target)
                    .map_err(|error| format!("Не удалось скопировать мир: {error}"))?;
                bytes += size;
            }
        }
    }
    Ok(bytes)
}

pub(crate) fn safe_world_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
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
    let name = if reserved {
        format!("{cleaned}_")
    } else {
        cleaned
    };
    name.chars().take(80).collect::<String>()
}

pub(crate) fn unique_world_dir(saves: &Path, name: &str) -> PathBuf {
    let mut candidate = saves.join(name);
    let mut index = 2;
    while candidate.symlink_metadata().is_ok() {
        candidate = saves.join(format!("{name} ({index})"));
        index += 1;
    }
    candidate
}

fn rename_world_dir(world: &Path, display: &str) -> Result<PathBuf, String> {
    let parent = world
        .parent()
        .ok_or_else(|| "Мир не найден".to_owned())?
        .to_path_buf();
    let current = world
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .ok_or_else(|| "Мир не найден".to_owned())?;
    let dir_name = safe_world_name(display);
    if dir_name.is_empty() {
        return Err("Имя мира пустое".to_owned());
    }
    let original = fs::read(world.join("level.dat")).unwrap_or_default();
    let patched = patch_level_name(world, display).unwrap_or(false);
    if dir_name == current {
        return Ok(world.to_path_buf());
    }
    let target = unique_world_dir(&parent, &dir_name);
    match fs::rename(world, &target) {
        Ok(()) => Ok(target),
        Err(error) => {
            if patched && !original.is_empty() {
                let _ = fs::write(world.join("level.dat"), &original);
            }
            Err(format!("Не удалось переименовать мир: {error}"))
        }
    }
}

fn copy_world_to(world: &Path, display: &str) -> Result<PathBuf, String> {
    let parent = world
        .parent()
        .ok_or_else(|| "Мир не найден".to_owned())?
        .to_path_buf();
    let dir_name = safe_world_name(display);
    if dir_name.is_empty() {
        return Err("Название копии пустое".to_owned());
    }
    let target = unique_world_dir(&parent, &dir_name);
    copy_world_dir(world, &target)?;
    let _ = patch_level_name(&target, display);
    Ok(target)
}

fn workspace_world(request: &Value) -> Result<(InstanceWorkspace, String, PathBuf, bool), String> {
    let name = request_str(request, "name");
    let world_id = request_str(request, "world");
    if world_id.is_empty() {
        return Err("Не указан мир".to_owned());
    }
    let workspace = InstanceWorkspace::new()?;
    let info = workspace.java_instance(name)?;
    let running = info.running;
    let world = workspace.world_directory(name, world_id)?;
    Ok((workspace, name.to_owned(), world, running))
}

fn require_stopped(running: bool, action: &str) -> Result<(), String> {
    if running {
        return Err(format!("Мир нельзя {action}, пока игра запущена"));
    }
    Ok(())
}

pub(crate) fn world_rename(request: &Value) -> Result<Value, String> {
    let display = clamp_name(request_str(request, "new_name"));
    if display.is_empty() {
        return Err("Введите новое имя мира".to_owned());
    }
    if display == "." || display == ".." {
        return Err("Небезопасное имя мира".to_owned());
    }
    let (workspace, name, world, running) = workspace_world(request)?;
    require_stopped(running, "переименовать")?;
    rename_world_dir(&world, &display)?;
    emit_status_event("workspace-changed", "Мир переименован.");
    let details = workspace.snapshot(&name)?;
    Ok(json!({ "details": details, "renamed": display }))
}

pub(crate) fn world_copy(request: &Value) -> Result<Value, String> {
    let requested = clamp_name(request_str(request, "new_name"));
    let (workspace, name, world, running) = workspace_world(request)?;
    require_stopped(running, "копировать")?;
    let original = read_level_name(&world).unwrap_or_else(|| {
        world
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut display = requested;
    if display.is_empty() {
        display = format!("{original} (копия)");
    }
    if display.is_empty() {
        display = "Копия мира".to_owned();
    }
    let target = copy_world_to(&world, &display)?;
    let copied = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    emit_status_event("workspace-changed", "Копия мира создана.");
    let details = workspace.snapshot(&name)?;
    Ok(json!({ "details": details, "copied": copied, "name": display }))
}

pub(crate) fn world_delete(request: &Value) -> Result<Value, String> {
    let (workspace, name, world, running) = workspace_world(request)?;
    require_stopped(running, "удалить")?;
    let display = read_level_name(&world).unwrap_or_else(|| {
        world
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    fs::remove_dir_all(&world).map_err(|error| format!("Не удалось удалить мир: {error}"))?;
    emit_status_event("workspace-changed", "Мир удалён.");
    let details = workspace.snapshot(&name)?;
    Ok(json!({ "details": details, "deleted": display }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_level_dat(level_name: &str) -> Vec<u8> {
        let mut out = vec![10u8];
        out.extend(0u16.to_be_bytes());
        out.push(10);
        out.extend((b"Data".len() as u16).to_be_bytes());
        out.extend_from_slice(b"Data");
        out.push(8);
        out.extend((b"LevelName".len() as u16).to_be_bytes());
        out.extend_from_slice(b"LevelName");
        out.extend((level_name.len() as u16).to_be_bytes());
        out.extend_from_slice(level_name.as_bytes());
        out.push(3);
        out.extend((b"SpawnX".len() as u16).to_be_bytes());
        out.extend_from_slice(b"SpawnX");
        out.extend(100i32.to_be_bytes());
        out.push(0);
        out.push(0);
        gzip_bytes(&out).unwrap()
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nivora-world-actions-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn level_name_patch_roundtrip() {
        let raw = build_level_dat("Старый мир");
        let mut data = gunzip_bounded(&raw).unwrap();
        assert!(patch_level_name_bytes(&mut data, "Новое имя мира"));
        let repacked = gzip_bytes(&data).unwrap();
        assert!(repacked.starts_with(&[0x1f, 0x8b]));
        let round = gunzip_bounded(&repacked).unwrap();
        let pos = level_name_value_pos(&round).unwrap();
        let (start, end) = value_bounds(&round, pos).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&round[start..end]),
            "Новое имя мира"
        );
        // соседний тег не задет
        assert!(round.windows(6).any(|window| window == b"SpawnX"));
    }

    #[test]
    fn patch_missing_tag_is_not_fatal() {
        let mut data = vec![10u8, 0, 0, 0];
        assert!(!patch_level_name_bytes(&mut data, "x"));
    }

    #[test]
    fn rename_moves_dir_and_updates_level_name() {
        let root = temp_root("rename");
        let saves = root.join("saves");
        let world = saves.join("Old World");
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), build_level_dat("Old World")).unwrap();
        fs::create_dir_all(world.join("region")).unwrap();
        fs::write(world.join("region/r.0.0.mca"), vec![1, 2, 3]).unwrap();
        let target = rename_world_dir(&world, "Новый мир").unwrap();
        assert_eq!(target.file_name().unwrap(), "Новый мир");
        assert!(!world.exists());
        assert_eq!(read_level_name(&target).as_deref(), Some("Новый мир"));
        assert!(target.join("region/r.0.0.mca").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_duplicates_content_and_rewrites_level_name() {
        let root = temp_root("copy");
        let saves = root.join("saves");
        let world = saves.join("Base");
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), build_level_dat("Base")).unwrap();
        fs::write(world.join("session.lock"), b"lock").unwrap();
        fs::create_dir_all(world.join("region")).unwrap();
        fs::write(world.join("region/r.0.0.mca"), vec![7; 64]).unwrap();
        let target = copy_world_to(&world, "Base (копия)").unwrap();
        assert_eq!(target.file_name().unwrap(), "Base (копия)");
        assert!(world.is_dir());
        assert_eq!(read_level_name(&target).as_deref(), Some("Base (копия)"));
        assert!(!target.join("session.lock").exists());
        assert_eq!(
            fs::read(target.join("region/r.0.0.mca")).unwrap(),
            vec![7; 64]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_uses_unique_dir_when_target_exists() {
        let root = temp_root("copy-unique");
        let saves = root.join("saves");
        let world = saves.join("Base");
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), build_level_dat("Base")).unwrap();
        fs::create_dir_all(saves.join("Base (копия)")).unwrap();
        let target = copy_world_to(&world, "Base (копия)").unwrap();
        assert_eq!(target.file_name().unwrap(), "Base (копия) (2)");
        let _ = fs::remove_dir_all(&root);
    }
}
