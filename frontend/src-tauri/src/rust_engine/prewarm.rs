//! мгновенный запуск: проверяет готовность экземпляра и прогревает файлы в кэше ОС,
//! чтобы нажатие «Играть» не ждало чтения гигабайтов с диска
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::config::ConfigStore;
use super::state::resolve_instance_path;

const HEAD_BYTES: u64 = 32 * 1024 * 1024;
const TAIL_BYTES: u64 = 8 * 1024 * 1024;
const PER_LIBRARY_BYTES: u64 = 4 * 1024 * 1024;
const WARM_BUDGET: u64 = 512 * 1024 * 1024;

fn newest_version_json(instance_dir: &Path) -> Option<(PathBuf, String)> {
    let versions = instance_dir.join("versions");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&versions) {
        for dir in entries.flatten() {
            if !dir.path().is_dir() {
                continue;
            }
            if let Ok(files) = fs::read_dir(dir.path()) {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|value| value.to_str()) == Some("json") {
                        let modified = file
                            .metadata()
                            .and_then(|meta| meta.modified())
                            .unwrap_or(std::time::UNIX_EPOCH);
                        if best
                            .as_ref()
                            .map(|(time, _)| modified > *time)
                            .unwrap_or(true)
                        {
                            best = Some((modified, path));
                        }
                    }
                }
            }
        }
    }
    let (_, path) = best?;
    let version_id = path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|value| value.to_string_lossy().into_owned())?;
    Some((path, version_id))
}

fn warm_file(path: &Path, head: u64, tail: u64, budget: &mut u64) -> u64 {
    if *budget == 0 {
        return 0;
    }
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let size = meta.len();
    if size == 0 {
        return 0;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return 0;
    };
    let mut warmed = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    let mut read_range =
        |file: &mut fs::File, offset: u64, want: u64, budget: &mut u64, warmed: &mut u64| {
            if file.seek(SeekFrom::Start(offset)).is_err() {
                return;
            }
            let mut remaining = want.min(*budget);
            while remaining > 0 {
                let chunk = buf.len().min(remaining as usize);
                match file.read(&mut buf[..chunk]) {
                    Ok(0) => break,
                    Ok(read) => {
                        *warmed += read as u64;
                        *budget -= read as u64;
                        remaining -= read as u64;
                    }
                    Err(_) => break,
                }
            }
        };
    read_range(&mut file, 0, head.min(size), budget, &mut warmed);
    if size > head {
        read_range(
            &mut file,
            size - tail.min(size - head),
            tail,
            budget,
            &mut warmed,
        );
    }
    warmed
}

fn library_paths(version: &Value, roots: &[PathBuf]) -> (usize, usize, Vec<PathBuf>) {
    let entries = version
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut present: Vec<PathBuf> = Vec::new();
    let mut missing = 0usize;
    for entry in &entries {
        let Some(relative) = entry
            .get("downloads")
            .and_then(|downloads| downloads.get("artifact"))
            .and_then(|artifact| artifact.get("path"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let mut found = false;
        for root in roots {
            let candidate = root.join(relative);
            if candidate.is_file() {
                present.push(candidate);
                found = true;
                break;
            }
        }
        if !found {
            missing += 1;
        }
    }
    (entries.len(), missing, present)
}

fn assets_report(version: &Value, game_dir: &Path) -> Option<(usize, usize)> {
    let index_id = version
        .get("assetIndex")
        .and_then(|asset| asset.get("id"))
        .and_then(Value::as_str)?;
    let index_path = game_dir
        .join("assets")
        .join("indexes")
        .join(format!("{index_id}.json"));
    let raw = fs::read_to_string(&index_path).ok()?;
    let index: Value = serde_json::from_str(&raw).ok()?;
    let objects = index.get("objects").and_then(Value::as_object)?;
    let total = objects.len();
    let mut missing = 0usize;
    for (_name, object) in objects {
        let Some(hash) = object.get("hash").and_then(Value::as_str) else {
            missing += 1;
            continue;
        };
        if !game_dir
            .join("assets")
            .join("objects")
            .join(&hash[..2])
            .join(hash)
            .is_file()
        {
            missing += 1;
        }
    }
    Some((total, missing))
}

pub fn instance_prewarm(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let game_dir = config.game_dir()?;
    let name = request
        .get("instance")
        .and_then(Value::as_str)
        .ok_or("Не указан экземпляр")?;
    let instance_path = resolve_instance_path(&game_dir, name).ok_or("Экземпляр не найден")?;
    let (json_path, version_id) = newest_version_json(&instance_path)
        .ok_or("В экземпляре нет установленной версии для прогрева")?;
    let raw =
        fs::read_to_string(&json_path).map_err(|error| format!("Версия не читается: {error}"))?;
    let version: Value =
        serde_json::from_str(&raw).map_err(|error| format!("Версия повреждена: {error}"))?;
    let mut budget = WARM_BUDGET;
    let mut warmed = 0u64;

    let client_jar = json_path.with_file_name(format!("{version_id}.jar"));
    let client_jar_ready = client_jar.is_file();
    if client_jar_ready {
        warmed += warm_file(&client_jar, HEAD_BYTES, TAIL_BYTES, &mut budget);
    }

    let library_roots = vec![game_dir.join("libraries"), instance_path.join("libraries")];
    let (total_libraries, missing_libraries, present_libraries) =
        library_paths(&version, &library_roots);
    for path in &present_libraries {
        warmed += warm_file(path, PER_LIBRARY_BYTES, 0, &mut budget);
    }

    let assets = assets_report(&version, &game_dir);

    Ok(json!({
        "version": version_id,
        "client_jar_ready": client_jar_ready,
        "total_libraries": total_libraries,
        "missing_libraries": missing_libraries,
        "assets_total": assets.map(|(total, _)| total),
        "assets_missing": assets.map(|(_, missing)| missing),
        "warmed_bytes": warmed,
    }))
}
