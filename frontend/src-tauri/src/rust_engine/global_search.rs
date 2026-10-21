// глобальный поиск по всем экземплярам: где установлен мод, в каком
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::time::Duration;

use super::config::ConfigStore;
use super::instances::{InstanceInfo, InstanceStore};

const MAX_RESULTS_PER_GROUP: usize = 50;
const MAX_JARS_PER_SCAN: usize = 512;
const SCAN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub(crate) struct JarManifest {
    pub(crate) loader: String,
    pub(crate) mod_id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) path: PathBuf,
}

// тот же nivora-turbo, что и у Python-воркера: ищем по env, соседним
// каталогам и системным путям
fn turbo_binary() -> Option<PathBuf> {
    super::turbo::turbo_binary()
}

// батч-скан манифестов: без бинарника или при сбое — пустой список,
// поиск просто остаётся на именах файлов
pub(crate) fn scan_jars(binary: &Path, jars: &[PathBuf]) -> Vec<JarManifest> {
    if jars.is_empty() {
        return Vec::new();
    }
    let child = super::platform::command(binary)
        .arg("scan-jars")
        .args(jars)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn();
    let Ok(child) = child else {
        return Vec::new();
    };
    let child = Arc::new(Mutex::new(child));
    let mut stdout = {
        let mut guard = child.lock().unwrap_or_else(|poison| poison.into_inner());
        guard.stdout.take()
    };
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watchdog = child.clone();
    std::thread::spawn(move || {
        if done_rx.recv_timeout(SCAN_TIMEOUT).is_err() {
            if let Ok(mut guard) = watchdog.lock() {
                let _ = guard.kill();
            }
        }
    });
    let mut output = String::new();
    if let Some(stdout) = stdout.as_mut() {
        let _ = stdout.read_to_string(&mut output);
    }
    let _ = done_tx.send(());
    let _ = {
        let mut guard = child.lock().unwrap_or_else(|poison| poison.into_inner());
        guard.wait()
    };
    let mut manifests = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 5 {
            continue;
        }
        let path = PathBuf::from(fields[4]);
        // строки с "-" в loader — jar без знакомого манифеста, не интересно
        if fields[0] == "-" {
            continue;
        }
        manifests.push(JarManifest {
            loader: fields[0].to_owned(),
            mod_id: fields[1].to_owned(),
            name: fields[2].to_owned(),
            version: fields[3].to_owned(),
            path,
        });
    }
    manifests
}

pub fn global_search(request: &Value) -> Result<Value, String> {
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if query.len() < 2 {
        return Ok(json!({ "query": query, "results": empty_groups(), "total": 0 }));
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let mut mods: Vec<Value> = Vec::new();
    let mut worlds: Vec<Value> = Vec::new();
    let mut resourcepacks: Vec<Value> = Vec::new();
    let mut shaderpacks: Vec<Value> = Vec::new();
    for info in store.list() {
        if info.game_edition == "bedrock" {
            continue;
        }
        let instance = json!({
            "instance": info.name,
            "instance_kind": info.kind,
            "game_edition": info.game_edition,
            "minecraft_version": info.minecraft_version,
        });
        let root = info.path.clone();
        if info.kind == "server" {
            collect_matches(&root.join("mods"), &query, &instance, "mod", &mut mods);
            collect_matches(&root.join("plugins"), &query, &instance, "plugin", &mut mods);
            collect_world_like(&root.join("world"), &query, &instance, &mut worlds);
            collect_matches(&root.join("resourcepacks"), &query, &instance, "resourcepack", &mut resourcepacks);
            collect_jar_manifests(&info, &query, &instance, &mut mods);
        } else {
            collect_matches(&root.join("mods"), &query, &instance, "mod", &mut mods);
            collect_matches(&root.join("resourcepacks"), &query, &instance, "resourcepack", &mut resourcepacks);
            collect_matches(&root.join("shaderpacks"), &query, &instance, "shaderpack", &mut shaderpacks);
            collect_worlds(&root.join("saves"), &query, &instance, &mut worlds);
            collect_jar_manifests(&info, &query, &instance, &mut mods);
        }
    }
    truncate_group(&mut mods);
    truncate_group(&mut worlds);
    truncate_group(&mut resourcepacks);
    truncate_group(&mut shaderpacks);
    let total = mods.len() + worlds.len() + resourcepacks.len() + shaderpacks.len();
    Ok(json!({
        "query": query,
        "results": {
            "mods": mods,
            "worlds": worlds,
            "resourcepacks": resourcepacks,
            "shaderpacks": shaderpacks,
        },
        "total": total,
    }))
}

fn empty_groups() -> Value {
    json!({ "mods": [], "worlds": [], "resourcepacks": [], "shaderpacks": [] })
}

// вторая линия поиска: имена модов ВНУТРИ jar-манифестов. Файл может
// называться «some-weird-name-1.2.jar», а мод внутри — Sodium: без этого
// прохода «где установлен мод» по внутреннему имени не находился
fn collect_jar_manifests(info: &InstanceInfo, query: &str, instance: &Value, output: &mut Vec<Value>) {
    if output.len() >= MAX_RESULTS_PER_GROUP {
        return;
    }
    let Some(binary) = turbo_binary() else {
        return;
    };
    let mut directories = vec![info.path.join("mods")];
    if info.kind == "server" {
        directories.push(info.path.join("plugins"));
    }
    let mut jars: Vec<PathBuf> = Vec::new();
    for directory in &directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_symlink() || !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !(name.ends_with(".jar") || name.ends_with(".jar.disabled")) {
                continue;
            }
            jars.push(path);
            if jars.len() >= MAX_JARS_PER_SCAN {
                break;
            }
        }
    }
    for manifest in scan_jars(&binary, &jars) {
        if output.len() >= MAX_RESULTS_PER_GROUP {
            return;
        }
        let mod_id = manifest.mod_id.trim();
        let name = manifest.name.trim();
        let matched = mod_id.to_lowercase().contains(query) || name.to_lowercase().contains(query);
        if !matched || mod_id.is_empty() || mod_id == "-" {
            continue;
        }
        let path_text = manifest.path.to_string_lossy().to_string();
        // файл уже найден по имени — не дублируем строку
        if output.iter().any(|item| item.get("path").and_then(Value::as_str) == Some(path_text.as_str())) {
            continue;
        }
        let file_name = manifest
            .path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let enabled = !file_name.to_lowercase().ends_with(".jar.disabled");
        let size = fs::metadata(&manifest.path).map(|meta| meta.len()).unwrap_or(0);
        let display_name = if name.is_empty() || name == "-" { mod_id } else { name };
        let mut item = instance.clone();
        if let Some(object) = item.as_object_mut() {
            object.insert("name".to_owned(), json!(display_name));
            object.insert("mod_id".to_owned(), json!(mod_id));
            object.insert("version".to_owned(), json!(manifest.version));
            object.insert("loader".to_owned(), json!(manifest.loader));
            object.insert("file_name".to_owned(), json!(file_name));
            object.insert("path".to_owned(), json!(path_text));
            object.insert("enabled".to_owned(), json!(enabled));
            object.insert("size_bytes".to_owned(), json!(size));
            object.insert("kind".to_owned(), json!("mod"));
        }
        output.push(item);
    }
}

