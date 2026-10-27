use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// ограничивает число записей в каждой категории манифеста
const MAX_MANIFEST_FILES: usize = 20_000;
/// файлы настроек больше лимита не хешируются и помечаются символом "-"
const MAX_HASHED_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_VERSION_JSON_BYTES: u64 = 4 * 1024 * 1024;
/// ключи options.txt, используемые настройками и диагностикой
const INTERESTING_OPTIONS: [&str; 10] = [
    "renderDistance", "simulationDistance", "graphicsMode", "fov", "guiScale",
    "maxFps", "mipmapLevels", "fullscreen", "particleStatus", "gamma",
];

#[derive(Debug)]
pub(crate) struct InstanceManifest {
    pub(crate) root: PathBuf,
    pub(crate) mods: BTreeMap<String, Value>,
    pub(crate) resourcepacks: BTreeMap<String, Value>,
    pub(crate) shaderpacks: BTreeMap<String, Value>,
    pub(crate) configs: BTreeMap<String, Value>,
    pub(crate) meta: Map<String, Value>,
    pub(crate) options: Map<String, Value>,
}

impl InstanceManifest {
    /// агрегаты, на которые опирается автонастройка (InstanceManifest.stats)
    pub(crate) fn stats(&self) -> Map<String, Value> {
        let sum = |bucket: &BTreeMap<String, Value>, key: &str| -> i64 {
            bucket
                .values()
                .map(|entry| entry.get(key).and_then(Value::as_i64).unwrap_or(0))
                .sum()
        };
        let mut stats = Map::new();
        stats.insert("mod_count".to_owned(), json!(self.mods.len()));
        stats.insert("mod_bytes".to_owned(), json!(sum(&self.mods, "size")));
        stats.insert("class_count".to_owned(), json!(sum(&self.mods, "classes")));
        stats.insert("unpacked_bytes".to_owned(), json!(sum(&self.mods, "unpacked")));
        stats.insert("shader_count".to_owned(), json!(self.shaderpacks.len()));
        stats.insert(
            "shader_programs".to_owned(),
            json!(sum(&self.shaderpacks, "programs")),
        );
        stats.insert(
            "resourcepack_count".to_owned(),
            json!(self.resourcepacks.len()),
        );
        stats
    }
}

/// сначала использует `nivora-turbo`, затем локальный обход
pub(crate) fn build_manifest(root: &Path) -> Result<InstanceManifest, String> {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned();
    let metadata = fs::symlink_metadata(root).map_err(|_| format!("Каталог экземпляра недоступен: {name}"))?;
    if metadata.is_symlink() || !metadata.is_dir() {
        return Err(format!("Каталог экземпляра недоступен: {name}"));
    }
    if let Some(entries) = super::turbo::instance_manifest(root) {
        return Ok(manifest_from_entries(root, entries));
    }
    Ok(rust_manifest(root))
}

fn manifest_from_entries(root: &Path, entries: Vec<Value>) -> InstanceManifest {
    let mut manifest = InstanceManifest {
        root: root.to_path_buf(),
        mods: BTreeMap::new(),
        resourcepacks: BTreeMap::new(),
        shaderpacks: BTreeMap::new(),
        configs: BTreeMap::new(),
        meta: Map::new(),
        options: Map::new(),
    };
    for entry in entries {
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "meta" => {
                if let (Some(key), Some(value)) = (
                    entry.get("key").and_then(Value::as_str),
                    entry.get("value").and_then(Value::as_str),
                ) {
                    manifest.meta.insert(key.to_owned(), json!(value));
                }
            }
            "option" => {
                if let (Some(key), Some(value)) = (
                    entry.get("key").and_then(Value::as_str),
                    entry.get("value").and_then(Value::as_str),
                ) {
                    manifest.options.insert(key.to_owned(), json!(value));
                }
            }
            "mod" | "resourcepack" | "shaderpack" | "config" => {
                let path = entry.get("path").and_then(Value::as_str).unwrap_or_default();
                if path.is_empty() {
                    continue;
                }
                let bucket = match kind {
                    "mod" => &mut manifest.mods,
                    "resourcepack" => &mut manifest.resourcepacks,
                    "shaderpack" => &mut manifest.shaderpacks,
                    _ => &mut manifest.configs,
                };
                bucket.insert(path.to_owned(), entry);
            }
            _ => {}
        }
    }
    manifest
}

