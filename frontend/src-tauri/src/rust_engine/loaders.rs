// версии и установщики модлоадеров
use regex::Regex;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};

use super::constants::{FABRIC_META_URL, QUILT_META_URL};
use super::net::HttpClient;

pub(crate) struct LoaderRepository {
    http: Arc<HttpClient>,
}

impl LoaderRepository {
    pub(crate) fn new(http: Arc<HttpClient>) -> Self {
        Self { http }
    }

    pub(crate) fn profile(
        &self,
        loader: &str,
        minecraft_version: &str,
        loader_version: Option<&str>,
    ) -> Result<Value, String> {
        match loader {
            "fabric" => self.fabric_profile(minecraft_version, loader_version),
            "quilt" => self.quilt_profile(minecraft_version, loader_version),
            other => Err(format!("JSON-профиль для loader пока не описан: {other}")),
        }
    }

    pub(crate) fn choices(&self, loader: &str, minecraft_version: &str) -> Result<Vec<Value>, String> {
        match loader {
            "fabric" => {
                let url = format!("{FABRIC_META_URL}/v2/versions/loader/{}", quote_component(minecraft_version));
                Ok(meta_loader_choices(&self.http.get_json(&url, None)?, "fabric"))
            }
            "quilt" => {
                let url = format!("{QUILT_META_URL}/v3/versions/loader/{}", quote_component(minecraft_version));
                Ok(meta_loader_choices(&self.http.get_json(&url, None)?, "quilt"))
            }
            "forge" => self.forge_choices(minecraft_version),
            "neoforge" => self.neoforge_choices(minecraft_version),
            other => Err(format!("Неизвестный loader: {other}")),
        }
    }

    pub(crate) fn installer_url(
        &self,
        loader: &str,
        minecraft_version: &str,
        loader_version: &str,
    ) -> Result<String, String> {
        match loader {
            "forge" => {
                let prefix = format!("{minecraft_version}-");
                let coordinate = if loader_version.starts_with(&prefix) {
                    loader_version.to_owned()
                } else {
                    format!("{minecraft_version}-{loader_version}")
                };
                Ok(format!(
                    "https://maven.minecraftforge.net/net/minecraftforge/forge/{coordinate}/forge-{coordinate}-installer.jar"
                ))
            }
            "neoforge" => Ok(format!(
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/{loader_version}/neoforge-{loader_version}-installer.jar"
            )),
            other => Err(format!("У loader {other} нет installer URL")),
        }
    }

    fn fabric_profile(&self, minecraft_version: &str, loader_version: Option<&str>) -> Result<Value, String> {
        let loader_version = self.latest_loader_version("fabric", "Fabric", minecraft_version, loader_version)?;
        let url = format!(
            "{FABRIC_META_URL}/v2/versions/loader/{}/profile/json",
            joined_quoted(&[minecraft_version, &loader_version])
        );
        self.validate_profile(self.http.get_json(&url, None)?, "fabric")
    }

    fn quilt_profile(&self, minecraft_version: &str, loader_version: Option<&str>) -> Result<Value, String> {
        let loader_version = self.latest_loader_version("quilt", "Quilt", minecraft_version, loader_version)?;
        let url = format!(
            "{QUILT_META_URL}/v3/versions/loader/{}/profile/json",
            joined_quoted(&[minecraft_version, &loader_version])
        );
        self.validate_profile(self.http.get_json(&url, None)?, "quilt")
    }

    /// явная версия лоадера, иначе самая свежая из choices
    fn latest_loader_version(
        &self,
        loader: &str,
        label: &str,
        minecraft_version: &str,
        loader_version: Option<&str>,
    ) -> Result<String, String> {
        if let Some(version) = loader_version.filter(|value| !value.is_empty()) {
            return Ok(version.to_owned());
        }
        let choices = self.choices(loader, minecraft_version)?;
        let Some(first) = choices.first() else {
            return Err(format!("{label} не поддерживает Minecraft {minecraft_version}"));
        };
        Ok(first.get("version").and_then(Value::as_str).unwrap_or_default().to_owned())
    }

    fn validate_profile(&self, profile: Value, loader: &str) -> Result<Value, String> {
        let has = |key: &str| profile.get(key).and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false);
        if !has("id") || !has("inheritsFrom") {
            return Err(format!("{loader} вернул некорректный launcher profile"));
        }
        Ok(profile)
    }

    fn forge_choices(&self, minecraft_version: &str) -> Result<Vec<Value>, String> {
        let url = "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json";
        let value = self.http.get_json(url, None)?;
        let mut choices = Vec::new();
        if let Some(raw) = value.get(minecraft_version).and_then(Value::as_array) {
            for item in raw {
                match item {
                    Value::String(text) => choices.push(choice_dict("forge", text)),
                    Value::Number(number) => choices.push(choice_dict("forge", &number.to_string())),
                    _ => {}
                }
            }
        }
        Ok(choices)
    }

    fn neoforge_choices(&self, minecraft_version: &str) -> Result<Vec<Value>, String> {
        let url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
        let raw = self.http.request(url, "GET", None, None, None, None)?;
        let text = String::from_utf8_lossy(&raw);
        neoforge_choices_from(&text, minecraft_version)
    }
}

