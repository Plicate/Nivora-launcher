//! Наборы модов: переключаемые профили включённых/выключенных модов инстанса.
//!
//! Состояние хранится в файле `.nivora-mod-profiles.json` в корне экземпляра.
//! Набор фиксирует для каждого мода, включён ли он; применение набора
//! переименовывает файлы в `mods` (суффикс `.disabled`) и двигает метаданные
//! рядом, как это делает обычное переключение мода.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::config::ConfigStore;
use super::instance_workspace::{acquire_stopped_instance_lock, InstanceWorkspace};

const PROFILES_FILE: &str = ".nivora-mod-profiles.json";
const DISABLED_SUFFIX: &str = ".disabled";
const MAX_PROFILES: usize = 20;
const MAX_LABEL_CHARS: usize = 60;

fn profiles_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(PROFILES_FILE)
}

fn load_state(instance_dir: &Path) -> (String, Vec<Value>) {
    let raw = fs::read_to_string(profiles_path(instance_dir)).unwrap_or_default();
    let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let active = parsed
        .get("active")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let profiles = parsed
        .get("profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (active, profiles)
}

fn save_state(instance_dir: &Path, active: &str, profiles: &[Value]) -> Result<(), String> {
    let payload = json!({ "version": 1, "active": active, "profiles": profiles });
    let rendered =
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{\"version\":1}".to_owned());
    fs::write(profiles_path(instance_dir), rendered)
        .map_err(|error| format!("Не удалось сохранить наборы модов: {error}"))
}

fn logical_name(file_name: &str) -> &str {
    file_name.strip_suffix(DISABLED_SUFFIX).unwrap_or(file_name)
}

fn is_symlink_path(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn sidecar_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.nivora-mod"))
}

fn scan_mods(directory: &Path) -> Result<Vec<(String, bool)>, String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Не удалось открыть папку mods: {error}"))?;
    let mut mods = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        if is_symlink_path(&path) {
            continue;
        }
        let enabled = !name.ends_with(DISABLED_SUFFIX);
        mods.push((logical_name(&name).to_owned(), enabled));
    }
    mods.sort();
    Ok(mods)
}

fn clean_label(raw: &str) -> Result<String, String> {
    let label = raw.trim();
    if label.is_empty() {
        return Err("Введите название набора".to_owned());
    }
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err("Название набора слишком длинное".to_owned());
    }
    Ok(label.to_owned())
}

fn profiles_payload(active: &str, profiles: &[Value]) -> Value {
    let list: Vec<Value> = profiles
        .iter()
        .map(|profile| {
            let mods = profile
                .get("mods")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let count = mods.len();
            let enabled_count = mods
                .iter()
                .filter(|entry| {
                    entry
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count();
            json!({
                "name": profile.get("name").and_then(Value::as_str).unwrap_or_default(),
                "created": profile.get("created").and_then(Value::as_i64).unwrap_or(0),
                "count": count,
                "enabled_count": enabled_count,
            })
        })
        .collect();
    json!({ "active": active, "profiles": list })
}

fn find_profile<'a>(profiles: &'a [Value], label: &str) -> Option<&'a Value> {
    profiles
        .iter()
        .find(|profile| profile.get("name").and_then(Value::as_str) == Some(label))
}

