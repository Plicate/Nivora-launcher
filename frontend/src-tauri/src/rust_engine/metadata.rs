// версии Minecraft и метаданные библиотек Mojang
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use super::config::ConfigStore;
use super::constants::{
    is_supported_version, MINECRAFT_ASSET_BASE_URL, MINECRAFT_LIBRARY_BASE_URL, VERSION_MANIFEST_URL,
};
use super::net::HttpClient;

pub(crate) struct VersionRepository {
    http: Arc<HttpClient>,
    manifest_url: String,
    manifest: Option<Value>,
    versions: HashMap<String, Value>,
}

impl VersionRepository {
    pub(crate) fn new(config: &ConfigStore, http: Option<Arc<HttpClient>>) -> Result<Self, String> {
        let http = match http {
            Some(http) => http,
            // используем таймаут 30 секунд и не более 3 попыток
            None => Arc::new(HttpClient::new(30, 3, "", None)?),
        };
        let custom = config.str_setting("data_api_url");
        let manifest_url = if custom.is_empty() { VERSION_MANIFEST_URL.to_owned() } else { custom };
        Ok(Self { http, manifest_url, manifest: None, versions: HashMap::new() })
    }

    fn manifest(&mut self, refresh: bool) -> Result<&Value, String> {
        if refresh || self.manifest.is_none() {
            let value = self.http.get_json(&self.manifest_url, None)?;
            if !value.is_object() || !value.get("versions").map(Value::is_array).unwrap_or(false) {
                return Err("Mojang version manifest имеет неожиданный формат".to_owned());
            }
            self.manifest = Some(value);
        }
        self.manifest
            .as_ref()
            .ok_or_else(|| "Mojang version manifest имеет неожиданный формат".to_owned())
    }

