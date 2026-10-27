// запросы на перезагрузку в режиме разработчика; никогда не обходят ни
// целостность пакета, ни доверие к исполнению
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::config::ConfigStore;
use super::plugins::PluginStore;

const IDENTITY: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]{0,79}\z";

fn identity_valid(value: &str) -> bool {
    static IDENTITY_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(IDENTITY).expect("валидный regex идентичности"));
    IDENTITY_RE.is_match(value)
}

fn reload_path(config: &ConfigStore) -> PathBuf {
    config.paths.data_home.join("plugin-dev-reload.json")
}

fn load_requests(config: &ConfigStore) -> Result<Map<String, Value>, String> {
    let path = reload_path(config);
    if std::fs::symlink_metadata(&path).map(|meta| meta.is_symlink()).unwrap_or(false) {
        return Err("Unsafe developer reload file".to_owned());
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Map::new()),
    };
    if !metadata.is_file() || metadata.len() > 32 * 1024 {
        return Err("Unsafe developer reload file".to_owned());
    }
    let raw = std::fs::read(&path).map_err(|_| "Unsafe developer reload file".to_owned())?;
    // при повреждённом JSON возвращаем понятную ошибку реестра
    let value: Value = serde_json::from_slice(&raw).map_err(|_| "Invalid reload registry".to_owned())?;
    let Some(fields) = value.as_object() else {
        return Err("Invalid reload registry".to_owned());
    };
    if fields.len() > 128 {
        return Err("Invalid reload registry".to_owned());
    }
    for (key, value) in fields {
        // принимаем только целые числа; логические и дробные значения отклоняем
        let Value::Number(number) = value else {
            return Err("Invalid reload identity or timestamp".to_owned());
        };
        let Some(timestamp) = number.as_i64() else {
            return Err("Invalid reload identity or timestamp".to_owned());
        };
        if !identity_valid(key) || timestamp <= 0 {
            return Err("Invalid reload identity or timestamp".to_owned());
        }
    }
    Ok(fields.clone())
}

/// точка входа должна быть безопасным относительным путём внутри пакета
fn entrypoint_parts(entrypoint: &str) -> Result<Vec<String>, String> {
    if entrypoint.is_empty()
        || entrypoint.len() > 256
        || entrypoint.contains('\\')
        || entrypoint.contains(':')
        || Path::new(entrypoint).is_absolute()
    {
        return Err("Unsafe plugin entrypoint".to_owned());
    }
    let parts: Vec<String> = Path::new(entrypoint)
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_owned))
        .collect();
    if parts.iter().any(|part| part == "..") {
        return Err("Unsafe plugin entrypoint".to_owned());
    }
    Ok(parts)
}

fn mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.mtime() as i64 * 1_000_000_000 + metadata.mtime_nsec() as i64
    }
    #[cfg(not(unix))]
    {
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as i64)
            .unwrap_or(0)
    }
}

pub(crate) fn changes(config: &ConfigStore) -> Result<Value, String> {
    if !config.bool_setting("developer_mode") {
        return Ok(json!({"enabled": false, "generation": "", "requests": {}}));
    }
    let store = PluginStore::new(config)?;
    let mut entries: Vec<(String, String, i64, i64)> = Vec::new();
    // скрипты входа поставляются отдельными файлами, без обхода каталогов
    let mut seen = 0usize;
    let directory = std::fs::read_dir(&store.root).map_err(|error| error.to_string())?;
    for entry in directory {
        seen += 1;
        if seen > 128 {
            break;
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !identity_valid(&name) || !entry.path().is_dir() {
            continue;
        }
        let root = entry.path();
        if std::fs::symlink_metadata(&root).map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Cannot watch a linked plugin".to_owned());
        }
        let manifest_path = root.join("plugin.json");
        let manifest_metadata = std::fs::symlink_metadata(&manifest_path);
        let unsafe_manifest = manifest_metadata.as_ref().is_ok_and(|meta| meta.is_symlink())
            || std::fs::metadata(&manifest_path).map(|meta| !meta.is_file() || meta.len() > 256 * 1024).unwrap_or(true);
        if unsafe_manifest {
            return Err("Cannot watch an unsafe manifest".to_owned());
        }
        let manifest: Value = std::fs::read(&manifest_path)
            .map_err(|_| "Cannot watch an unsafe manifest".to_owned())
            .and_then(|raw| serde_json::from_slice(&raw).map_err(|_| "Cannot watch an invalid manifest".to_owned()))?;
        let entrypoint = match manifest.as_object().and_then(|fields| fields.get("entrypoint")) {
            None => "main.js".to_owned(),
            Some(value) => value.as_str().ok_or_else(|| "Unsafe plugin entrypoint".to_owned())?.to_owned(),
        };
        let parts = entrypoint_parts(&entrypoint)?;
        let mut candidate = root.clone();
        for part in &parts {
            candidate = candidate.join(part);
            if std::fs::symlink_metadata(&candidate).map(|meta| meta.is_symlink()).unwrap_or(false) {
                return Err("Cannot watch a linked entrypoint".to_owned());
            }
        }
        for name in ["plugin.json".to_owned(), entrypoint.clone()] {
            let path = root.join(&name);
            if std::fs::symlink_metadata(&path).map(|meta| meta.is_symlink()).unwrap_or(false) {
                return Err("Cannot watch a linked entrypoint".to_owned());
            }
            let Ok(metadata) = std::fs::metadata(&path) else { continue };
            entries.push((entry.file_name().to_string_lossy().into_owned(), name, mtime_ns(&metadata), metadata.len() as i64));
        }
    }
    let requests = load_requests(config)?;
    let entry_values: Vec<Value> = entries
        .into_iter()
        .map(|(id, name, modified, size)| json!([id, name, modified, size]))
        .collect();
    let fingerprint_payload = json!([entry_values, Value::Object(requests.clone())]);
    let digest = Sha256::digest(python_dumps_sorted(&fingerprint_payload).as_bytes());
    let generation: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(json!({"enabled": true, "generation": generation, "requests": requests}))
}

