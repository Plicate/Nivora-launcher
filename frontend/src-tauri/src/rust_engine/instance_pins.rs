
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::platform_support::is_reparse_point;

const PINS_FILE: &str = ".trial-launcher-pins.json";
const PIN_KINDS: [&str; 5] = ["world", "mod", "resourcepack", "shader", "server"];
const MAX_PINS: usize = 200;
const MAX_PIN_BYTES: u64 = 64 * 1024;

/// обрезка значения пина: пробелы по краям и максимум 512 символов
fn normalize(value: &str) -> String {
    let trimmed = value.trim();
    trimmed.chars().take(512).collect()
}

/// разбор записи "kind:value"; None, если запись невалидна
fn parse_entry(entry: &str) -> Option<(String, String)> {
    let (kind, payload) = entry.split_once(':')?;
    let kind = kind.trim().to_lowercase();
    let payload = normalize(payload);
    if !PIN_KINDS.contains(&kind.as_str()) || payload.is_empty() || payload.contains(':') {
        return None;
    }
    Some((kind, payload))
}

/// атомарная запись приватного JSON (режим 0600 на unix)
fn write_private_json(path: &Path, value: &Value) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", super::snapshots::random_hex(4)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|error| format!("Не удалось записать закрепления: {error}"))?;
    use std::io::Write as _;
    let body = serde_json::to_string(value).unwrap_or_default();
    file.write_all(format!("{body}\n").as_bytes())
        .and_then(|_| file.flush())
        .map_err(|error| format!("Не удалось записать закрепления: {error}"))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Не удалось записать закрепления: {error}")
    })
}

/// список закреплений: невалидные и повторяющиеся записи отбрасываются
pub(crate) fn read_pins(instance_root: &Path) -> Vec<(String, String)> {
    let path: PathBuf = instance_root.join(PINS_FILE);
    let Ok(meta) = fs::symlink_metadata(&path) else { return Vec::new() };
    if is_reparse_point(&path) || !meta.is_file() || meta.len() > MAX_PIN_BYTES {
        return Vec::new();
    }
    let Ok(raw) = fs::read_to_string(&path) else { return Vec::new() };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&raw) else { return Vec::new() };
    let mut values: Vec<(String, String)> = Vec::new();
    for item in items {
        let Some(entry) = item.as_str() else { continue };
        let Some(parsed) = parse_entry(entry) else { continue };
        if values.iter().any(|(kind, value)| *kind == parsed.0 && *value == parsed.1) {
            continue;
        }
        values.push(parsed);
        if values.len() >= MAX_PINS {
            break;
        }
    }
    values
}

fn pins_value(pins: &[(String, String)]) -> Value {
    Value::Array(
        pins.iter()
            .map(|(kind, value)| json!(format!("{kind}:{value}")))
            .collect(),
    )
}

pub(crate) fn toggle_pin(instance_root: &Path, kind: &str, value: &str) -> Result<Value, String> {
    let Some((kind, value)) = parse_entry(&format!("{kind}:{value}")) else {
        return Err("Недопустимое закрепление: проверьте тип и значение".to_owned());
    };
    let current = read_pins(instance_root);
    let path = instance_root.join(PINS_FILE);
    if current.iter().any(|(item_kind, item_value)| *item_kind == kind && *item_value == value) {
        let remaining: Vec<(String, String)> = current
            .into_iter()
            .filter(|(item_kind, item_value)| !(item_kind == &kind && item_value == &value))
            .collect();
        write_private_json(&path, &pins_value(&remaining))?;
        return Ok(json!({"pinned": false, "pins": pins_public(&remaining)}));
    }
    if current.len() >= MAX_PINS {
        return Err(format!("Максимум {MAX_PINS} закреплений на экземпляр"));
    }
    let mut updated = current;
    updated.push((kind, value));
    write_private_json(&path, &pins_value(&updated))?;
    Ok(json!({"pinned": true, "pins": pins_public(&updated)}))
}

fn pins_public(pins: &[(String, String)]) -> Value {
    Value::Array(
        pins.iter()
            .map(|(kind, value)| json!({"kind": kind, "value": value}))
            .collect(),
    )
}

fn instance_root(name: &str) -> Result<PathBuf, String> {
    let config = super::config::ConfigStore::load()?;
    let store = super::instances::InstanceStore::new(&config)?;
    let info = store.get(name)?;
    Ok(info.path)
}

pub(crate) fn instance_pins_get(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let root = instance_root(name)?;
    Ok(json!({"pins": pins_public(&read_pins(&root))}))
}

pub(crate) fn instance_pin_toggle(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let kind = request.get("kind").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase();
    let value = request.get("value").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let root = instance_root(name)?;
    toggle_pin(&root, &kind, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("nivora-pins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("создать каталог");
        directory
    }

    #[test]
    fn toggle_roundtrip_and_limits() {
        let root = temp_root("toggle");
        assert!(read_pins(&root).is_empty());

        let added = toggle_pin(&root, "world", "Survival").unwrap();
        assert_eq!(added["pinned"], json!(true));
        assert_eq!(added["pins"].as_array().unwrap().len(), 1);
        assert_eq!(read_pins(&root), vec![("world".to_owned(), "Survival".to_owned())]);

        // повторный toggle снимает пин
        let removed = toggle_pin(&root, "world", "Survival").unwrap();
        assert_eq!(removed["pinned"], json!(false));
        assert_eq!(removed["pins"].as_array().unwrap().len(), 0);
        assert!(read_pins(&root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_entries_rejected_and_deduped() {
        let root = temp_root("invalid");
        fs::write(
            root.join(PINS_FILE),
            serde_json::to_string(&json!(["mod:sodium", "mod:sodium", "bogus:x", "mod:", "mod:a:b"])).unwrap(),
        )
        .unwrap();
        let pins = read_pins(&root);
        assert_eq!(pins, [("mod".to_owned(), "sodium".to_owned())]);

        assert!(toggle_pin(&root, "hack", "x").is_err());
        // значение с двоеточием не проходит
        assert!(toggle_pin(&root, "mod", "a:b").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn max_pins_enforced() {
        let root = temp_root("max");
        for index in 0..MAX_PINS {
            toggle_pin(&root, "mod", &format!("mod{index}")).unwrap();
        }
        let error = toggle_pin(&root, "mod", "overflow").unwrap_err();
        assert_eq!(error, format!("Максимум {MAX_PINS} закреплений на экземпляр"));
        assert_eq!(read_pins(&root).len(), MAX_PINS);
        let _ = fs::remove_dir_all(&root);
    }
}
