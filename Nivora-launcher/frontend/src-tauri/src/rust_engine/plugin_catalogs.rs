// Каталоги серверных плагинов (Modrinth/Spigot/Hangar/custom): настройки,
// поиск и детали. Поисковые запросы обслуживает gui-слой.
use serde_json::{json, Value};

pub const PLUGIN_METADATA_SCHEMA: i64 = 1;
pub const PLUGIN_METADATA_MAX_BYTES: u64 = 64 * 1024;
pub const MAX_PLUGIN_DEPENDENCY_PROJECTS: usize = 64;
pub const MAX_PLUGIN_DEPENDENCY_DEPTH: usize = 16;
pub const MAX_PLUGIN_DEPENDENCIES_PER_PROJECT: usize = 100;
pub const UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE: &str =
    "[unverified-download] Каталог не предоставил доверенный хеш плагина. Подтвердите скачивание явно.";

pub fn default_plugin_catalogs() -> Value {
    json!([
        { "id": "modrinth-plugins", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2", "enabled": true },
        { "id": "spigot", "name": "Spigot", "kind": "spigot", "api_url": "https://api.spiget.org/v2", "enabled": true },
        { "id": "hangar", "name": "Hangar", "kind": "hangar", "api_url": "https://hangar.papermc.io/api/v1", "enabled": true },
    ])
}

/// Нормализация списка каталогов из настроек: уникальные id, допустимые
/// kind, только https.
pub fn normalize_plugin_catalogs(value: &Value) -> Value {
    let raw: Vec<Value> = match value.as_array() {
        Some(items) => items.clone(),
        None => default_plugin_catalogs()
            .as_array()
            .cloned()
            .unwrap_or_default(),
    };
    let mut result: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in raw.iter().take(20) {
        let Some(fields) = item.as_object() else { continue };
        let catalog_id = fields.get("id").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        let name = fields.get("name").and_then(Value::as_str).unwrap_or("").trim().chars().take(80).collect::<String>();
        let kind = fields.get("kind").and_then(Value::as_str).unwrap_or("custom").trim().to_lowercase();
        let api_url = fields.get("api_url").and_then(Value::as_str).unwrap_or("").trim().trim_end_matches('/').to_owned();
        if catalog_id.is_empty()
            || seen.contains(&catalog_id)
            || name.is_empty()
            || !["modrinth", "spigot", "hangar", "custom"].contains(&kind.as_str())
            || !api_url.starts_with("https://")
        {
            continue;
        }
        seen.push(catalog_id.clone());
        result.push(json!({
            "id": catalog_id.chars().take(64).collect::<String>(),
            "name": name,
            "kind": kind,
            "api_url": api_url.chars().take(512).collect::<String>(),
            "enabled": fields.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        }));
    }
    if result.is_empty() {
        return default_plugin_catalogs();
    }
    Value::Array(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_deduplicates_and_drops_bad_entries() {
        let value = json!([
            { "id": "Modrinth-Plugins", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2/" },
            { "id": "modrinth-plugins", "name": "dup", "kind": "modrinth", "api_url": "https://x.example" },
            { "id": "bad", "name": "Bad", "kind": "nope", "api_url": "https://x.example" },
            { "id": "insecure", "name": "Insecure", "kind": "custom", "api_url": "http://x.example" },
        ]);
        let normalized = normalize_plugin_catalogs(&value);
        assert_eq!(normalized.as_array().map(Vec::len), Some(1));
        assert_eq!(normalized[0]["id"], json!("modrinth-plugins"));
        assert_eq!(normalized[0]["api_url"], json!("https://api.modrinth.com/v2"));
    }

    #[test]
    fn non_array_input_falls_back_to_defaults() {
        let normalized = normalize_plugin_catalogs(&json!("nope"));
        assert_eq!(normalized.as_array().map(Vec::len), Some(3));
    }
}
