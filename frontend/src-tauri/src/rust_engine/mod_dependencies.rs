// зависимости модов из манифестов в JAR-файлах
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::config::ConfigStore;
use super::instances::InstanceStore;

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_MODS: usize = 600;
const PROJECT_METADATA_SUFFIX: &str = ".trial-project.json";
const NOISE_IDS: &[&str] = &[
    "minecraft",
    "fabricloader",
    "fabric-loader",
    "fabric",
    "quilt_loader",
    "quilt-loader",
    "quilt_base",
    "quilted_fabric_api",
    "forge",
    "neoforge",
    "minecraftforge",
    "java",
    "cloth-config",
    "cloth_config",
    "architectury",
];

fn py_str(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => if *flag { "True" } else { "False" }.to_owned(),
        Some(other) => other.to_string(),
    }
}

fn sget(value: &Value, key: &str) -> String {
    py_str(value.get(key))
}

fn py_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().map(|raw| raw != 0.0).unwrap_or(false),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(fields)) => !fields.is_empty(),
    }
}

fn re_toml_mod_id() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^modId\s*=\s*[\"']([\w.-]+)"#).unwrap())
}

fn re_toml_display() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^displayName\s*=\s*[\"']([^\"']+)"#).unwrap())
}

fn re_toml_version() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^version\s*=\s*[\"']([^\"']+)"#).unwrap())
}

fn re_toml_block_start() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[dependencies\.[\w.-]+\]\]").unwrap())
}

fn re_toml_dep_id() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"modId\s*=\s*[\"']([\w.-]+)"#).unwrap())
}

fn re_toml_kind() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"type\s*=\s*[\"'](\w+)"#).unwrap())
}

fn re_toml_mandatory() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"mandatory\s*=\s*(\w+)").unwrap())
}

fn re_toml_range() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"versionRange\s*=\s*[\"']([^\"']*)"#).unwrap())
}

fn re_mcmod_token() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"([A-Za-z][\w.-]*)").unwrap())
}

fn read_zip_member(archive: &mut zip::ZipArchive<File>, name: &str) -> Vec<u8> {
    let Ok(entry) = archive.by_name(name) else { return Vec::new() };
    if entry.is_dir() || entry.size() > MAX_MANIFEST_BYTES {
        return Vec::new();
    }
    let mut buffer = Vec::new();
    // ограничение ещё и на чтение: заголовок size в архиве мог солгать
    let mut limited = entry.take(MAX_MANIFEST_BYTES + 1);
    if limited.read_to_end(&mut buffer).is_err() || buffer.len() as u64 > MAX_MANIFEST_BYTES {
        return Vec::new();
    }
    buffer
}

fn fabric_manifest(raw: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_str(&String::from_utf8_lossy(raw)).ok()?;
    let Some(fields) = value.as_object() else { return None };
    let entries = |mapping: &Map<String, Value>, optional: bool| -> Vec<Value> {
        mapping
            .iter()
            .map(|(dep_id, constraint)| {
                let text = match constraint {
                    Value::String(raw) => raw.clone(),
                    _ => "любая".to_owned(),
                };
                json!({
                    "id": dep_id,
                    "constraint": text.chars().take(80).collect::<String>(),
                    "optional": optional,
                })
            })
            .collect()
    };
    let empty = Map::new();
    let depends = fields.get("depends").and_then(Value::as_object).unwrap_or(&empty);
    let suggests = fields.get("suggests").and_then(Value::as_object).unwrap_or(&empty);
    let breaks: Vec<Value> = fields
        .get("breaks")
        .and_then(Value::as_object)
        .map(|mapping| mapping.keys().map(|key| json!(key)).collect())
        .unwrap_or_default();
    let mut dependencies = entries(depends, false);
    dependencies.extend(entries(suggests, true));
    let mod_id = sget(&value, "id");
    let name = {
        let raw = sget(&value, "name");
        if raw.is_empty() { mod_id.clone() } else { raw }
    };
    Some(json!({
        "mod_id": mod_id,
        "name": name,
        "version": sget(&value, "version"),
        "loader": "fabric",
        "dependencies": dependencies,
        "breaks": breaks,
    }))
}