fn collect_matches(directory: &Path, query: &str, instance: &Value, kind: &str, output: &mut Vec<Value>) {
    if output.len() >= MAX_RESULTS_PER_GROUP {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut items: Vec<(String, PathBuf, bool, u64)> = Vec::new();
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_symlink() || !(path.is_file() || path.is_dir()) {
            continue;
        }
        let raw_name = entry.file_name().to_string_lossy().to_string();
        let lowered = raw_name.to_lowercase();
        if lowered.starts_with('.') {
            continue;
        }
        let disabled = lowered.ends_with(".disabled");
        let enabled_name = if disabled { raw_name.trim_end_matches(".disabled").to_string() } else { raw_name.clone() };
        if !enabled_name.to_lowercase().contains(query) {
            continue;
        }
        let size = entry.metadata().map(|meta| if meta.is_dir() { 0 } else { meta.len() }).unwrap_or(0);
        items.push((enabled_name, path, !disabled, size));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path, enabled, size) in items {
        if output.len() >= MAX_RESULTS_PER_GROUP {
            return;
        }
        let mut item = instance.clone();
        if let Some(object) = item.as_object_mut() {
            object.insert("name".to_owned(), json!(name));
            object.insert("path".to_owned(), json!(path.to_string_lossy()));
            object.insert("enabled".to_owned(), json!(enabled));
            object.insert("size_bytes".to_owned(), json!(size));
            object.insert("kind".to_owned(), json!(kind));
        }
        output.push(item);
    }
}

fn collect_worlds(saves: &Path, query: &str, instance: &Value, output: &mut Vec<Value>) {
    if output.len() >= MAX_RESULTS_PER_GROUP {
        return;
    }
    let entries = match fs::read_dir(saves) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut items: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_symlink() || !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !name.to_lowercase().contains(query) {
            continue;
        }
        items.push((name, path));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in items {
        if output.len() >= MAX_RESULTS_PER_GROUP {
            return;
        }
        let mut item = instance.clone();
        if let Some(object) = item.as_object_mut() {
            object.insert("name".to_owned(), json!(name));
            object.insert("path".to_owned(), json!(path.to_string_lossy()));
            object.insert("kind".to_owned(), json!("world"));
        }
        output.push(item);
    }
}

// мир сервера — один каталог world, проверяем совпадением имени каталога
fn collect_world_like(world: &Path, query: &str, instance: &Value, output: &mut Vec<Value>) {
    if output.len() >= MAX_RESULTS_PER_GROUP || !world.is_dir() {
        return;
    }
    let name = world.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
    if name.to_lowercase().contains(query) {
        let mut item = instance.clone();
        if let Some(object) = item.as_object_mut() {
            object.insert("name".to_owned(), json!(name));
            object.insert("path".to_owned(), json!(world.to_string_lossy()));
            object.insert("kind".to_owned(), json!("world"));
        }
        output.push(item);
    }
}

fn truncate_group(group: &mut Vec<Value>) {
    group.truncate(MAX_RESULTS_PER_GROUP);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_query_returns_empty_groups() {
        let response = global_search(&json!({ "query": "a" })).unwrap();
        assert_eq!(response["total"], json!(0));
        assert!(response["results"]["mods"].as_array().unwrap().is_empty());
    }
}
