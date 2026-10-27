
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use super::instance_manifest::{build_manifest, InstanceManifest};
use super::platform_support::is_reparse_point;

/// переносятся только разрешённые категории, без миров и логов
const PORTABLE_KINDS: [&str; 4] = ["mod", "resourcepack", "shaderpack", "config"];
const MAX_PORT_ITEMS: usize = 2000;

fn safe_relative_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"^[^\\/]+(?:[\\/][^\\/]+)*$").expect("safe relative"))
}

/// ключ сопоставления модов: суффикс .disabled не различает файлы
fn mod_key(path: &str) -> &str {
    path.strip_suffix(".disabled").unwrap_or(path)
}

fn entry_str(entry: &Value, key: &str) -> String {
    entry.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn entry_int(entry: &Value, key: &str) -> i64 {
    entry.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn entry_summary(entry: &Value) -> Value {
    let path = entry_str(entry, "path");
    let name = {
        let stored = entry_str(entry, "name");
        if stored.is_empty() {
            Path::new(&path)
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            stored
        }
    };
    json!({
        "path": path,
        "name": name,
        "version": entry_str(entry, "version"),
        "loader": entry_str(entry, "loader"),
        "mod_id": entry_str(entry, "mod_id"),
        "size": entry_int(entry, "size"),
        "sha256": entry_str(entry, "sha256"),
        "enabled": !path.ends_with(".disabled"),
        "classes": entry_int(entry, "classes"),
    })
}

struct BucketDiff {
    added: Vec<Value>,
    removed: Vec<Value>,
    changed: Vec<Value>,
}

/// сравнение одной категории: key_fn сопоставляет переименованные файлы
/// (например, вкл/выкл мода через .disabled)
fn bucket_diff(
    source: &BTreeMap<String, Value>,
    target: &BTreeMap<String, Value>,
    compare_versions: bool,
    key_fn: Option<fn(&str) -> &str>,
) -> BucketDiff {
    let key_of: fn(&str) -> &str = key_fn.unwrap_or(|path| path);
    let mut source_keys: BTreeMap<String, &String> = BTreeMap::new();
    for path in source.keys() {
        source_keys.insert(key_of(path).to_owned(), path);
    }
    let mut target_keys: BTreeMap<String, &String> = BTreeMap::new();
    for path in target.keys() {
        target_keys.insert(key_of(path).to_owned(), path);
    }
    let mut diff = BucketDiff { added: Vec::new(), removed: Vec::new(), changed: Vec::new() };
    for (base, path) in &source_keys {
        let entry = &source[*path];
        let other_path = target_keys.get(base);
        let other = other_path.map(|other| &target[*other]);
        let Some(other) = other else {
            diff.added.push(entry_summary(entry));
            continue;
        };
        let same_hash = entry_str(entry, "sha256") == entry_str(other, "sha256");
        let same_size = entry.get("size") == other.get("size");
        if !same_hash || !same_size {
            let mut summary = entry_summary(entry);
            if compare_versions && (!entry_str(entry, "version").is_empty() || !entry_str(other, "version").is_empty()) {
                summary["from_version"] = json!(entry_str(other, "version"));
                summary["to_version"] = json!(entry_str(entry, "version"));
            }
            if key_fn.is_some() {
                summary["from_path"] = json!(entry_str(other, "path"));
                summary["to_path"] = json!(entry_str(entry, "path"));
            }
            diff.changed.push(summary);
        } else if key_fn.is_some() && entry_str(other, "path") != path.as_str() {
            // содержимое одинаковое, но файл переименован (вкл/выкл мода)
            let mut summary = entry_summary(entry);
            summary["from_path"] = json!(entry_str(other, "path"));
            summary["to_path"] = json!(entry_str(entry, "path"));
            diff.changed.push(summary);
        }
    }
    for (base, path) in &target_keys {
        if !source_keys.contains_key(base) {
            diff.removed.push(entry_summary(&target[*path]));
        }
    }
    diff.added.sort_by_key(|item| item["path"].as_str().unwrap_or_default().to_owned());
    diff.removed.sort_by_key(|item| item["path"].as_str().unwrap_or_default().to_owned());
    diff.changed.sort_by_key(|item| item["path"].as_str().unwrap_or_default().to_owned());
    diff
}

fn bucket_report(label: &str, diff: BucketDiff) -> Value {
    json!({
        "label": label,
        "added": diff.added,
        "removed": diff.removed,
        "changed": diff.changed,
    })
}

/// различия между манифестами исходного и целевого экземпляров
pub(crate) fn compare(source: &InstanceManifest, target: &InstanceManifest) -> Value {
    let mods = bucket_diff(&source.mods, &target.mods, true, Some(mod_key));
    // состояние включённости: одинаковый jar с разным суффиксом .disabled
    let mut mods_changed = mods.changed;
    for item in &mut mods_changed {
        let source_enabled = item["enabled"].as_bool().unwrap_or(true);
        let from_path = item
            .get("from_path")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| item["path"].as_str().unwrap_or_default())
            .to_owned();
        let from_enabled = !from_path.ends_with(".disabled");
        item["enabled_changed"] = json!(source_enabled != from_enabled);
    }
    let resourcepacks = bucket_report("resourcepacks", bucket_diff(&source.resourcepacks, &target.resourcepacks, true, None));
    let shaderpacks = bucket_report("shaderpacks", bucket_diff(&source.shaderpacks, &target.shaderpacks, true, None));
    let configs = bucket_report("configs", bucket_diff(&source.configs, &target.configs, false, None));

    let mut options_changed: Vec<Value> = Vec::new();
    let mut option_keys: Vec<&String> = source.options.keys().collect();
    option_keys.sort();
    for key in option_keys {
        let value = &source.options[key];
        let current = target.options.get(key).cloned().unwrap_or(json!(""));
        if current != *value {
            options_changed.push(json!({
                "key": key,
                "from": current.as_str().unwrap_or_default(),
                "to": value.as_str().unwrap_or_default(),
            }));
        }
    }
    let mut meta = Map::new();
    for (key, value) in &source.meta {
        let current = target.meta.get(key).cloned().unwrap_or(json!(""));
        if current != *value {
            meta.insert(
                key.clone(),
                json!({"from": current.as_str().unwrap_or_default(), "to": value.as_str().unwrap_or_default()}),
            );
        }
    }

    let count = |report: &Value| {
        report["added"].as_array().map(Vec::len).unwrap_or(0)
            + report["removed"].as_array().map(Vec::len).unwrap_or(0)
            + report["changed"].as_array().map(Vec::len).unwrap_or(0)
    };
    let summary = json!({
        "mods_added": mods.added.len(),
        "mods_removed": mods.removed.len(),
        "mods_changed": mods_changed.len(),
        "configs_changed": count(&configs),
        "packs_changed": count(&resourcepacks) + count(&shaderpacks),
    });
    json!({
        "mods": {"label": "mods", "added": mods.added, "removed": mods.removed, "changed": mods_changed},
        "resourcepacks": resourcepacks,
        "shaderpacks": shaderpacks,
        "configs": configs,
        "options": {"changed": options_changed},
        "meta": Value::Object(meta),
        "stats": {"source": Value::Object(source.stats()), "target": Value::Object(target.stats())},
        "summary": summary,
    })
}


/// каталог-владелец файла: mods / resourcepacks / shaderpacks / config
fn portable_root(kind: &str, relative: &str) -> String {
    let _ = kind;
    relative.split('/').next().unwrap_or_default().to_owned()
}

/// проверка, что относительный путь не уходит из корня через ссылки
fn resolved_within(root: &Path, relative: &str) -> Result<(), String> {
    let canonical_root = root.canonicalize().map_err(|_| "Путь выходит за каталог экземпляра".to_owned())?;
    let mut current = canonical_root.clone();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        current.push(part);
        if let Ok(meta) = fs::symlink_metadata(&current) {
            if meta.is_symlink() {
                let resolved = current
                    .canonicalize()
                    .map_err(|_| "Путь выходит за каталог экземпляра".to_owned())?;
                if !resolved.starts_with(&canonical_root) {
                    return Err("Путь выходит за каталог экземпляра".to_owned());
                }
                current = resolved;
            }
        }
    }
    Ok(())
}

