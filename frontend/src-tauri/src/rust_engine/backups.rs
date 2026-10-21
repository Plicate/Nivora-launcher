// резервные копии игровых миров: ZIP-архивы только игрового содержимого
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::state::{instance_kind_of, instance_running, launcher_roots, read_bounded_json, resolve_instance_path, safe_instance_name, settings_game_dir};

const BACKUP_CONFIG_FILE: &str = ".nivora-launcher-backups.json";
const MAX_BACKUP_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_BACKUP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_BACKUP_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BACKUP_FILES: usize = 100_000;
const SERVER_WORLD_NAMES: [&str; 3] = ["world", "world_nether", "world_the_end"];
const SCHEDULE_KEYS: [&str; 7] = ["enabled", "interval_hours", "keep", "destination", "last_run", "last_status", "last_error"];

fn err(message: &str) -> String {
    message.to_owned()
}

fn epoch_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0)
}

/// локальное время в формате %Y%m%d-%H%M%S (как time.strftime).
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let seconds = now.as_secs() as i64;
    match local_datetime(seconds) {
        Some((year, month, day, hour, minute, second)) => format!(
            "{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"
        ),
        None => format!("{seconds}"),
    }
}

#[cfg(unix)]
fn local_datetime(seconds: i64) -> Option<(i64, u32, u32, u32, u32, u32)> {
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_year as i64 + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32))
    }
}

#[cfg(windows)]
fn local_datetime(seconds: i64) -> Option<(i64, u32, u32, u32, u32, u32)> {
    use windows_sys::Win32::System::SystemServices::TIME_ZONE_ID_DAYLIGHT;
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

    let bias_minutes: i64 = unsafe {
        let mut info: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let status = GetTimeZoneInformation(&mut info);
        // bias = UTC − local; в дневном свете действует DaylightBias.
        let extra = if status == TIME_ZONE_ID_DAYLIGHT { info.DaylightBias } else { info.StandardBias };
        (info.Bias + extra) as i64
    };
    let local = seconds - bias_minutes * 60;
    let days = local.div_euclid(86_400);
    let rest = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some((year, month, day, (rest / 3600) as u32, ((rest % 3600) / 60) as u32, (rest % 60) as u32))
}

#[cfg(windows)]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

struct Instance {
    name: String,
    path: PathBuf,
    kind: String,
}

fn resolve_instance(game_dir: &Path, raw: &str) -> Result<Instance, String> {
    let name = safe_instance_name(raw);
    let Some(path) = resolve_instance_path(game_dir, raw) else {
        return Err(err(&format!("Экземпляр не найден: {raw}")));
    };
    Ok(Instance { name, kind: instance_kind_of(&path), path })
}

fn settings(config_home: &Path) -> Value {
    read_bounded_json(&config_home.join("settings.json"), 1024 * 1024).unwrap_or(Value::Null)
}

/// приведение bool как в настройках: true/1/yes/on и false/0/no/off.
fn coerce_bool(value: Option<&Value>, fallback: bool) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => match text.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => fallback,
        },
        Some(Value::Number(number)) => number.as_f64().map(|value| value != 0.0).unwrap_or(fallback),
        _ => fallback,
    }
}