// запасной разбор манифеста на Rust

fn rust_manifest(root: &Path) -> InstanceManifest {
    let mut manifest = InstanceManifest {
        root: root.to_path_buf(),
        mods: BTreeMap::new(),
        resourcepacks: BTreeMap::new(),
        shaderpacks: BTreeMap::new(),
        configs: BTreeMap::new(),
        meta: Map::new(),
        options: Map::new(),
    };

    // моды: только корень mods/, jar/litemod (суффикс .disabled учитываем)
    let mods_dir = root.join("mods");
    if is_plain_directory(&mods_dir) {
        let mut jars: Vec<_> = fs::read_dir(&mods_dir)
            .map(|entries| entries.filter_map(|entry| entry.ok()).collect())
            .unwrap_or_default();
        jars.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
        for entry in jars {
            let path = entry.path();
            let lowered = entry.file_name().to_string_lossy().to_lowercase();
            let base = lowered.strip_suffix(".disabled").unwrap_or(&lowered);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_symlink() || !meta.is_file() || !(base.ends_with(".jar") || base.ends_with(".litemod")) {
                continue;
            }
            let mut record = json!({
                "kind": "mod",
                "path": format!("mods/{}", entry.file_name().to_string_lossy()),
                "size": meta.len(),
                "sha256": file_digest(&path, meta.len()),
            });
            if let Some(Value::Object(metadata)) = jar_manifest_metadata(&path) {
                for (key, value) in metadata {
                    record[key] = value;
                }
            }
            manifest
                .mods
                .insert(format!("mods/{}", entry.file_name().to_string_lossy()), record);
        }
    }

    walk_asset_tree(&root.join("resourcepacks"), "resourcepacks", &mut manifest.resourcepacks);
    walk_asset_tree(&root.join("shaderpacks"), "shaderpacks", &mut manifest.shaderpacks);

    let config_dir = root.join("config");
    if is_plain_directory(&config_dir) {
        let mut files: Vec<PathBuf> = Vec::new();
        collect_files_sorted(&config_dir, &mut files, 0);
        for path in files {
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if meta.is_symlink() || !meta.is_file() {
                continue;
            }
            let relative = path.strip_prefix(&config_dir).unwrap_or(&path);
            manifest.configs.insert(
                format!("config/{}", relative.to_string_lossy().replace('\\', "/")),
                json!({
                    "kind": "config",
                    "path": format!("config/{}", relative.to_string_lossy().replace('\\', "/")),
                    "size": meta.len(),
                    "sha256": file_digest(&path, meta.len()),
                }),
            );
        }
    }

    let options_path = root.join("options.txt");
    if let Ok(meta) = fs::symlink_metadata(&options_path) {
        if meta.is_file() && !meta.is_symlink() {
            manifest.configs.insert(
                "options.txt".to_owned(),
                json!({
                    "kind": "config",
                    "path": "options.txt",
                    "size": meta.len(),
                    "sha256": file_digest(&options_path, meta.len()),
                }),
            );
            for (key, value) in read_interesting_options(&options_path) {
                manifest.options.insert(key, json!(value));
            }
        }
    }

    let (version_id, minecraft_version) = version_meta(root);
    if !version_id.is_empty() {
        manifest.meta.insert("version_id".to_owned(), json!(version_id));
    }
    if !minecraft_version.is_empty() {
        manifest.meta.insert("minecraft_version".to_owned(), json!(minecraft_version));
    }
    manifest
}