/// стабильный JSON с отсортированными ключами и стандартными пробелами
fn python_dumps_sorted(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_dumps_sorted).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap_or_default(),
                        python_dumps_sorted(&fields[key])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub fn plugin_dev_changes(_request: &Value) -> Result<Value, String> {
    let config = super::config::ConfigStore::load()?;
    changes(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_dumps_format_matches_separators() {
        assert_eq!(python_dumps_sorted(&json!({})), "{}");
        assert_eq!(python_dumps_sorted(&json!([])), "[]");
        assert_eq!(python_dumps_sorted(&json!({"b": 1, "a": [1, 2]})), r#"{"a": [1, 2], "b": 1}"#);
        assert_eq!(python_dumps_sorted(&json!({"запросы": {"b": 2, "a": 1}})), r#"{"запросы": {"a": 1, "b": 2}}"#);
    }

    #[test]
    fn entrypoint_validation_rejects_escapes() {
        assert_eq!(entrypoint_parts("main.js").unwrap(), vec!["main.js".to_owned()]);
        assert_eq!(entrypoint_parts("src/main.js").unwrap(), vec!["src".to_owned(), "main.js".to_owned()]);
        assert!(entrypoint_parts("").is_err());
        assert!(entrypoint_parts("a\\b.js").is_err());
        assert!(entrypoint_parts("a:b.js").is_err());
        assert!(entrypoint_parts("/abs/main.js").is_err());
        assert!(entrypoint_parts("../main.js").is_err());
        assert!(entrypoint_parts("a/../b.js").is_err());
        assert!(entrypoint_parts(&"x".repeat(257)).is_err());
    }

    #[test]
    fn reload_registry_validates_strictly() {
        let root = std::env::temp_dir().join(format!("nivora-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        let config = ConfigStore::load().unwrap();
        let registry = reload_path(&config);
        std::fs::create_dir_all(registry.parent().unwrap()).unwrap();

        assert!(load_requests(&config).unwrap().is_empty(), "нет файла — пустой реестр");

        std::fs::write(&registry, br#"{"demo": 123}"#).unwrap();
        let requests = load_requests(&config).unwrap();
        assert_eq!(requests["demo"], json!(123));

        std::fs::write(&registry, b"not-json").unwrap();
        assert_eq!(load_requests(&config).unwrap_err(), "Invalid reload registry");
        std::fs::write(&registry, b"[1, 2]").unwrap();
        assert_eq!(load_requests(&config).unwrap_err(), "Invalid reload registry");

        for bad in [
            br#"{"with space": 1}"#.as_ref(),
            br#"{"demo": true}"#.as_ref(),
            br#"{"demo": 1.5}"#.as_ref(),
            br#"{"demo": "123"}"#.as_ref(),
            br#"{"demo": 0}"#.as_ref(),
            br#"{"demo": -5}"#.as_ref(),
        ] {
            std::fs::write(&registry, bad).unwrap();
            assert_eq!(
                load_requests(&config).unwrap_err(),
                "Invalid reload identity or timestamp",
                "реестр {:?} должен быть отвергнут",
                String::from_utf8_lossy(bad)
            );
        }
        // значение 2^63 не помещается в `i64`
        std::fs::write(&registry, br#"{"demo": 9223372036854775808}"#).unwrap();
        assert_eq!(load_requests(&config).unwrap_err(), "Invalid reload identity or timestamp");

        #[cfg(unix)]
        {
            let linked = root.join("linked.json");
            std::fs::write(&linked, b"{}").unwrap();
            let _ = std::fs::remove_file(&registry);
            std::os::unix::fs::symlink(&linked, &registry).unwrap();
            assert_eq!(load_requests(&config).unwrap_err(), "Unsafe developer reload file");
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}