fn quilt_manifest(raw: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_str(&String::from_utf8_lossy(raw)).ok()?;
    let loader = value.get("quilt_loader").and_then(Value::as_object)?;
    let metadata = loader.get("metadata").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut dependencies: Vec<Value> = Vec::new();
    for item in loader.get("depends").and_then(Value::as_array).unwrap_or(&Vec::new()) {
        match item {
            Value::String(dep_id) => dependencies.push(json!({ "id": dep_id, "constraint": "любая", "optional": false })),
            Value::Object(fields) if py_truthy(fields.get("id")) => {
                let constraint = fields.get("versions");
                let text = match constraint {
                    Some(Value::Array(list)) => list.iter().map(|entry| py_str(Some(entry))).collect::<Vec<_>>().join(","),
                    _ => {
                        if py_truthy(constraint) {
                            py_str(constraint)
                        } else {
                            "любая".to_owned()
                        }
                    }
                };
                let optional = sget_ref(fields, "optional").to_lowercase() == "true"
                    || (py_truthy(fields.get("unless")) && !py_truthy(constraint));
                dependencies.push(json!({
                    "id": py_str(fields.get("id")),
                    "constraint": text.chars().take(80).collect::<String>(),
                    "optional": optional,
                }));
            }
            _ => {}
        }
    }
    let loader_id = sget_ref(loader, "id");
    let name = {
        let raw = sget_ref(&metadata, "name");
        if raw.is_empty() { loader_id.clone() } else { raw }
    };
    Some(json!({
        "mod_id": loader_id,
        "name": name,
        "version": sget_ref(loader, "version"),
        "loader": "quilt",
        "dependencies": dependencies,
        "breaks": [],
    }))
}

fn sget_ref(fields: &Map<String, Value>, key: &str) -> String {
    py_str(fields.get(key))
}

fn toml_manifest(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let mod_id = re_toml_mod_id().captures(&text)?.get(1)?.as_str().to_owned();
    let display = re_toml_display().captures(&text).and_then(|captures| captures.get(1)).map(|m| m.as_str().to_owned());
    let version = re_toml_version().captures(&text).and_then(|captures| captures.get(1)).map(|m| m.as_str().to_owned());
    let loader = if text.to_lowercase().contains("neoforge") { "neoforge" } else { "forge" };
    let mut dependencies: Vec<Value> = Vec::new();
    // блок заканчивается перед следующей секцией TOML
    let markers: Vec<usize> = re_toml_block_start().find_iter(&text).map(|found| found.end()).collect();
    for start in markers {
        let body_end = text[start..]
            .find("\n[")
            .map(|offset| start + offset)
            .unwrap_or(text.len());
        let body = &text[start..body_end];
        let Some(dep_id) = re_toml_dep_id().captures(body).and_then(|captures| captures.get(1)) else { continue };
        let kind = re_toml_kind().captures(body).and_then(|captures| captures.get(1)).map(|m| m.as_str().to_owned());
        let mandatory = re_toml_mandatory().captures(body).and_then(|captures| captures.get(1)).map(|m| m.as_str().to_owned());
        let constraint = re_toml_range()
            .captures(body)
            .and_then(|captures| captures.get(1))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_default();
        let optional = if let Some(kind) = kind {
            kind.to_lowercase() != "required"
        } else if let Some(mandatory) = mandatory {
            mandatory.to_lowercase() != "true"
        } else {
            false
        };
        dependencies.push(json!({
            "id": dep_id.as_str(),
            "constraint": constraint.chars().take(80).collect::<String>(),
            "optional": optional,
        }));
    }
    Some(json!({
        "mod_id": mod_id,
        "name": display.unwrap_or_else(|| mod_id.clone()),
        "version": version.unwrap_or_default(),
        "loader": loader,
        "dependencies": dependencies,
        "breaks": [],
    }))
}