fn is_plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.is_dir() && !meta.is_symlink())
        .unwrap_or(false)
}

fn walk_asset_tree(directory: &Path, prefix: &str, bucket: &mut BTreeMap<String, Value>) {
    if !is_plain_directory(directory) {
        return;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files_sorted(directory, &mut files, 0);
    for path in files {
        if bucket.len() > MAX_MANIFEST_FILES {
            return;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        let relative = path.strip_prefix(directory).unwrap_or(&path);
        let relative = relative.to_string_lossy().replace('\\', "/");
        let kind = if prefix == "resourcepacks" { "resourcepack" } else { "shaderpack" };
        bucket.insert(
            format!("{prefix}/{relative}"),
            json!({
                "kind": kind,
                "path": format!("{prefix}/{relative}"),
                "size": meta.len(),
                "sha256": file_digest(&path, meta.len()),
            }),
        );
    }
}

/// обходит дерево в глубину и сортирует каждый уровень без учёта регистра
fn collect_files_sorted(directory: &Path, output: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 {
        return;
    }
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map(|entries| entries.filter_map(|entry| entry.ok()).collect())
        .unwrap_or_default();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_files_sorted(&path, output, depth + 1);
        } else if meta.is_file() {
            output.push(path);
        }
    }
}

fn file_digest(path: &Path, size: u64) -> String {
    if size > MAX_HASHED_CONFIG_BYTES {
        return "-".to_owned();
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return "-".to_owned(),
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return "-".to_owned(),
        }
    }
    format!("{:x}", hasher.finalize())
}

fn read_interesting_options(path: &Path) -> Vec<(String, String)> {
    // считывает только нужные параметры и убирает пробелы по краям значений
    let file = super::options_file::OptionsFile::read(path);
    INTERESTING_OPTIONS
        .iter()
        .filter_map(|key| {
            file.get(key)
                .map(|value| (key.to_string(), value.trim().to_owned()))
        })
        .collect()
}

/// versions/*/*.json: самый свежий по mtime; (id, inheritsFrom|minecraft)
fn version_meta(root: &Path) -> (String, String) {
    let versions = root.join("versions");
    if !is_plain_directory(&versions) {
        return (String::new(), String::new());
    }
    let mut best: Option<(std::time::SystemTime, String, String)> = None;
    let mut candidates: Vec<PathBuf> = Vec::new();
    collect_version_jsons(&versions, &mut candidates, 0);
    for path in candidates {
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_VERSION_JSON_BYTES {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
        if !value.is_object() {
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
        let version_id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .unwrap_or(stem)
            .to_owned();
        let inherits = value
            .get("inheritsFrom")
            .or_else(|| value.get("minecraft"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if best.as_ref().map(|(time, _, _)| modified > *time).unwrap_or(true) {
            best = Some((modified, version_id, inherits));
        }
    }
    best.map(|(_, version_id, inherits)| (version_id, inherits)).unwrap_or_default()
}

fn collect_version_jsons(directory: &Path, output: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_dir() && !meta.is_symlink() {
            collect_version_jsons(&path, output, depth + 1);
        } else if meta.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            output.push(path);
        }
    }
}

// базовые сведения из манифестов JAR

const JAR_MANIFEST_CANDIDATES: [&str; 6] = [
    "fabric.mod.json",
    "quilt.mod.json",
    "litemod.json",
    "mcmod.info",
    "META-INF/mods.toml",
    "META-INF/neoforge.mods.toml",
];

/// возвращает поля мода, используемые сравнением экземпляров и автонастройкой
pub(crate) fn jar_manifest_metadata(path: &Path) -> Option<Value> {
    let mut loader = String::new();
    let mut mod_id = String::new();
    let mut name = String::new();
    let mut version = String::new();
    let mut classes: i64 = 0;
    let mut unpacked: i64 = 0;
    let file = fs::File::open(path).ok()?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return None,
    };
    for index in 0..archive.len() {
        let Ok(member) = archive.by_index(index) else { continue };
        let member_name = member.name().map(|v| v.to_lowercase()).unwrap_or_default();
        unpacked += member.size() as i64;
        if member_name.ends_with(".class") {
            classes += 1;
        }
    }
    for candidate in JAR_MANIFEST_CANDIDATES {
        let Ok(mut member) = archive.by_name(candidate) else { continue };
        let mut raw = Vec::new();
        if member.read_to_end(&mut raw).is_err() || raw.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&raw);
        if candidate.ends_with("mods.toml") {
            loader = if candidate.contains("neoforge") { "neoforge" } else { "forge" }.to_owned();
            mod_id = toml_field(&text, "modId");
            version = toml_field(&text, "version");
            name = toml_field(&text, "displayName");
        } else {
            loader = if candidate.contains("fabric") {
                "fabric"
            } else if candidate.contains("quilt") {
                "quilt"
            } else {
                "legacy"
            }
            .to_owned();
            mod_id = json_field(&text, "id")
                .or_else(|| json_field(&text, "modid"))
                .or_else(|| json_field(&text, "name"))
                .unwrap_or_default();
            version = json_field(&text, "version").unwrap_or_default();
            name = json_field(&text, "name")
                .or_else(|| json_field(&text, "displayName"))
                .unwrap_or_default();
        }
        break;
    }
    Some(json!({
        "loader": loader,
        "mod_id": mod_id,
        "name": name,
        "version": version,
        "classes": classes,
        "unpacked": unpacked,
    }))
}