fn choice_dict(loader: &str, version: &str) -> Value {
    json!({ "loader": loader, "version": version, "installer_url": "" })
}

fn meta_loader_choices(value: &Value, loader: &str) -> Vec<Value> {
    let mut choices = Vec::new();
    let Some(items) = value.as_array() else {
        return choices;
    };
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let loader_data = match object.get("loader") {
            Some(inner) if inner.is_object() => inner,
            _ => item,
        };
        if let Some(version) = loader_data.get("version").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            choices.push(choice_dict(loader, version));
        }
    }
    choices
}

fn neoforge_choices_from(raw: &str, minecraft_version: &str) -> Result<Vec<Value>, String> {
    let upper = raw.to_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return Err("NeoForge metadata содержит запрещённые XML-декларации".to_owned());
    }
    // файл содержит плоский список `<version>`; текст узла не очищаем
    static VERSION_TAGS: OnceLock<Regex> = OnceLock::new();
    let regex = VERSION_TAGS.get_or_init(|| Regex::new(r"(?s)<version(?:\s[^>]*)?>(.*?)</version>").unwrap());
    let values: Vec<String> = regex
        .captures_iter(raw)
        .map(|captures| captures[1].to_string())
        .filter(|text| !text.is_empty())
        .collect();
    let parts: Vec<&str> = minecraft_version.split('.').collect();
    if parts.len() < 2 {
        return Ok(Vec::new());
    }
    // схема NeoForge кодирует всю минорную линию MC: 1.20.1 → 20.1.x,
    // 1.21 → 21.0.x
    let minor_line = if parts.len() > 2 { parts[2] } else { "0" };
    let prefix = format!("{}.{}.", parts[1], minor_line);
    Ok(values
        .into_iter()
        .filter(|value| value.starts_with(&prefix))
        .map(|value| choice_dict("neoforge", &value))
        .collect())
}

fn joined_quoted(parts: &[&str]) -> String {
    parts.iter().map(|part| quote_component(part)).collect::<Vec<_>>().join("/")
}

fn quote_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let character = *byte as char;
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-' | '~') {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub fn loader_choices(request: &Value) -> Result<Value, String> {
    let config = super::config::ConfigStore::load()?;
    let loader = request.get("loader").and_then(Value::as_str).unwrap_or("fabric");
    let minecraft_version = request.get("minecraft_version").and_then(Value::as_str).unwrap_or("");
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let http = super::net::client_from_settings(config.settings(), operation_id)?;
    let repository = LoaderRepository::new(http);
    let choices = repository.choices(loader, minecraft_version)?;
    Ok(json!({ "choices": choices }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_component_encodes_like_urllib() {
        assert_eq!(quote_component("1.20.4"), "1.20.4");
        assert_eq!(quote_component("1.20.4+build.1"), "1.20.4%2Bbuild.1");
        assert_eq!(quote_component("a b/c"), "a%20b%2Fc");
        assert_eq!(quote_component("~._-"), "~._-");
    }

    #[test]
    fn meta_choice_shape_matches_dataclass() {
        let payload = json!([
            { "loader": { "version": "0.92.2+build.1" }, "intermediary": {} },
            { "loader": { "version": "" } },
            { "loader": { "stable": true } },
            "не словарь"
        ]);
        let choices = meta_loader_choices(&payload, "fabric");
        assert_eq!(
            choices,
            vec![json!({ "loader": "fabric", "version": "0.92.2+build.1", "installer_url": "" })]
        );
    }

    #[test]
    fn neoforge_versions_follow_mc_minor_line() {
        let xml = "<metadata><versioning><versions>\
                   <version>20.4.237</version><version>21.0.143</version>\
                   <version>21.1.77</version><version>21.4.99</version>\
                   </versions></versioning></metadata>";
        let for_1211 = neoforge_choices_from(xml, "1.21.1").unwrap();
        assert_eq!(for_1211.len(), 1);
        assert_eq!(for_1211[0]["version"], json!("21.1.77"));
        assert_eq!(neoforge_choices_from(xml, "1.21").unwrap()[0]["version"], json!("21.0.143"));
        assert!(neoforge_choices_from(xml, "1.19").unwrap().is_empty());
        // версия менее чем с двумя компонентами не даёт результатов
        assert!(neoforge_choices_from(xml, "custom").unwrap().is_empty());
        assert!(neoforge_choices_from("<!DOCTYPE metadata><metadata/>", "1.21.1").is_err());
    }

    #[test]
    fn installer_urls_for_forge_and_neoforge() {
        let repository = LoaderRepository::new(Arc::new(HttpClient::new(1, 1, "", None).unwrap()));
        assert_eq!(
            repository.installer_url("forge", "1.20.4", "49.0.30").unwrap(),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.4-49.0.30/forge-1.20.4-49.0.30-installer.jar"
        );
        // координата уже с версией MC не дублирует её
        assert_eq!(
            repository.installer_url("forge", "1.20.4", "1.20.4-49.0.30").unwrap(),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.4-49.0.30/forge-1.20.4-49.0.30-installer.jar"
        );
        assert_eq!(
            repository.installer_url("neoforge", "1.21.1", "21.1.77").unwrap(),
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.77/neoforge-21.1.77-installer.jar"
        );
        assert!(repository.installer_url("fabric", "1.20.4", "0.92.2").is_err());
    }
}
