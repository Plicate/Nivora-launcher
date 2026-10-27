
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use super::MAX_MODS;

/// сведения о моде из каталога mods
#[derive(Clone, Debug)]
pub(crate) struct ModEntry {
    pub(crate) file: String,
    pub(crate) mod_id: String,
    pub(crate) enabled: bool,
    /// загрузчики, найденные по манифестам внутри jar
    pub(crate) loaders: BTreeSet<String>,
    /// конфиги миксинов в корне архива (*.mixins.json)
    pub(crate) mixins: BTreeSet<String>,
    /// префиксы пакетов классов: по ним стек ошибки сводится к файлу
    pub(crate) packages: BTreeSet<String>,
    /// зависимости из манифеста: [{id, constraint, optional}]
    pub(crate) dependencies: Vec<Value>,
    pub(crate) mtime: f64,
    pub(crate) corrupted: bool,
}

/// собирает поля кандидата в ModEntry
trait IntoCandidate {
    fn parts(self) -> (String, bool, f64);
}

struct Candidate {
    file: String,
    enabled: bool,
    mtime: f64,
}

impl IntoCandidate for Candidate {
    fn parts(self) -> (String, bool, f64) {
        (self.file, self.enabled, self.mtime)
    }
}

/// сканирует архивы из mods, сначала через nivora-turbo, затем средствами Rust
/// пропускает ссылки и посторонние файлы, результаты сортирует по имени
pub(crate) fn scan_mods(instance_path: &Path) -> Vec<ModEntry> {
    let directory = instance_path.join("mods");
    let Ok(dir_meta) = fs::symlink_metadata(&directory) else {
        return Vec::new();
    };
    if dir_meta.is_symlink() || !dir_meta.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());

    // собираем подходящие файлы и время их изменения
    let mut candidates: Vec<Candidate> = Vec::new();
    for name in names {
        if candidates.len() >= MAX_MODS {
            break;
        }
        let lowered = name.to_lowercase();
        let enabled = !lowered.ends_with(".disabled");
        let base = lowered.strip_suffix(".disabled").unwrap_or(&lowered);
        if !base.ends_with(".jar") && !base.ends_with(".zip") {
            continue;
        }
        let path = directory.join(&name);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        candidates.push(Candidate { file: name, enabled, mtime });
    }

    // сканируем архивы одним вызовом nivora-turbo
    let paths: Vec<std::path::PathBuf> = candidates.iter().map(|item| directory.join(&item.file)).collect();
    let scans = super::turbo::scan_mods(&paths);
    candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| match &scans {
            Some(list) => mod_entry_from_scan(candidate, list.get(index).cloned().unwrap_or_default()),
            None => mod_entry_fallback(candidate, &paths[index]),
        })
        .collect()
}

fn mod_entry_from_scan(candidate: impl IntoCandidate, scan: super::turbo::ModScan) -> ModEntry {
    let (file, enabled, mtime) = candidate.parts();
    // выключенные и повреждённые моды не участвуют в анализе зависимостей
    let mod_id = if enabled && !scan.corrupted { scan.mod_id.to_lowercase() } else { String::new() };
    ModEntry {
        file,
        mod_id,
        enabled,
        loaders: scan.loaders.iter().cloned().collect(),
        mixins: scan.mixins.iter().cloned().collect(),
        packages: scan.packages.iter().cloned().collect(),
        dependencies: Vec::new(),
        mtime,
        corrupted: scan.corrupted,
    }
}

/// запасной разбор архивов средствами Rust
fn mod_entry_fallback(candidate: impl IntoCandidate, path: &Path) -> ModEntry {
    let (file, enabled, mtime) = candidate.parts();
    let mut loaders = BTreeSet::new();
    let mut mixins = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let mut manifest: Option<Map<String, Value>> = None;
    let mut corrupted = false;
    match fs::File::open(path) {
        Ok(handle) => match zip::ZipArchive::new(handle) {
            Ok(mut archive) => {
                for index in 0..archive.len() {
                    let Ok(member) = archive.by_index(index) else { continue };
                    let lowered = member.name().map(|v| v.to_lowercase()).unwrap_or_default();
                    match lowered.as_str() {
                        "fabric.mod.json" => {
                            loaders.insert("fabric".to_owned());
                        }
                        "quilt.mod.json" => {
                            loaders.insert("quilt".to_owned());
                        }
                        "meta-inf/neoforge.mods.toml" => {
                            loaders.insert("neoforge".to_owned());
                        }
                        "meta-inf/mods.toml" | "mcmod.info" => {
                            loaders.insert("forge".to_owned());
                        }
                        _ => {}
                    }
                    if !lowered.contains('/')
                        && lowered.ends_with(".json")
                        && lowered.contains("mixin")
                        && mixins.len() < 8
                    {
                        mixins.insert(lowered.clone());
                    }
                    if packages.len() < 16 && lowered.ends_with(".class") && !lowered.starts_with("meta-inf/") {
                        // сохраняем до трёх сегментов пакета, например com/example/mod
                        let slashes: Vec<usize> = lowered
                            .match_indices('/')
                            .map(|(offset, _)| offset)
                            .collect();
                        if slashes.len() >= 2 {
                            let end = slashes[slashes.len().min(3) - 1];
                            packages.insert(lowered[..end].to_owned());
                        }
                    }
                }
                if enabled {
                    manifest = super::instance_manifest::jar_manifest_metadata(path)
                        .and_then(|value| value.as_object().cloned());
                }
            }
            Err(_) => corrupted = true,
        },
        Err(_) => corrupted = true,
    }
    let mod_id = manifest
        .as_ref()
        .and_then(|fields| fields.get("mod_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    ModEntry {
        file,
        mod_id,
        enabled,
        loaders,
        mixins,
        packages,
        dependencies: Vec::new(),
        mtime,
        corrupted,
    }
}

/// сканирует моды и их зависимости для диагностики; автонастройке достаточно загрузчиков и пакетов
pub(crate) fn scan_mods_full(instance_path: &Path) -> Vec<ModEntry> {
    let directory = instance_path.join("mods");
    let mut mods = scan_mods(instance_path);
    for entry in mods.iter_mut() {
        if !entry.enabled {
            continue; // выключенные моды в анализе зависимостей не участвуют
        }
        if let Some(manifest) = super::mod_dependencies::read_mod_manifest(&directory.join(&entry.file)) {
            if let Some(dependencies) = manifest.get("dependencies").and_then(Value::as_array) {
                entry.dependencies = dependencies.clone();
            }
        }
    }
    mods
}

#[allow(dead_code)] // используется платформенными ветками
fn stem_of(file: &str) -> String {
    let trimmed = file.strip_suffix(".disabled").unwrap_or(file);
    trimmed
        .strip_suffix(".jar")
        .or_else(|| trimmed.strip_suffix(".zip"))
        .unwrap_or(trimmed)
        .to_owned()
}

/// выявляет моды для другого загрузчика; Fabric-моды допустимы на Forge при наличии Sinytra Connector
pub(crate) fn wrong_loader(mod_entry: &ModEntry, loader: &str, connector_present: bool) -> bool {
    if mod_entry.loaders.is_empty() || mod_entry.corrupted {
        return false;
    }
    let has = |name: &str| mod_entry.loaders.contains(name);
    let fabric_like = has("fabric") || has("quilt");
    match loader {
        "fabric" => !has("fabric"),
        "quilt" => !fabric_like,
        "forge" => !(connector_present && fabric_like) && !has("forge"),
        "neoforge" => !(connector_present && fabric_like) && !(has("forge") || has("neoforge")),
        _ => false,
    }
}