fn mcmod_manifest(raw: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_str(&String::from_utf8_lossy(raw)).ok()?;
    let item = match &value {
        Value::Array(items) if !items.is_empty() => &items[0],
        other => other,
    };
    let fields = item.as_object()?;
    let mut dependencies: Vec<Value> = Vec::new();
    for token in fields.get("useDependencies").and_then(Value::as_array).unwrap_or(&Vec::new()) {
        let text = py_str(Some(token));
        if let Some(captures) = re_mcmod_token().captures(&text) {
            let optional = text.starts_with("after:") || text.starts_with("after-");
            dependencies.push(json!({
                "id": captures[1],
                "constraint": text.chars().take(80).collect::<String>(),
                "optional": optional,
            }));
        }
    }
    let mod_id = {
        let raw = sget(item, "modid");
        if raw.is_empty() { sget(item, "name") } else { raw }
    };
    Some(json!({
        "mod_id": mod_id,
        "name": sget(item, "name"),
        "version": sget(item, "version"),
        "loader": "forge",
        "dependencies": dependencies,
        "breaks": [],
    }))
}

pub(crate) fn read_mod_manifest(path: &Path) -> Option<Value> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.is_symlink() || !metadata.is_file() {
        return None;
    }
    let suffix = path.extension()?.to_string_lossy().to_lowercase();
    if suffix != "jar" && suffix != "zip" {
        return None;
    }
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let names: HashSet<String> = archive.file_names().filter_map(|name| name.ok()).map(|name| name.to_string()).collect();
    let parsers: &[(&str, fn(&[u8]) -> Option<Value>)] = &[
        ("fabric.mod.json", fabric_manifest),
        ("quilt.mod.json", quilt_manifest),
        ("META-INF/mods.toml", toml_manifest),
        ("META-INF/neoforge.mods.toml", toml_manifest),
        ("mcmod.info", mcmod_manifest),
    ];
    for (name, parser) in parsers {
        if !names.contains(*name) {
            continue;
        }
        if let Some(manifest) = parser(&read_zip_member(&mut archive, name)) {
            if !sget(&manifest, "mod_id").is_empty() {
                return Some(manifest);
            }
        }
    }
    None
}