    pub(crate) fn list_versions(&mut self, include_snapshots: bool) -> Result<Vec<Value>, String> {
        let mut entries: Vec<Value> = {
            let manifest = self.manifest(false)?;
            manifest
                .get("versions")
                .and_then(Value::as_array)
                .map(|versions| {
                    versions
                        .iter()
                        .filter(|entry| {
                            entry.is_object()
                                && is_supported_version(entry.get("id").and_then(Value::as_str).unwrap_or(""))
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        if !include_snapshots {
            entries.retain(|entry| entry.get("type").and_then(Value::as_str) == Some("release"));
        }
        Ok(entries)
    }

    pub(crate) fn find_version(&mut self, version_id: &str) -> Result<Value, String> {
        if !is_supported_version(version_id) {
            return Err(format!("Версия вне поддерживаемого диапазона 1.0–26.3: {version_id}"));
        }
        if let Some(entry) = self.versions.get(version_id) {
            return Ok(entry.clone());
        }
        let found = {
            let manifest = self.manifest(false)?;
            manifest
                .get("versions")
                .and_then(Value::as_array)
                .and_then(|versions| {
                    versions
                        .iter()
                        .find(|entry| entry.is_object() && entry.get("id").and_then(Value::as_str) == Some(version_id))
                        .cloned()
                })
        };
        match found {
            Some(entry) => {
                self.versions.insert(version_id.to_owned(), entry.clone());
                Ok(entry)
            }
            None => Err(format!("Версия Minecraft не найдена в официальном manifest: {version_id}")),
        }
    }

    pub(crate) fn fetch_version_json(&mut self, version_id: &str) -> Result<Value, String> {
        let entry = self.find_version(version_id)?;
        let Some(url) = entry.get("url").and_then(Value::as_str).filter(|url| !url.is_empty()) else {
            return Err(format!("У версии {version_id} отсутствует URL JSON"));
        };
        let value = self.http.get_json(url, None)?;
        if !value.is_object() {
            return Err(format!("JSON версии {version_id} имеет неожиданный формат"));
        }
        Ok(value)
    }

    pub(crate) fn resolve(&mut self, version_id: &str) -> Result<Value, String> {
        let mut seen: HashSet<String> = HashSet::new();
        self.resolve_inner(version_id, &mut seen)
    }

    fn resolve_inner(&mut self, version_id: &str, seen: &mut HashSet<String>) -> Result<Value, String> {
        if seen.contains(version_id) {
            return Err(format!("Циклическое наследование version JSON: {version_id}"));
        }
        seen.insert(version_id.to_owned());
        let child = self.fetch_version_json(version_id)?;
        let Some(parent_id) = child
            .get("inheritsFrom")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Ok(child);
        };
        let parent = self.resolve_inner(parent_id, seen)?;
        Ok(merge_version_json(&parent, &child))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn asset_url(sha1: &str) -> String {
    // для строки короче двух символов используем её целиком
    let prefix = sha1.get(..2).unwrap_or(sha1);
    format!("{MINECRAFT_ASSET_BASE_URL}/{prefix}/{sha1}")
}

pub(crate) fn library_url(library: &Value, classifier: Option<&str>) -> Result<String, String> {
    let downloads = library
        .get("downloads")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(classifier) = classifier {
        let item = downloads
            .get("classifiers")
            .and_then(|classifiers| classifiers.get(classifier))
            .filter(|value| value.is_object())
            .cloned();
        if let Some(item) = item {
            if let Some(url) = item.get("url").and_then(Value::as_str).filter(|url| !url.is_empty()) {
                return Ok(url.to_owned());
            }
            if let Some(path) = item.get("path").and_then(Value::as_str).filter(|path| !path.is_empty()) {
                return Ok(format!("{}/{path}", library_base(library)));
            }
        }
        let name = library.get("name").and_then(Value::as_str).unwrap_or("");
        return Ok(format!("{}/{}", library_base(library), maven_path(name, Some(classifier))?));
    }
    if let Some(url) = downloads
        .get("artifact")
        .filter(|value| value.is_object())
        .and_then(|artifact| artifact.get("url"))
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
    {
        return Ok(url.to_owned());
    }
    let name = library.get("name").and_then(Value::as_str).unwrap_or("");
    let path = maven_path(name, classifier)?;
    Ok(format!("{}/{path}", library_base(library)))
}

fn library_base(library: &Value) -> String {
    library
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .unwrap_or(MINECRAFT_LIBRARY_BASE_URL)
        .trim_end_matches('/')
        .to_owned()
}

/// слияние формата наследования Mojang; оба входа остаются нетронутыми
pub(crate) fn merge_version_json(parent: &Value, child: &Value) -> Value {
    let mut result = match parent {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    let Some(child_map) = child.as_object() else {
        return Value::Object(result);
    };
    for (key, value) in child_map {
        match key.as_str() {
            "libraries" => {
                let parent_libraries = parent.get("libraries").cloned().unwrap_or_else(|| json!([]));
                result.insert(key.clone(), merge_named_list(&parent_libraries, value));
            }
            "arguments" if value.is_object() => {
                let parent_arguments = parent.get("arguments").cloned().unwrap_or_else(|| json!({}));
                result.insert(key.clone(), merge_arguments(&parent_arguments, value));
            }
            "downloads" | "logging" if value.is_object() => {
                let parent_part = parent.get(key).cloned().unwrap_or_else(|| json!({}));
                result.insert(key.clone(), deep_merge(&parent_part, value));
            }
            _ => {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(result)
}

fn merge_named_list(parent: &Value, child: &Value) -> Value {
    let mut merged: Vec<Value> = match parent {
        Value::Array(items) => items.clone(),
        _ => Vec::new(),
    };
    let Some(child_items) = child.as_array() else {
        return Value::Array(merged);
    };
    // элементы без name группируются под общим ключом None
    let mut indexes: HashMap<Option<String>, usize> = HashMap::new();
    for (index, item) in merged.iter().enumerate() {
        if item.is_object() {
            indexes.insert(name_key(item), index);
        }
    }
    for item in child_items {
        if item.is_object() {
            let key = name_key(item);
            if let Some(index) = indexes.get(&key) {
                merged[*index] = item.clone();
                continue;
            }
            indexes.insert(key, merged.len());
        }
        merged.push(item.clone());
    }
    Value::Array(merged)
}

fn name_key(item: &Value) -> Option<String> {
    match item.get("name") {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn merge_arguments(parent: &Value, child: &Value) -> Value {
    let mut result: Map<String, Value> = match parent {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    let Some(child_map) = child.as_object() else {
        return Value::Object(result);
    };
    for (key, value) in child_map {
        let merged = match (value.as_array(), result.get(key).and_then(Value::as_array)) {
            (Some(new_items), Some(existing)) => {
                let mut combined = existing.clone();
                combined.extend(new_items.iter().cloned());
                Value::Array(combined)
            }
            _ => value.clone(),
        };
        result.insert(key.clone(), merged);
    }
    Value::Object(result)
}

fn deep_merge(parent: &Value, child: &Value) -> Value {
    let (Some(parent_map), Some(child_map)) = (parent.as_object(), child.as_object()) else {
        return child.clone();
    };
    let mut result = parent_map.clone();
    for (key, value) in child_map {
        let merged = match result.get(key) {
            Some(existing) => deep_merge(existing, value),
            None => value.clone(),
        };
        result.insert(key.clone(), merged);
    }
    Value::Object(result)
}

fn maven_part_regex() -> &'static regex::Regex {
    static PART: OnceLock<regex::Regex> = OnceLock::new();
    PART.get_or_init(|| regex::Regex::new(r"^[A-Za-z0-9._-]+\z").unwrap())
}

fn maven_version_regex() -> &'static regex::Regex {
    static VERSION: OnceLock<regex::Regex> = OnceLock::new();
    VERSION.get_or_init(|| regex::Regex::new(r"^[A-Za-z0-9._+-]+\z").unwrap())
}

pub(crate) fn maven_path(name: &str, classifier: Option<&str>) -> Result<String, String> {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return Err(format!("Некорректная Maven library coordinate: {name}"));
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let part_ok = |part: &str| !part.is_empty() && maven_part_regex().is_match(part) && !part.contains("..");
    if !part_ok(group)
        || !part_ok(artifact)
        || version.is_empty()
        || !maven_version_regex().is_match(version)
        || version.contains("..")
    {
        return Err(format!("Небезопасная Maven library coordinate: {name}"));
    }
    // если classifier не задан, берём его из четвёртой части координаты
    let classifier = match classifier {
        Some(value) => Some(value.to_owned()),
        None if parts.len() >= 4 => Some(parts[3].to_owned()),
        None => None,
    };
    if let Some(classifier) = classifier.as_deref() {
        if classifier.is_empty() || !maven_part_regex().is_match(classifier) {
            return Err(format!("Небезопасный classifier Maven library: {classifier}"));
        }
    }
    let suffix = classifier.as_deref().map(|value| format!("-{value}")).unwrap_or_default();
    Ok(format!(
        "{}/{artifact}/{version}/{artifact}-{version}{suffix}.jar",
        group.replace('.', "/")
    ))
}

pub fn versions(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let include_snapshots = match request.get("include_snapshots") {
        Some(Value::Bool(value)) => *value,
        _ => true,
    };
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let http = super::net::client_from_settings(config.settings(), operation_id)?;
    let mut repository = VersionRepository::new(&config, Some(http))?;
    let values = repository.list_versions(include_snapshots)?;
    let versions: Vec<Value> = values
        .iter()
        .map(|value| {
            json!({
                "id": value.get("id").and_then(Value::as_str).unwrap_or(""),
                "type": value.get("type").and_then(Value::as_str).unwrap_or("release"),
                "release_time": value.get("releaseTime").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();
    Ok(json!({ "versions": versions }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_paths_and_validation() {
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.1", None).unwrap(),
            "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar"
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.1", Some("natives-linux")).unwrap(),
            "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-linux.jar"
        );
        // classifier из четвёртой части координаты
        assert_eq!(
            maven_path("net.java.dev.jna:jna:5.12.1:natives", None).unwrap(),
            "net/java/dev/jna/jna/5.12.1/jna-5.12.1-natives.jar"
        );
        assert_eq!(
            maven_path("a:b", None).unwrap_err(),
            "Некорректная Maven library coordinate: a:b"
        );
        assert!(maven_path("a:..b:1", None).is_err());
        assert!(maven_path("a:b:1", Some("bad/slash")).is_err());
        assert!(maven_path("a:b:..", None).is_err());
    }

    #[test]
    fn merge_version_json_combines_inheritance() {
        let parent = json!({
            "id": "1.20.4",
            "arguments": { "game": ["--username", "${auth_name}"], "jvm": ["-Xmx2G"] },
            "libraries": [
                { "name": "com.mojang:brigadier:1.0.18" },
                { "name": "org.lwjgl:lwjgl:3.3.1" }
            ],
            "downloads": { "client": { "url": "https://a/client", "sha1": "old" } }
        });
        let child = json!({
            "id": "1.20.4-fabric",
            "inheritsFrom": "1.20.4",
            "arguments": { "game": ["--fabric"] },
            "libraries": [
                { "name": "org.lwjgl:lwjgl:3.3.1", "url": "https://mirror" },
                { "name": "net.fabricmc:fabric-loader:0.92.2" }
            ],
            "downloads": { "client": { "sha1": "new" } },
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient"
        });
        let merged = merge_version_json(&parent, &child);
        assert_eq!(merged["id"], json!("1.20.4-fabric"));
        assert_eq!(merged["mainClass"], json!("net.fabricmc.loader.impl.launch.knot.KnotClient"));
        // списки аргументов конкатенируются: родительские впереди
        assert_eq!(merged["arguments"]["game"], json!(["--username", "${auth_name}", "--fabric"]));
        assert_eq!(merged["arguments"]["jvm"], json!(["-Xmx2G"]));
        // библиотека с тем же name заменяется, новая добавляется в конец
        let libraries = merged["libraries"].as_array().unwrap();
        assert_eq!(libraries.len(), 3);
        assert_eq!(libraries[1]["url"], json!("https://mirror"));
        assert_eq!(libraries[2]["name"], json!("net.fabricmc:fabric-loader:0.92.2"));
        // downloads сливается глубоко: url родителя остаётся, sha1 перезаписан
        assert_eq!(merged["downloads"]["client"]["url"], json!("https://a/client"));
        assert_eq!(merged["downloads"]["client"]["sha1"], json!("new"));
        // входы не мутируются
        assert_eq!(parent["libraries"].as_array().unwrap().len(), 2);
        assert_eq!(parent["downloads"]["client"]["sha1"], json!("old"));
    }

    #[test]
    fn asset_and_library_urls() {
        assert_eq!(
            asset_url("0123456789abcdef"),
            format!("{MINECRAFT_ASSET_BASE_URL}/01/0123456789abcdef")
        );
        let plain = json!({ "name": "com.mojang:brigadier:1.0.18" });
        assert_eq!(
            library_url(&plain, None).unwrap(),
            "https://libraries.minecraft.net/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"
        );
        // хвостовой "/" у кастомного репозитория срезается
        let mirrored = json!({ "name": "org.lwjgl:lwjgl:3.3.1", "url": "https://repo.example/mc/" });
        assert_eq!(
            library_url(&mirrored, Some("natives-linux")).unwrap(),
            "https://repo.example/mc/org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-linux.jar"
        );
        let direct = json!({
            "name": "org.lwjgl:lwjgl:3.3.1",
            "downloads": { "classifiers": { "natives-linux": { "url": "https://cdn/lwjgl.jar" } } }
        });
        assert_eq!(library_url(&direct, Some("natives-linux")).unwrap(), "https://cdn/lwjgl.jar");
        let artifact = json!({
            "name": "com.mojang:brigadier:1.0.18",
            "downloads": { "artifact": { "url": "https://libraries.minecraft.net/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar" } }
        });
        assert_eq!(
            library_url(&artifact, None).unwrap(),
            "https://libraries.minecraft.net/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"
        );
    }

    #[test]
    fn instance_names_reuse_state_implementation() {
        use crate::rust_engine::state::safe_instance_name;
        assert_eq!(safe_instance_name("Alpha Test!"), "Alpha-Test");
        assert_eq!(safe_instance_name("Мой мир"), "default");
    }
}
