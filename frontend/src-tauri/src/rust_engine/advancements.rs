//! прогресс-борд модпака: читает advancements/<uuid>.json из сейвов экземпляра
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use super::config::ConfigStore;
use super::state::resolve_instance_path;

pub fn instance_advancements(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let game_dir = config.game_dir()?;
    let name = request
        .get("instance")
        .and_then(Value::as_str)
        .ok_or("Не указан экземпляр")?;
    let instance_path = resolve_instance_path(&game_dir, name).ok_or("Экземпляр не найден")?;
    let mut best: Option<(std::time::SystemTime, PathBuf, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(instance_path.join("saves")) {
        for world in entries.flatten() {
            let world_path = world.path();
            if !world_path.join("level.dat").is_file() {
                continue;
            }
            if let Ok(files) = fs::read_dir(world_path.join("advancements")) {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|value| value.to_str()) != Some("json") {
                        continue;
                    }
                    let modified = file
                        .metadata()
                        .and_then(|meta| meta.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    if best
                        .as_ref()
                        .map(|(time, _, _)| modified > *time)
                        .unwrap_or(true)
                    {
                        best = Some((modified, path, world_path.clone()));
                    }
                }
            }
        }
    }
    let (_, file, world_path) = best.ok_or("В экземпляре нет миров с достижениями")?;
    let raw =
        fs::read_to_string(&file).map_err(|error| format!("Достижения не читаются: {error}"))?;
    let data: Value =
        serde_json::from_str(&raw).map_err(|error| format!("Достижения повреждены: {error}"))?;
    let world_name = decode_nbt_name(&world_path).unwrap_or_else(|| {
        world_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    });

    let mut total: i64 = 0;
    let mut done: i64 = 0;
    let mut by_namespace: HashMap<String, i64> = HashMap::new();
    let mut by_namespace_total: HashMap<String, i64> = HashMap::new();
    let mut categories: HashMap<String, i64> = HashMap::new();
    let mut recent: Vec<(i64, String)> = Vec::new();
    if let Some(entries) = data.as_object() {
        for (id, entry) in entries {
            if id == "DataVersion" {
                continue;
            }
            total += 1;
            let (namespace, _, rest) = split_id(id);
            let is_minecraft = namespace == "minecraft";
            *by_namespace_total.entry(namespace.clone()).or_insert(0) += 1;
            if !entry.get("done").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            done += 1;
            *by_namespace.entry(namespace).or_insert(0) += 1;
            if is_minecraft {
                let category = rest.split('/').next().unwrap_or("прочее").to_owned();
                *categories.entry(category).or_insert(0) += 1;
            }
            let gained = entry
                .get("criteria")
                .and_then(Value::as_object)
                .map(|criteria| {
                    criteria
                        .values()
                        .filter_map(|value| value.get("gained").and_then(Value::as_i64))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            recent.push((gained, id.clone()));
        }
    }
    recent.sort_by(|a, b| b.0.cmp(&a.0));
    let recent: Vec<Value> = recent
        .into_iter()
        .take(8)
        .map(|(gained, id)| json!({ "id": id, "gained": gained }))
        .collect();
    let percent = if total > 0 { done * 100 / total } else { 0 };
    let vanilla_done = by_namespace.get("minecraft").copied().unwrap_or(0);
    let vanilla_total = by_namespace_total.get("minecraft").copied().unwrap_or(0);
    let mods_done = (done - vanilla_done).max(0);
    let mods_total = (total - vanilla_total).max(0);
    let mods_percent = if mods_total > 0 {
        mods_done * 100 / mods_total
    } else {
        0
    };
    Ok(json!({
        "world": world_name,
        "done": done,
        "total": total,
        "percent": percent,
        "mods_done": mods_done,
        "mods_total": mods_total,
        "mods_percent": mods_percent,
        "by_namespace_total": by_namespace_total,
        "by_namespace": by_namespace,
        "categories": categories,
        "recent": recent,
    }))
}

fn split_id(id: &str) -> (String, String, String) {
    match id.split_once(':') {
        Some((namespace, rest)) => (namespace.to_owned(), id.to_owned(), rest.to_owned()),
        None => ("minecraft".to_owned(), id.to_owned(), id.to_owned()),
    }
}

fn decode_nbt_name(world_path: &PathBuf) -> Option<String> {
    let raw = fs::read(world_path.join("level.dat")).ok()?;
    let value = super::world_preview::decode_nbt_bytes(&raw).ok()?;
    value
        .get("Data")
        .and_then(|data| data.get("LevelName"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}