fn sidecar(path: &Path) -> Map<String, Value> {
    let Some(name) = path.file_name() else { return Map::new() };
    let sidecar_path = path.with_file_name(format!(".{}{}", name.to_string_lossy(), PROJECT_METADATA_SUFFIX));
    let Ok(metadata) = fs::symlink_metadata(&sidecar_path) else { return Map::new() };
    if metadata.is_symlink() || !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Map::new();
    }
    fs::read_to_string(&sidecar_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

pub(crate) fn dependency_graph(mods_directory: &Path) -> Result<Value, String> {
    let directory_meta = fs::symlink_metadata(mods_directory).map_err(|_| "Каталог модов недоступен".to_owned())?;
    if directory_meta.is_symlink() || !directory_meta.is_dir() {
        return Err("Каталог модов недоступен".to_owned());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(mods_directory)
        .map_err(|_| "Каталог модов недоступен".to_owned())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
    let mut nodes: Vec<Value> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut by_file: HashMap<String, usize> = HashMap::new();
    for path in entries {
        if nodes.len() >= MAX_MODS {
            break;
        }
        if !path.is_file() {
            continue;
        }
        let file_name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let disabled = file_name.ends_with(".disabled");
        let manifest = read_mod_manifest(&path);
        let metadata = sidecar(&path);
        let stem = path.file_stem().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default();
        let manifest_fields = || manifest.as_ref();
        let mod_id = {
            let from_manifest = manifest_fields().map(|value| sget(value, "mod_id")).unwrap_or_default();
            let from_metadata = py_str(metadata.get("project_id"));
            if !from_manifest.is_empty() {
                from_manifest
            } else if !from_metadata.is_empty() {
                from_metadata
            } else {
                stem.clone()
            }
        };
        let display_name = {
            let from_manifest = manifest_fields().map(|value| sget(value, "name")).unwrap_or_default();
            let from_metadata = py_str(metadata.get("display_name"));
            let raw_stem = path.file_stem().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
            if !from_manifest.is_empty() {
                from_manifest
            } else if !from_metadata.is_empty() {
                from_metadata
            } else {
                raw_stem
            }
        };
        let version = {
            let from_manifest = manifest_fields().map(|value| sget(value, "version")).unwrap_or_default();
            let from_metadata = py_str(metadata.get("version"));
            if !from_manifest.is_empty() {
                from_manifest
            } else {
                from_metadata
            }
        };
        let source = match metadata.get("source") {
            Some(Value::String(text)) => text.clone(),
            Some(other) => py_str(Some(other)),
            None => "Локальный файл".to_owned(),
        };
        let loader = manifest_fields().map(|value| sget(value, "loader")).unwrap_or_default();
        let dependencies = manifest_fields()
            .and_then(|value| value.get("dependencies"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let node = json!({
            "id": mod_id,
            "file": file_name,
            "display_name": display_name,
            "version": version,
            "enabled": !disabled,
            "source": source,
            "loader": loader,
            "dependencies": dependencies,
        });
        nodes.push(node);
        let index = nodes.len() - 1;
        by_id.entry(mod_id.to_lowercase()).or_insert(index);
        by_file.insert(file_name.to_lowercase(), index);
    }

    let mut edges: Vec<Value> = Vec::new();
    let mut missing: Vec<Value> = Vec::new();
    let mut seen_missing: HashSet<String> = HashSet::new();
    for node in &nodes {
        let Some(dependencies) = node.get("dependencies").and_then(Value::as_array) else { continue };
        for dependency in dependencies {
            let dependency_id = py_str(dependency.get("id")).trim().to_lowercase();
            if dependency_id.is_empty()
                || NOISE_IDS.contains(&dependency_id.as_str())
                || node["id"].as_str().unwrap_or_default().to_lowercase().contains(&dependency_id)
            {
                continue;
            }
            let target = by_id
                .get(&dependency_id)
                .or_else(|| by_file.get(&format!("{dependency_id}.jar")))
                .map(|index| &nodes[*index]);
            if let Some(target) = target {
                edges.push(json!({
                    "from": node["id"],
                    "to": target["id"],
                    "optional": py_truthy(dependency.get("optional")),
                    "constraint": dependency.get("constraint").cloned().unwrap_or_else(|| json!("")),
                }));
            } else if !seen_missing.contains(&dependency_id) {
                seen_missing.insert(dependency_id.clone());
                missing.push(json!({
                    "id": dependency_id,
                    "required_by": node["id"],
                    "optional": py_truthy(dependency.get("optional")),
                }));
            }
        }
    }
    Ok(json!({ "nodes": nodes, "edges": edges, "missing": missing }))
}

/// что сломается, если удалить мод: обратный обход по рёбрам
pub(crate) fn removal_impact(graph: &Value, mod_id: &str) -> Value {
    let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(edges) = graph.get("edges").and_then(Value::as_array) {
        for edge in edges {
            let to = edge["to"].as_str().unwrap_or_default().to_lowercase();
            let from = edge["from"].as_str().unwrap_or_default().to_owned();
            if to.is_empty() || from.is_empty() {
                continue;
            }
            reverse.entry(to).or_default().push(from);
        }
    }
    let mut broken: Vec<Value> = Vec::new();
    let start = mod_id.to_lowercase();
    let mut pending = vec![start.clone()];
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(start);
    while let Some(current) = pending.pop() {
        if let Some(dependents) = reverse.get(&current) {
            for dependent in dependents {
                if !seen.contains(dependent) {
                    seen.insert(dependent.clone());
                    broken.push(json!(dependent));
                    pending.push(dependent.clone());
                }
            }
        }
    }
    json!({ "mod_id": mod_id, "breaks": broken })
}

pub fn mod_dependency_graph(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(name)?;
    if info.game_edition != "java" {
        return Err("Дерево зависимостей доступно для Java-экземпляров".to_owned());
    }
    dependency_graph(&info.path.join("mods"))
}

pub fn mod_removal_impact(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(name)?;
    let graph = dependency_graph(&info.path.join("mods"))?;
    Ok(removal_impact(&graph, request.get("mod_id").and_then(Value::as_str).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-mod-deps-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    fn make_jar(path: &Path, members: &[(&str, &str)]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in members {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn fabric_and_quilt_manifests_parsed() {
        let root = temp_root("fabric");
        let jar = root.join("alpha.jar");
        make_jar(
            &jar,
            &[(
                "fabric.mod.json",
                r#"{"id":"alpha","name":"Alpha Mod","version":"1.2.3","depends":{"beta":"^2.0","minecraft":"1.20.4"},"suggests":{"gamma":"any"},"breaks":{"delta":"*"},"extra":{"x":1}}"#,
            )],
        );
        let manifest = read_mod_manifest(&jar).unwrap();
        assert_eq!(manifest["mod_id"], json!("alpha"));
        assert_eq!(manifest["loader"], json!("fabric"));
        let deps = manifest["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0], json!({ "id": "beta", "constraint": "^2.0", "optional": false }));
        assert_eq!(deps[2]["optional"], json!(true));
        assert_eq!(manifest["breaks"], json!(["delta"]));

        let quilt_jar = root.join("quilt.jar");
        make_jar(
            &quilt_jar,
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"zeta","version":"0.1","metadata":{"name":"Zeta"},"depends":["beta",{"id":"qsl","versions":["1.0","2.0"]},{"id":"opt","optional":true},{"id":"unlessed","unless":"replacement"}]}}"#,
            )],
        );
        let quilt = read_mod_manifest(&quilt_jar).unwrap();
        assert_eq!(quilt["mod_id"], json!("zeta"));
        let deps = quilt["dependencies"].as_array().unwrap();
        assert_eq!(deps[0], json!({ "id": "beta", "constraint": "любая", "optional": false }));
        assert_eq!(deps[1]["constraint"], json!("1.0,2.0"));
        assert_eq!(deps[2]["optional"], json!(true));
        assert_eq!(deps[3]["optional"], json!(true));
    }

    #[test]
    fn toml_and_mcmod_manifests_parsed() {
        let root = temp_root("toml");
        let jar = root.join("forge.jar");
        make_jar(
            &jar,
            &[(
                "META-INF/mods.toml",
                r#"modId="forgeport"
displayName="Forge Port"
version="3.1"
[[dependencies.forgeport]]
modId="forge"
mandatory=true
versionRange="[40,)"
type="required"
[[dependencies.forgeport]]
modId="optionaldep"
mandatory=false
type="optional"
"#,
            )],
        );
        let manifest = read_mod_manifest(&jar).unwrap();
        assert_eq!(manifest["mod_id"], json!("forgeport"));
        assert_eq!(manifest["loader"], json!("forge"));
        let deps = manifest["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0]["constraint"], json!("[40,)"));
        assert_eq!(deps[0]["optional"], json!(false));
        assert_eq!(deps[1]["optional"], json!(true));

        let neo_jar = root.join("neoforge.jar");
        make_jar(&neo_jar, &[("META-INF/neoforge.mods.toml", "modId=\"neothing\"\n[[dependencies.neoforge]]\nmodId=\"neoforge\"\ntype=\"required\"\n")]);
        let manifest = read_mod_manifest(&neo_jar).unwrap();
        assert_eq!(manifest["loader"], json!("neoforge"));
        assert_eq!(manifest["name"], json!("neothing"));

        let mcmod_jar = root.join("legacy.jar");
        make_jar(
            &mcmod_jar,
            &[("mcmod.info", r#"[{"modid":"legacy","name":"Legacy","version":"0.9","useDependencies":["after:library","broken-token"]}]"#)],
        );
        let mcmod = read_mod_manifest(&mcmod_jar).unwrap();
        assert_eq!(mcmod["mod_id"], json!("legacy"));
        let deps = mcmod["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0]["id"], json!("after"));
        assert_eq!(deps[0]["optional"], json!(true));
        assert_eq!(deps[1]["id"], json!("broken-token"));
        assert_eq!(deps[1]["optional"], json!(false));
    }

    #[test]
    fn graph_builds_edges_and_missing() {
        let root = temp_root("graph");
        let mods = root.join("mods");
        fs::create_dir_all(&mods).unwrap();
        make_jar(
            &mods.join("alpha-1.0.jar"),
            &[("fabric.mod.json", r#"{"id":"alpha","name":"Alpha","version":"1","depends":{"beta":"*","minecraft":"1.20.4","sodium":"*"},"suggests":{"missingone":"*"}}"#)],
        );
        make_jar(&mods.join("beta-2.0.jar"), &[("fabric.mod.json", r#"{"id":"beta","name":"Beta","version":"2","depends":{}}"#)]);
        fs::write(mods.join("gamma.disabled"), b"not a jar").unwrap();
        fs::write(
            mods.join(".gamma.disabled.trial-project.json"),
            r#"{"project_id":"gamma","display_name":"Gamma","version":"9","source":"Modrinth"}"#,
        )
        .unwrap();

        // боковой файл тоже становится узлом графа; его скрывает интерфейс
        let graph = dependency_graph(&mods).unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let ids: Vec<&str> = nodes.iter().map(|node| node["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec![".gamma.disabled.trial-project", "alpha", "beta", "gamma"]);
        let gamma = nodes.iter().find(|node| node["id"] == json!("gamma")).unwrap();
        assert_eq!(gamma["enabled"], json!(false));
        assert_eq!(gamma["source"], json!("Modrinth"));
        let edges = graph["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["to"], json!("beta"));
        let missing: Vec<&str> = graph["missing"].as_array().unwrap().iter().map(|item| item["id"].as_str().unwrap()).collect();
        assert_eq!(missing, vec!["sodium", "missingone"]);

        let impact = removal_impact(&graph, "beta");
        assert_eq!(impact["mod_id"], json!("beta"));
        assert_eq!(impact["breaks"], json!(["alpha"]));
        assert_eq!(removal_impact(&graph, "alpha")["breaks"], json!([]));
    }

    #[test]
    fn by_file_lookup_matches_jar_named_after_dependency() {
        let root = temp_root("byfile");
        let mods = root.join("mods");
        fs::create_dir_all(&mods).unwrap();
        make_jar(
            &mods.join("wrap.jar"),
            &[("fabric.mod.json", r#"{"id":"wrap","name":"Wrap","version":"1","depends":{"libxyz":"*"}}"#)],
        );
        fs::write(mods.join("libxyz.jar"), b"no manifest inside").unwrap();
        let graph = dependency_graph(&mods).unwrap();
        assert_eq!(graph["missing"], json!([]));
        assert_eq!(graph["edges"][0]["to"], json!("libxyz"));
    }

    #[test]
    fn unavailable_directory_is_an_error() {
        let root = temp_root("missing-dir");
        assert_eq!(dependency_graph(&root.join("mods")).unwrap_err(), "Каталог модов недоступен");
        let mods = root.join("mods");
        fs::create_dir_all(&mods).unwrap();
        let graph = dependency_graph(&mods).unwrap();
        assert_eq!(graph["nodes"], json!([]));
    }
}