fn instance_info(name: &str) -> Result<super::instances::InstanceInfo, String> {
    let config = super::config::ConfigStore::load()?;
    let store = super::instances::InstanceStore::new(&config)?;
    store.get(name)
}

/// копирует файлы между экземплярами, предварительно создавая резервную копию цели
pub(crate) fn port_changes(
    source: &InstanceManifest,
    target_root: &Path,
    items: &[Value],
    snapshot: &dyn Fn(&str),
) -> Result<Value, String> {
    if items.is_empty() {
        return Err("Не выбрано, что переносить".to_owned());
    }
    let mut copied: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut snapshotted: Vec<String> = Vec::new();
    let buckets = [
        ("mod", &source.mods),
        ("resourcepack", &source.resourcepacks),
        ("shaderpack", &source.shaderpacks),
        ("config", &source.configs),
    ];
    for item in items.iter().take(MAX_PORT_ITEMS) {
        let kind = item.get("kind").and_then(Value::as_str).unwrap_or_default();
        let path = item.get("path").and_then(Value::as_str).unwrap_or_default();
        let invalid = !PORTABLE_KINDS.contains(&kind)
            || !safe_relative_pattern().is_match(path)
            || path.split('/').any(|part| part == "..");
        if invalid {
            failed.push(json!({"path": path, "error": "Недопустимый путь"}));
            continue;
        }
        let bucket = buckets.iter().find(|(key, _)| *key == kind).map(|(_, value)| *value);
        let source_file = source.root.join(path);
        let entry = bucket.and_then(|bucket| bucket.get(path));
        if entry.is_none() || is_reparse_point(&source_file) || !source_file.is_file() {
            failed.push(json!({"path": path, "error": "Файл не найден в исходном экземпляре"}));
            continue;
        }
        let owner = portable_root(kind, path);
        if kind != "config" && !["mods", "resourcepacks", "shaderpacks"].contains(&owner.as_str()) {
            failed.push(json!({"path": path, "error": "Неизвестная категория файла"}));
            continue;
        }
        let destination = target_root.join(path);
        if let Err(error) = resolved_within(target_root, path) {
            failed.push(json!({"path": path, "error": error}));
            continue;
        }
        if ["mods", "resourcepacks", "shaderpacks"].contains(&owner.as_str()) && !snapshotted.contains(&owner) {
            snapshot(&owner);
            snapshotted.push(owner);
        }
        if let Some(parent) = destination.parent() {
            if fs::create_dir_all(parent).is_err() {
                failed.push(json!({"path": path, "error": "Не удалось создать каталог"}));
                continue;
            }
        }
        let destination_exists = destination.exists() || destination.is_symlink();
        if destination_exists && (destination.is_symlink() || !destination.is_file()) {
            failed.push(json!({"path": path, "error": "Целевой файл не заменяется"}));
            continue;
        }
        if destination_exists && fs::remove_file(&destination).is_err() {
            failed.push(json!({"path": path, "error": "Целевой файл не заменяется"}));
            continue;
        }
        match fs::copy(&source_file, &destination) {
            Ok(_) => copied.push(path.to_owned()),
            Err(error) => failed.push(json!({"path": path, "error": format!("Не удалось скопировать файл: {error}")})),
        }
    }
    snapshotted.sort();
    Ok(json!({"copied": copied, "failed": failed, "snapshotted": snapshotted}))
}