/// извлекает строковое поле JSON, обрабатывая экранированные кавычки и обратные слеши
fn json_field(text: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let mut search = 0usize;
    while let Some(offset) = text[search..].find(&pattern) {
        let after = search + offset + pattern.len();
        let rest = text[after..].trim_start();
        if let Some(value) = rest.strip_prefix(':') {
            let value = value.trim_start();
            if let Some(value) = value.strip_prefix('"') {
                let mut result = String::new();
                let mut escaped = false;
                for char in value.chars() {
                    if escaped {
                        match char {
                            '"' => result.push('"'),
                            '\\' => result.push('\\'),
                            _ => {
                                result.push('\\');
                                result.push(char);
                            }
                        }
                        escaped = false;
                    } else if char == '\\' {
                        escaped = true;
                    } else if char == '"' {
                        return Some(result);
                    } else {
                        result.push(char);
                    }
                }
                return None;
            }
        }
        search = after;
    }
    None
}

/// извлекает простое строковое значение ключа TOML
fn toml_field(text: &str, key: &str) -> String {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(key) else { continue };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            let quote = rest.chars().next().unwrap();
            let value = &rest[1..];
            if let Some(end) = value.find(quote) {
                return value[..end].to_owned();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nivora-manifest-{tag}-{}-{}",
            std::process::id(),
            super::super::mods::random_hex(4)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// минимальный jar-мод: fabric.mod.json + один класс
    fn sample_mod(path: &Path, mod_id: &str) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("fabric.mod.json", options).unwrap();
        writer
            .write_all(
                format!(
                    "{{\"id\": \"{mod_id}\", \"name\": \"Mod {mod_id}\", \"version\": \"1.0.0\"}}"
                )
                .as_bytes(),
            )
            .unwrap();
        writer.start_file("com/example/Mod.class", options).unwrap();
        writer.write_all(b"\xca\xfe\xba\xbe").unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn rust_manifest_collects_mods_options_and_meta() {
        let root = temp_root("full");
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::create_dir_all(root.join("shaderpacks/inner")).unwrap();
        fs::create_dir_all(root.join("config/deep")).unwrap();
        fs::create_dir_all(root.join("versions/1.20.1")).unwrap();
        sample_mod(&root.join("mods/sodium.jar"), "sodium");
        sample_mod(&root.join("mods/terralith.jar.disabled"), "terralith");
        fs::write(root.join("shaderpacks/complementary.zip"), b"zip").unwrap();
        fs::write(root.join("shaderpacks/inner/shader.frag"), b"frag").unwrap();
        fs::write(root.join("config/deep.cfg"), b"cfg").unwrap();
        fs::write(root.join("options.txt"), "renderDistance:24\nmaxFps:120\nignored:x\n").unwrap();
        fs::write(
            root.join("versions/1.20.1/1.20.1.json"),
            serde_json::to_string(&json!({"id": "1.20.1-fabric", "inheritsFrom": "1.20.1"})).unwrap(),
        )
        .unwrap();

        let manifest = rust_manifest(&root);
        let stats = manifest.stats();
        assert_eq!(stats["mod_count"], json!(2));
        assert_eq!(stats["class_count"], json!(2));
        assert_eq!(stats["shader_count"], json!(2));
        assert_eq!(manifest.options.get("renderDistance"), Some(&json!("24")));
        assert_eq!(manifest.options.get("maxFps"), Some(&json!("120")));
        assert!(manifest.options.get("ignored").is_none());
        assert_eq!(manifest.meta.get("version_id"), Some(&json!("1.20.1-fabric")));
        assert_eq!(manifest.meta.get("minecraft_version"), Some(&json!("1.20.1")));
        let sodium = manifest.mods.get("mods/sodium.jar").unwrap();
        assert_eq!(sodium["mod_id"], json!("sodium"));
        assert_eq!(sodium["loader"], json!("fabric"));
        let terralith = manifest.mods.get("mods/terralith.jar.disabled").unwrap();
        assert_eq!(terralith["mod_id"], json!("terralith"));
    }

    #[test]
    fn build_manifest_reports_missing_directory() {
        let error = build_manifest(Path::new("/nonexistent-nivora-instance")).unwrap_err();
        assert!(error.starts_with("Каталог экземпляра недоступен"));
    }

    #[test]
    fn manifest_from_entries_keeps_turbo_shape() {
        let entries = vec![
            json!({"kind": "meta", "key": "version_id", "value": "1.21.4"}),
            json!({"kind": "option", "key": "renderDistance", "value": "16"}),
            json!({"kind": "mod", "path": "mods/create.jar", "size": 100, "sha256": "ab", "loader": "forge", "mod_id": "create", "name": "Create", "version": "6", "classes": 9000, "unpacked": 5_000_000}),
            json!({"kind": "shaderpack", "path": "shaderpacks/bsl.zip", "size": 5, "sha256": "cd", "programs": 42, "files": 60}),
            json!({"kind": "junk", "path": "whatever"}),
        ];
        let manifest = manifest_from_entries(Path::new("/tmp/x"), entries);
        let stats = manifest.stats();
        assert_eq!(stats["mod_count"], json!(1));
        assert_eq!(stats["class_count"], json!(9000));
        assert_eq!(stats["shader_programs"], json!(42));
        assert_eq!(manifest.meta.get("version_id"), Some(&json!("1.21.4")));
        assert_eq!(manifest.options.get("renderDistance"), Some(&json!("16")));
        assert_eq!(manifest.mods.len() + manifest.shaderpacks.len(), 2);
    }

    #[test]
    fn json_and_toml_field_extraction() {
        let json = r#"{"id": "sodium", "name": "Sodium \"fast\"", "version": "0.5"}"#;
        assert_eq!(json_field(json, "id").unwrap(), "sodium");
        assert_eq!(json_field(json, "name").unwrap(), "Sodium \"fast\"");
        assert_eq!(json_field(json, "missing"), None);
        let toml = "modLoader=\"javafml\"\nmodId=\"create\"\nversion=\"6.0\"\ndisplayName=\"Create\";\n";
        assert_eq!(toml_field(toml, "modId"), "create");
        assert_eq!(toml_field(toml, "version"), "6.0");
        assert_eq!(toml_field(toml, "displayName"), "Create");
        assert_eq!(toml_field(toml, "modLoader"), "javafml");
    }
}