fn py_int(value: Option<&Value>) -> Option<i64> {
    // int(x) в Python принимает числа и строки; иначе исключение.
    match value {
        Some(Value::Number(number)) => number.as_f64().map(|value| value as i64),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn normalize_schedule(raw: &Value) -> Map<String, Value> {
    let object = raw.as_object().cloned().unwrap_or_default();
    let destination = object.get("destination").and_then(Value::as_str).unwrap_or("local").trim().to_lowercase();
    let interval_hours = match py_int(object.get("interval_hours")) {
        Some(value) => value.clamp(1, 24 * 30),
        None => 24,
    };
    let keep = match py_int(object.get("keep")) {
        Some(value) => value.clamp(1, 50),
        None => 5,
    };
    let mut result = Map::new();
    result.insert("enabled".into(), json!(coerce_bool(object.get("enabled"), false)));
    result.insert("interval_hours".into(), json!(interval_hours));
    result.insert("keep".into(), json!(keep));
    result.insert("destination".into(), json!(if destination == "cloud" { "cloud" } else { "local" }));
    result
}

fn normalize_instance_schedule(raw: &Value) -> Map<String, Value> {
    let object = raw.as_object().cloned().unwrap_or_default();
    let mut result = normalize_schedule(&Value::Object(object.clone()));
    let last_run = object
        .get("last_run")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    result.insert("last_run".into(), json!(last_run));
    result.insert(
        "last_status".into(),
        json!(object.get("last_status").and_then(Value::as_str).unwrap_or("").chars().take(200).collect::<String>()),
    );
    result.insert(
        "last_error".into(),
        json!(object.get("last_error").and_then(Value::as_str).unwrap_or("").chars().take(400).collect::<String>()),
    );
    result
}

fn schedule_object(value: Map<String, Value>) -> Value {
    Value::Object(value)
}

fn read_stored_schedule(instance_path: &Path) -> Map<String, Value> {
    read_bounded_json(&instance_path.join(BACKUP_CONFIG_FILE), MAX_BACKUP_CONFIG_BYTES)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn write_stored_schedule(instance_path: &Path, values: &Map<String, Value>) -> Result<(), String> {
    let mut payload = normalize_instance_schedule(&schedule_object(values.clone()));
    payload.insert("override".into(), json!(coerce_bool(values.get("override"), false)));
    atomic_private_json(&instance_path.join(BACKUP_CONFIG_FILE), &Value::Object(payload))
}

fn atomic_private_json(path: &Path, value: &Value) -> Result<(), String> {
    let temporary = path.with_extension(format!("json.part-{}", std::process::id()));
    {
        let mut file = super::platform::create_private(&temporary)
            .map_err(|error| format!("Не удалось записать расписание копий: {error}"))?;
        let text = serde_json::to_string(value).map_err(|_| err("Не удалось записать расписание копий"))?;
        file.write_all(text.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Не удалось записать расписание копий: {error}"))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Не удалось записать расписание копий: {error}")
    })
}

fn roots() -> (PathBuf, PathBuf) {
    let roots = launcher_roots();
    let game = settings_game_dir(&roots.config_home, &roots.data_home);
    (roots.config_home, game)
}

fn schedule_for(config_home: &Path, game_dir: &Path, raw_name: &str) -> Result<Value, String> {
    let instance = resolve_instance(game_dir, raw_name)?;
    let global = settings(config_home)
        .get("backup_schedule")
        .cloned()
        .unwrap_or(Value::Null);
    let mut merged = normalize_schedule(&global);
    let stored = read_stored_schedule(&instance.path);
    let override_on = coerce_bool(stored.get("override"), false);
    if override_on {
        let mut instance_values = merged.clone();
        for key in stored.keys() {
            if SCHEDULE_KEYS.contains(&key.as_str()) {
                instance_values.insert(key.clone(), stored.get(key).cloned().unwrap_or(Value::Null));
            }
        }
        merged = normalize_instance_schedule(&schedule_object(instance_values));
    }
    merged.insert("override".into(), json!(override_on));
    Ok(schedule_object(merged))
}

fn local_root(game_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let root = game_dir.join("backups").join(name);
    fs::create_dir_all(&root).map_err(|error| format!("Каталог резервных копий недоступен: {error}"))?;
    super::platform::restrict_dir(&root);
    Ok(root)
}

/// список локальных ZIP-копий: свежие первыми.
fn local_backups(game_dir: &Path, name: &str) -> Result<Vec<Value>, String> {
    let root = local_root(game_dir, name)?;
    let mut names: Vec<String> = fs::read_dir(&root)
        .map_err(|error| format!("Каталог резервных копий недоступен: {error}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".zip"))
        .collect();
    names.sort();
    let mut result: Vec<Value> = Vec::new();
    for file_name in names {
        let path = root.join(&file_name);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        result.push(json!({
            "id": file_name.trim_end_matches(".zip"),
            "created_at": meta.modified().ok().and_then(|value| value.duration_since(UNIX_EPOCH).ok()).map(|value| value.as_secs_f64()).unwrap_or(0.0),
            "size_bytes": meta.len(),
            "destination": "local",
        }));
    }
    result.sort_by(|a, b| {
        let left = a["created_at"].as_f64().unwrap_or(0.0);
        let right = b["created_at"].as_f64().unwrap_or(0.0);
        right.partial_cmp(&left).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(result)
}

fn world_directories(instance: &Instance) -> Vec<PathBuf> {
    if instance.kind == "server" {
        return SERVER_WORLD_NAMES
            .iter()
            .map(|name| instance.path.join(name))
            .filter(|path| fs::metadata(path).map(|meta| meta.is_dir()).unwrap_or(false) && !fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false))
            .collect();
    }
    let saves = instance.path.join("saves");
    if fs::symlink_metadata(&saves).map(|meta| meta.is_symlink()).unwrap_or(true) || !saves.is_dir() {
        return Vec::new();
    }
    let mut worlds: Vec<PathBuf> = fs::read_dir(&saves)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false))
        .collect();
    worlds.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
    worlds
}

fn worlds_root(instance: &Instance) -> PathBuf {
    if instance.kind == "server" {
        instance.path.clone()
    } else {
        instance.path.join("saves")
    }
}

fn write_archive(archive_path: &Path, instance_name: &str, kind: &str, worlds: &[PathBuf]) -> Result<usize, String> {
    let temporary = archive_path.with_file_name(format!("{}.part", archive_path.file_name().and_then(|name| name.to_str()).unwrap_or("backup.zip")));
    let mut count = 0usize;
    let mut total_bytes: u64 = 0;
    let result = (|| -> Result<(), String> {
        let file = super::platform::create_private(&temporary)
            .map_err(|error| format!("Не удалось создать резервную копию: {error}"))?;
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6));
        let manifest = json!({
            "format": "nivora-world-backup/1",
            "instance": instance_name,
            "created_at": epoch_seconds(),
            "kind": kind,
            "worlds": worlds.iter().map(|world| world.file_name().and_then(|name| name.to_str()).unwrap_or_default()).collect::<Vec<_>>(),
        });
        writer.start_file("manifest.json", options).map_err(|error| format!("Не удалось создать резервную копию: {error}"))?;
        writer.write_all(serde_json::to_string(&manifest).unwrap_or_default().as_bytes()).map_err(|error| format!("Не удалось создать резервную копию: {error}"))?;

        fn walk(dir: &Path, prefix: &str, writer: &mut zip::ZipWriter<fs::File>, options: zip::write::SimpleFileOptions, state: &mut (usize, u64)) -> Result<(), String> {
            let mut entries: Vec<_> = fs::read_dir(dir).map_err(|error| format!("Не удалось прочитать мир: {error}"))?.filter_map(|entry| entry.ok()).collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in &entries {
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                if meta.is_symlink() || !meta.is_file() {
                    continue;
                }
                if state.0 >= MAX_BACKUP_FILES || state.1 > MAX_BACKUP_BYTES {
                    return Err(err("Мир слишком велик для резервной копии (лимит 2 ГиБ)"));
                }
                let size = meta.len();
                if size > MAX_BACKUP_FILE_BYTES {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let relative = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
                writer.start_file(relative.clone(), options).map_err(|error| format!("Не удалось создать резервную копию: {error}"))?;
                let mut source = fs::File::open(entry.path()).map_err(|error| format!("Не удалось прочитать файл мира: {error}"))?;
                std::io::copy(&mut source, writer).map_err(|error| format!("Не удалось создать резервную копию: {error}"))?;
                state.0 += 1;
                state.1 += size;
            }
            // подкаталоги — после файлов текущего уровня (rglob обходит вглубь).
            for entry in entries {
                if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
                    && !fs::symlink_metadata(entry.path()).map(|meta| meta.is_symlink()).unwrap_or(false)
                {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let prefix = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                    walk(&entry.path(), &prefix, writer, options, state)?;
                }
            }
            Ok(())
        }

        for world in worlds {
            let world_name = world.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned();
            let mut state = (count, total_bytes);
            walk(world, &format!("worlds/{world_name}"), &mut writer, options, &mut state)?;
            count = state.0;
            total_bytes = state.1;
        }
        writer.finish().map_err(|error| format!("Не удалось создать резервную копию: {error}"))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            fs::rename(&temporary, archive_path).map_err(|error| {
                let _ = fs::remove_file(&temporary);
                format!("Не удалось создать резервную копию: {error}")
            })?;
            Ok(count)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn prune_local(game_dir: &Path, instance: &Instance, keep: i64) -> Result<(), String> {
    let root = local_root(game_dir, &instance.name)?;
    let mut archives: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&root).into_iter().flatten().filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if !name.ends_with(".zip") {
            continue;
        }
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.is_symlink() {
                continue;
            }
            if let Ok(modified) = meta.modified() {
                archives.push((modified, path));
            }
        }
    }
    archives.sort_by_key(|(modified, _)| *modified);
    let keep = keep.clamp(1, 50) as usize;
    if archives.len() > keep {
        for (_, path) in &archives[..archives.len() - keep] {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

pub fn world_backup_list(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let (config_home, game_dir) = roots();
    Ok(json!({
        "backups": local_backups(&game_dir, &resolve_instance(&game_dir, name)?.name)?,
        "schedule": schedule_for(&config_home, &game_dir, name)?,
    }))
}

pub fn world_backup_create(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let reason: String = request
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("manual")
        .chars()
        .take(24)
        .collect();
    let reason = if reason.is_empty() { "manual".to_owned() } else { reason };
    let (config_home, game_dir) = roots();
    let instance = resolve_instance(&game_dir, name)?;
    if instance_running(&instance.path) {
        return Err(err("Остановите экземпляр перед созданием резервной копии мира"));
    }
    let worlds = world_directories(&instance);
    if worlds.is_empty() {
        return Err(err("В этом экземпляре нет миров для резервного копирования"));
    }
    let backup_id = format!("{}-{}", timestamp(), reason);
    let destination_root = local_root(&game_dir, &instance.name)?;
    let archive_path = destination_root.join(format!("{backup_id}.zip"));
    let files = write_archive(&archive_path, &instance.name, &instance.kind, &worlds)?;
    let size_bytes = fs::symlink_metadata(&archive_path).map(|meta| meta.len()).unwrap_or(0);
    let schedule_value = schedule_for(&config_home, &game_dir, name)?;
    let keep = schedule_value.get("keep").and_then(Value::as_i64).unwrap_or(5);
    prune_local(&game_dir, &instance, keep)?;
    let mut values = Map::new();
    values.insert("last_run".into(), json!(epoch_seconds()));
    values.insert("last_status".into(), json!(format!("сохранено {files} файлов")));
    apply_schedule_values(&config_home, &game_dir, name, &Value::Object(values), None)?;
    Ok(json!({
        "backup": {
            "backup_id": backup_id,
            "size_bytes": size_bytes,
            "files": files,
            "backups": local_backups(&game_dir, &instance.name)?,
        },
        "schedule": schedule_for(&config_home, &game_dir, name)?,
    }))
}

pub fn world_backup_restore(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let backup_id = request.get("backup_id").and_then(Value::as_str).unwrap_or_default();
    let (_config_home, game_dir) = roots();
    let instance = resolve_instance(&game_dir, name)?;
    if instance_running(&instance.path) {
        return Err(err("Остановите экземпляр перед восстановлением мира"));
    }
    let archive_path = local_root(&game_dir, &instance.name)?.join(format!("{backup_id}.zip"));
    let is_present = fs::symlink_metadata(&archive_path).map(|meta| meta.is_file()).unwrap_or(false);
    if !is_present {
        return Err(err("Резервная копия не найдена"));
    }
    let worlds_root = worlds_root(&instance);
    fs::create_dir_all(&worlds_root).map_err(|error| format!("Не удалось восстановить мир: {error}"))?;
    super::platform::restrict_dir(&worlds_root);
    let suffix = format!(".pre-restore-{}", timestamp());
    let restored = restore_entries(&archive_path, &worlds_root, &suffix)?;
    Ok(json!({ "restored": restored, "backups": local_backups(&game_dir, &instance.name)? }))
}

fn restore_entries(archive_path: &Path, worlds_root: &Path, suffix: &str) -> Result<Vec<String>, String> {
    let file = fs::File::open(archive_path).map_err(|_| err("Резервная копия не найдена"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| err("Резервная копия повреждена"))?;
    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let member = archive.by_index(index).map_err(|_| err("Резервная копия повреждена"))?;
        if member.is_dir() {
            continue;
        }
        names.push(member.name().to_owned());
    }
    if names.len() > MAX_BACKUP_FILES {
        return Err(err("Резервная копия слишком велика для восстановления"));
    }
    for entry in &names {
        let parts: Vec<&str> = entry.split('/').collect();
        if entry.starts_with("__") || parts.contains(&"..") || entry.starts_with('/') {
            return Err(err("Резервная копия содержит небезопасные пути"));
        }
    }
    let mut restored: Vec<String> = Vec::new();
    for entry in &names {
        if entry == "manifest.json" {
            continue;
        }
        let parts: Vec<&str> = entry.split('/').collect();
        if parts.len() < 3 || parts[0] != "worlds" {
            continue;
        }
        let world_name = parts[1];
        if world_name == "." || world_name == ".." {
            return Err(err("Резервная копия содержит небезопасное имя мира"));
        }
        let destination = worlds_root.join(world_name);
        let exists = fs::symlink_metadata(&destination).is_ok();
        if exists && !fs::symlink_metadata(&destination).map(|meta| meta.is_symlink()).unwrap_or(false) && !restored.iter().any(|name| name == world_name) {
            let aside = worlds_root.join(format!("{world_name}{suffix}"));
            fs::rename(&destination, &aside).map_err(|error| format!("Не удалось восстановить мир: {error}"))?;
            restored.push(world_name.to_owned());
        }
        fs::create_dir_all(&destination).map_err(|error| format!("Не удалось восстановить мир: {error}"))?;
        super::platform::restrict_dir(&destination);
        let tail = &parts[2..];
        let target = destination.join(tail.join("/"));
        if fs::symlink_metadata(&target).map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err(err("Резервная копия содержит небезопасные пути"));
        }
        if !tail.iter().any(|part| *part == ".." || part.is_empty()) {
            fs::create_dir_all(target.parent().unwrap_or(&destination)).map_err(|error| format!("Не удалось восстановить мир: {error}"))?;
            let mut source = archive.by_name(entry).map_err(|_| err("Резервная копия повреждена"))?;
            let mut output = super::platform::create_private(&target)
                .map_err(|error| format!("Не удалось восстановить мир: {error}"))?;
            let mut limited = (&mut source).take(MAX_BACKUP_FILE_BYTES + 1);
            std::io::copy(&mut limited, &mut output).map_err(|error| format!("Не удалось восстановить мир: {error}"))?;
        }
    }
    Ok(restored)
}

pub fn world_backup_delete(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let backup_id = request.get("backup_id").and_then(Value::as_str).unwrap_or_default();
    let (_config_home, game_dir) = roots();
    let instance = resolve_instance(&game_dir, name)?;
    let path = local_root(&game_dir, &instance.name)?.join(format!("{backup_id}.zip"));
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Err(err("Резервная копия не найдена"));
    };
    if meta.is_symlink() || !meta.is_file() {
        return Err(err("Резервная копия не найдена"));
    }
    fs::remove_file(&path).map_err(|error| format!("Не удалось удалить копию: {error}"))?;
    Ok(json!({ "backups": local_backups(&game_dir, &instance.name)? }))
}

pub fn world_backup_schedule_get(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let (config_home, game_dir) = roots();
    Ok(json!({ "schedule": schedule_for(&config_home, &game_dir, name)? }))
}

pub fn world_backup_schedule_set(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let values = request.get("values").cloned().unwrap_or(Value::Null);
    let override_flag = request.get("override").and_then(Value::as_bool);
    let (config_home, game_dir) = roots();
    apply_schedule_values(&config_home, &game_dir, name, &values, override_flag)?;
    Ok(json!({ "schedule": schedule_for(&config_home, &game_dir, name)? }))
}

fn apply_schedule_values(config_home: &Path, game_dir: &Path, raw_name: &str, values: &Value, override_flag: Option<bool>) -> Result<Value, String> {
    let instance = resolve_instance(game_dir, raw_name)?;
    let mut stored = read_stored_schedule(&instance.path);
    if let Some(flag) = override_flag {
        stored.insert("override".into(), json!(flag));
    }
    let value_object = values.as_object().cloned().unwrap_or_default();
    if value_object.is_empty() {
        return schedule_for(config_home, game_dir, raw_name);
    }
    let mut merged = schedule_for(config_home, game_dir, raw_name)?
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (key, value) in stored.iter() {
        merged.insert(key.clone(), value.clone());
    }
    for (key, value) in value_object.iter() {
        if SCHEDULE_KEYS.contains(&key.as_str()) {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged.insert(
        "override".into(),
        json!(coerce_bool(stored.get("override"), false)),
    );
    write_stored_schedule(&instance.path, &merged)?;
    schedule_for(config_home, game_dir, raw_name)
}

pub(crate) fn cloud_configured(config_home: &Path) -> bool {
    let settings = settings(config_home);
    let url = settings.get("cloud_url").and_then(Value::as_str).unwrap_or_default().trim();
    let username = settings.get("cloud_username").and_then(Value::as_str).unwrap_or_default().trim();
    !url.is_empty() && !username.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-backups-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    fn make_client_instance(game: &Path, name: &str, worlds: &[(&str, &str)]) -> PathBuf {
        let instance = game.join("instances").join(name);
        fs::create_dir_all(instance.join("versions")).expect("versions");
        fs::write(
            instance.join(".nivora-launcher-instance.json"),
            json!({ "kind": "client", "display_name": name }).to_string(),
        )
        .expect("метаданные");
        for (world, file) in worlds {
            fs::create_dir_all(instance.join("saves").join(world)).expect("мир");
            fs::write(instance.join("saves").join(world).join(file), format!("данные {world}")).expect("файл мира");
        }
        instance
    }

    #[test]
    fn timestamp_shape_matches_python_strftime() {
        let value = timestamp();
        assert_eq!(value.len(), 15, "{value}");
        assert!(value.chars().nth(8) == Some('-'), "{value}");
        assert!(value[..8].chars().all(|character| character.is_ascii_digit()), "{value}");
    }

    #[test]
    fn schedule_normalization_clamps_and_defaults() {
        let raw = json!({ "enabled": "yes", "interval_hours": 9999, "keep": 0, "destination": "SKY" });
        let normalized = normalize_schedule(&raw);
        assert_eq!(normalized["enabled"], json!(true));
        assert_eq!(normalized["interval_hours"], json!(720));
        assert_eq!(normalized["keep"], json!(1));
        assert_eq!(normalized["destination"], json!("local"));
        let defaults = normalize_schedule(&json!({}));
        assert_eq!(defaults["interval_hours"], json!(24));
        assert_eq!(defaults["keep"], json!(5));
    }

    #[test]
    fn schedule_merge_respects_override_flag() {
        let root = temp_root("schedule");
        let game = root.join("game");
        let instance = make_client_instance(&game, "alpha", &[("w", "a.dat")]);
        let stored = json!({ "enabled": true, "interval_hours": 12, "keep": 3, "destination": "local", "last_run": 0, "last_status": "", "last_error": "", "override": true });
        write_stored_schedule(&instance, stored.as_object().unwrap()).expect("запись");

        let schedule = schedule_for(&root, &game, "alpha").expect("расписание");
        assert_eq!(schedule["override"], json!(true));
        assert_eq!(schedule["interval_hours"], json!(12));

        // глобальные настройки без override не видны в итоге.
        let values = json!({ "interval_hours": 48, "weird_key": 1 });
        let updated = apply_schedule_values(&root, &game, "alpha", &values, Some(false)).expect("обновление");
        assert_eq!(updated["override"], json!(false));
        // override выключен: schedule() показывает глобальные 24 часа,
        // хотя в хранилище записано 48.
        assert_eq!(updated["interval_hours"], json!(24));
        let stored_after = read_stored_schedule(&instance);
        assert_eq!(stored_after["interval_hours"], json!(48));
        assert!(stored_after.get("weird_key").is_none(), "неизвестные ключи не пишутся");

        // включение override поднимает сохранённые 48 часов.
        let flipped = apply_schedule_values(&root, &game, "alpha", &json!({ "enabled": true }), Some(true)).expect("override");
        assert_eq!(flipped["override"], json!(true));
        assert_eq!(flipped["interval_hours"], json!(48));
    }

    #[test]
    fn create_and_restore_roundtrip_with_prune() {
        let root = temp_root("roundtrip");
        let game = root.join("game");
        let instance = make_client_instance(&game, "beta", &[("survival", "level.dat"), ("creative", "b.txt")]);
        let worlds = world_directories(&Instance { name: "beta".into(), path: instance.clone(), kind: "client".into() });
        assert_eq!(worlds.len(), 2, "оба мира найдены");

        let backups_root = local_root(&game, "beta").expect("каталог копий");
        let archive = backups_root.join("manual-copy.zip");
        let files = write_archive(&archive, "beta", "client", &worlds).expect("архив");
        assert_eq!(files, 2);

        fs::write(instance.join("saves").join("survival").join("level.dat"), "испорчено").unwrap();
        let restored = restore_entries(&archive, &instance.join("saves"), ".pre-restore-test").expect("восстановление");
        assert_eq!(restored, vec!["creative".to_owned(), "survival".to_owned()]);
        let content = fs::read_to_string(instance.join("saves").join("survival").join("level.dat")).unwrap();
        assert_eq!(content, "данные survival");
        assert!(instance.join("saves").join("survival.pre-restore-test").is_dir());
        assert!(instance.join("saves").join("creative.pre-restore-test").is_dir());

        // список видит копию; prune с keep=1 удаляет старейшую.
        let listed = local_backups(&game, "beta").expect("список");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], json!("manual-copy"));
        let older = backups_root.join("older.zip");
        fs::write(&older, b"old").unwrap();
        let _ = fs::File::open(&older).unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(3600);
        let newer_time = SystemTime::now();
        set_mtime(&older, old_time);
        set_mtime(&archive, newer_time);
        prune_local(&game, &Instance { name: "beta".into(), path: instance.clone(), kind: "client".into() }, 1).expect("prune");
        assert!(!older.exists(), "старая копия удалена");
        assert!(archive.is_file(), "свежая копия осталась");
    }

    fn set_mtime(path: &Path, time: SystemTime) {
        let file = fs::File::options().write(true).open(path).expect("открыть");
        file.set_modified(time).expect("mtime");
    }

    #[test]
    fn restore_rejects_unsafe_archive_layouts() {
        let root = temp_root("unsafe");
        let worlds = root.join("saves");
        fs::create_dir_all(&worlds).expect("saves");
        let archive_path = root.join("bad.zip");
        for (name, payload) in [("worlds/ok/level.dat", b"x".as_slice()), ("__meta/escape", b"y".as_slice())] {
            let mut writer = zip::ZipWriter::new(fs::File::create(&archive_path).unwrap());
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(payload).unwrap();
            writer.finish().unwrap();
        }
        let error = restore_entries(&archive_path, &worlds, ".sfx").unwrap_err();
        assert_eq!(error, "Резервная копия содержит небезопасные пути");

        let trap = root.join("trap.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&trap).unwrap());
        writer.start_file("worlds/../escape.dat", zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(b"z").unwrap();
        writer.finish().unwrap();
        assert!(restore_entries(&trap, &worlds, ".sfx").is_err());
    }

    #[test]
    fn cloud_gate_follows_settings() {
        let root = temp_root("cloud");
        fs::create_dir_all(&root).expect("root");
        assert!(!cloud_configured(&root));
        fs::write(root.join("settings.json"), json!({ "cloud_url": "https://dav.example/remote.php/dav/files/u", "cloud_username": "u" }).to_string()).unwrap();
        assert!(cloud_configured(&root));
    }
}