pub(crate) fn instance_compare(request: &Value) -> Result<Value, String> {
    let name_a = request.get("source").and_then(Value::as_str).unwrap_or_default().trim();
    let name_b = request.get("target").and_then(Value::as_str).unwrap_or_default().trim();
    if name_a.is_empty() || name_b.is_empty() {
        return Err("Выберите два экземпляра для сравнения".to_owned());
    }
    if name_a == name_b {
        return Err("Выберите два разных экземпляра".to_owned());
    }
    let info_a = instance_info(name_a)?;
    let info_b = instance_info(name_b)?;
    if info_a.game_edition != "java" || info_b.game_edition != "java" {
        return Err("Сравнение доступно для экземпляров Java Edition".to_owned());
    }
    let manifest_a = build_manifest(&info_a.path)?;
    let manifest_b = build_manifest(&info_b.path)?;
    let report = compare(&manifest_a, &manifest_b);
    Ok(json!({
        "source": info_a.public(),
        "target": info_b.public(),
        "report": report,
        "source_stats": Value::Object(manifest_a.stats()),
        "target_stats": Value::Object(manifest_b.stats()),
    }))
}

pub(crate) fn instance_port_changes(request: &Value) -> Result<Value, String> {
    let name_a = request.get("source").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let name_b = request.get("target").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let Some(items) = request.get("items").and_then(Value::as_array) else {
        return Err("Список переноса повреждён".to_owned());
    };
    let info_a = instance_info(&name_a)?;
    let info_b = instance_info(&name_b)?;
    if info_a.game_edition != "java" || info_b.game_edition != "java" {
        return Err("Перенос доступен для экземпляров Java Edition".to_owned());
    }
    if info_b.running || info_a.running {
        return Err("Остановите оба экземпляра перед переносом правок".to_owned());
    }
    let target_store = super::snapshots::store_for(&info_b.name)?;
    let snapshot = |owner: &str| {
        // снапшот категории цели перед перезаписью файлов
        let category = match owner {
            "shaderpacks" => "shaders",
            other => other,
        };
        let _ = target_store.create(category, 168, 20);
    };
    let manifest = build_manifest(&info_a.path)?;
    let result = port_changes(&manifest, &info_b.path, items, &snapshot)?;
    let copied = result["copied"].as_array().map(Vec::len).unwrap_or(0);
    super::events::emit_status_event("workspace-changed", &format!("Перенесено файлов: {copied}."));
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    Ok(json!({
        "copied": result["copied"],
        "failed": result["failed"],
        "snapshotted": result["snapshotted"],
        "target": info_b.public(),
        "details": workspace.snapshot(&info_b.name)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn manifest_with_mods(root: &Path, mods: Vec<(&str, &str, i64)>) -> InstanceManifest {
        let mut manifest = InstanceManifest {
            root: root.to_path_buf(),
            mods: BTreeMap::new(),
            resourcepacks: BTreeMap::new(),
            shaderpacks: BTreeMap::new(),
            configs: BTreeMap::new(),
            meta: Map::new(),
            options: Map::new(),
        };
        for (file, sha, size) in mods {
            manifest.mods.insert(
                format!("mods/{file}"),
                json!({"kind": "mod", "path": format!("mods/{file}"), "sha256": sha, "size": size, "version": "1.0", "name": file}),
            );
        }
        manifest
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("nivora-compare-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("создать каталог");
        directory
    }

    #[test]
    fn compare_reports_added_removed_changed_and_toggle() {
        let root = temp_dir("compare");
        let mut target = manifest_with_mods(&root, vec![("shared.jar", "zzz", 11), ("old.jar", "ddd", 40), ("toggled.jar", "ccc", 30)]);
        target.options.insert("maxFps".into(), json!("60"));
        let mut source = manifest_with_mods(
            &root,
            vec![("shared.jar", "aaa", 10), ("new.jar", "bbb", 20), ("toggled.jar.disabled", "ccc", 30)],
        );
        source.options.insert("maxFps".into(), json!("120"));
        source.options.insert("renderDistance".into(), json!("12"));
        let report = compare(&source, &target);

        let added: Vec<&str> = report["mods"]["added"].as_array().unwrap().iter().map(|item| item["path"].as_str().unwrap()).collect();
        assert_eq!(added, ["mods/new.jar"]);
        let removed: Vec<&str> = report["mods"]["removed"].as_array().unwrap().iter().map(|item| item["path"].as_str().unwrap()).collect();
        assert_eq!(removed, ["mods/old.jar"]);
        let changed: Vec<&str> = report["mods"]["changed"].as_array().unwrap().iter().map(|item| item["path"].as_str().unwrap()).collect();
        assert_eq!(changed, ["mods/shared.jar", "mods/toggled.jar.disabled"]);
        // shared.jar: версии проставлены (у обеих сторон 1.0)
        let shared = &report["mods"]["changed"][0];
        assert_eq!(shared["from_version"], json!("1.0"));
        assert_eq!(shared["to_version"], json!("1.0"));
        // toggled: переименование без смены содержимого
        let toggled = &report["mods"]["changed"][1];
        assert_eq!(toggled["from_path"], json!("mods/toggled.jar"));
        assert_eq!(toggled["to_path"], json!("mods/toggled.jar.disabled"));
        assert_eq!(toggled["enabled_changed"], json!(true));
        // опции: изменившееся значение и отсутствующее у цели
        let options = &report["options"]["changed"];
        assert_eq!(options.as_array().unwrap().len(), 2);
        assert_eq!(options[0]["key"], json!("maxFps"));
        assert_eq!(options[0]["from"], json!("60"));
        assert_eq!(options[0]["to"], json!("120"));
        assert_eq!(options[1]["key"], json!("renderDistance"));
        assert_eq!(options[1]["from"], json!(""));
        assert_eq!(report["summary"]["mods_added"], json!(1));
        assert_eq!(report["summary"]["mods_removed"], json!(1));
        assert_eq!(report["summary"]["mods_changed"], json!(2));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn port_changes_copies_and_snapshots() {
        let root = temp_dir("port");
        let source_root = root.join("a");
        let target_root = root.join("b");
        fs::create_dir_all(source_root.join("mods")).unwrap();
        fs::create_dir_all(target_root.join("mods")).unwrap();
        fs::write(source_root.join("mods/x.jar"), b"xxx").unwrap();
        fs::write(target_root.join("mods/old.jar"), b"old").unwrap();
        let source = manifest_with_mods(&source_root, vec![("x.jar", "aaa", 3)]);

        let owners = std::cell::RefCell::new(Vec::<String>::new());
        let result = port_changes(
            &source,
            &target_root,
            &[
                json!({"kind": "mod", "path": "mods/x.jar"}),
                json!({"kind": "mod", "path": "mods/missing.jar"}),
                json!({"kind": "world", "path": "saves/x"}),
                json!({"kind": "mod", "path": "../escape.jar"}),
            ],
            &|owner| owners.borrow_mut().push(owner.to_owned()),
        )
        .unwrap();
        assert_eq!(result["copied"], json!(["mods/x.jar"]));
        assert_eq!(result["snapshotted"], json!(["mods"]));
        assert_eq!(*owners.borrow(), ["mods".to_owned()]);
        assert_eq!(result["failed"].as_array().unwrap().len(), 3);
        assert_eq!(fs::read(target_root.join("mods/x.jar")).unwrap(), b"xxx");
        assert!(target_root.join("mods/old.jar").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn port_changes_rejects_escape_and_requires_selection() {
        let root = temp_dir("guards");
        let source = manifest_with_mods(&root, vec![("x.jar", "aaa", 3)]);
        assert_eq!(port_changes(&source, &root, &[], &|_| {}).unwrap_err(), "Не выбрано, что переносить");

        let items = [json!({"kind": "config", "path": "options.txt"})];
        let result = port_changes(&source, &root, &items, &|_| {}).unwrap();
        assert_eq!(result["failed"][0]["error"], json!("Файл не найден в исходном экземпляре"));
        let _ = fs::remove_dir_all(&root);
    }
}