fn profile_mods(profile: &Value) -> Vec<(String, bool)> {
    profile
        .get("mods")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let file = entry.get("file").and_then(Value::as_str)?;
                    let enabled = entry.get("enabled").and_then(Value::as_bool)?;
                    Some((file.to_owned(), enabled))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn apply_profile(directory: &Path, mods: &[(String, bool)]) -> Result<usize, String> {
    let mut changed = 0usize;
    for (logical, enabled) in mods {
        let disabled_name = format!("{logical}{DISABLED_SUFFIX}");
        let current = if directory.join(&disabled_name).is_file() {
            disabled_name.clone()
        } else if directory.join(logical).is_file() {
            logical.clone()
        } else {
            continue;
        };
        let target = if *enabled {
            logical.clone()
        } else {
            disabled_name
        };
        if current == target {
            continue;
        }
        let source = directory.join(&current);
        let destination = directory.join(&target);
        if is_symlink_path(&source) || is_symlink_path(&destination) {
            return Err(format!("Файл мода {logical} является символьной ссылкой"));
        }
        if destination.exists() {
            return Err(format!("Файл уже существует: {target}"));
        }
        fs::rename(&source, &destination)
            .map_err(|error| format!("Не удалось переключить {logical}: {error}"))?;
        // Метаданные едут вместе с файлом; «залежавшийся» сайдкар под новым
        // именем уже указывает на актуальное имя и остаётся на месте.
        let sidecar = sidecar_path(&source);
        let next_sidecar = sidecar_path(&destination);
        if sidecar.is_file() && !is_symlink_path(&sidecar) && !is_symlink_path(&next_sidecar) {
            let _ = fs::rename(&sidecar, next_sidecar);
        }
        changed += 1;
    }
    Ok(changed)
}

fn instance_context(
    request: &Value,
) -> Result<(InstanceWorkspace, super::instances::InstanceInfo), String> {
    let workspace = InstanceWorkspace::new()?;
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let info = workspace.java_instance(name)?;
    Ok((workspace, info))
}

pub(crate) fn mod_profiles_get(request: &Value) -> Result<Value, String> {
    let (_workspace, info) = instance_context(request)?;
    let (active, profiles) = load_state(&info.path);
    Ok(profiles_payload(&active, &profiles))
}

pub(crate) fn mod_profiles_save(request: &Value) -> Result<Value, String> {
    let (workspace, info) = instance_context(request)?;
    let label = clean_label(
        request
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let config = ConfigStore::load()?;
    let _lock = acquire_stopped_instance_lock(&info.name, info.running, &config.paths.data_home)?;
    let directory = workspace.asset_directory(&info.name, "mods")?;
    let mods = scan_mods(&directory)?;
    if mods.is_empty() {
        return Err("В папке mods нет модов, нечего сохранять".to_owned());
    }
    let entries: Vec<Value> = mods
        .iter()
        .map(|(file, enabled)| json!({ "file": file, "enabled": enabled }))
        .collect();
    let (active, mut profiles) = load_state(&info.path);
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    match find_profile(&profiles, &label) {
        Some(existing) => {
            let mut updated = existing.clone();
            updated["mods"] = Value::Array(entries);
            updated["created"] = json!(created);
            if let Some(position) = profiles
                .iter()
                .position(|profile| profile.get("name").and_then(Value::as_str) == Some(&label))
            {
                profiles[position] = updated;
            }
        }
        None => {
            if profiles.len() >= MAX_PROFILES {
                return Err(format!("Наборов не может быть больше {MAX_PROFILES}"));
            }
            profiles.push(json!({ "name": label, "created": created, "mods": entries }));
        }
    }
    save_state(&info.path, &active, &profiles)?;
    Ok(profiles_payload(&active, &profiles))
}

pub(crate) fn mod_profiles_apply(request: &Value) -> Result<Value, String> {
    let (workspace, info) = instance_context(request)?;
    let label = clean_label(
        request
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let config = ConfigStore::load()?;
    let _lock = acquire_stopped_instance_lock(&info.name, info.running, &config.paths.data_home)?;
    let (active, profiles) = load_state(&info.path);
    let profile = find_profile(&profiles, &label)
        .ok_or_else(|| format!("Набор «{label}» не найден"))?
        .clone();
    let directory = workspace.asset_directory(&info.name, "mods")?;
    let mods = profile_mods(&profile);
    let changed = apply_profile(&directory, &mods)?;
    save_state(&info.path, &label, &profiles)?;
    let mut payload = profiles_payload(&label, &profiles);
    payload["changed"] = json!(changed);
    Ok(payload)
}

pub(crate) fn mod_profiles_delete(request: &Value) -> Result<Value, String> {
    let (_workspace, info) = instance_context(request)?;
    let label = clean_label(
        request
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let config = ConfigStore::load()?;
    let _lock = acquire_stopped_instance_lock(&info.name, info.running, &config.paths.data_home)?;
    let (active, profiles) = load_state(&info.path);
    if find_profile(&profiles, &label).is_none() {
        return Err(format!("Набор «{label}» не найден"));
    }
    let remaining: Vec<Value> = profiles
        .into_iter()
        .filter(|profile| profile.get("name").and_then(Value::as_str) != Some(&label))
        .collect();
    let next_active = if active == label {
        String::new()
    } else {
        active
    };
    save_state(&info.path, &next_active, &remaining)?;
    Ok(profiles_payload(&next_active, &remaining))
}
