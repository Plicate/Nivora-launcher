// фолбэк-проверка обновлений через GitHub Releases API — порт
use serde_json::{json, Value};

use super::constants::{APP_VERSION, GITHUB_RELEASES_API_URL, GITHUB_RELEASES_PAGE_URL};

const MAX_RELEASE_ASSETS: usize = 12;
const MAX_RELEASE_NOTES: usize = 4000;

fn app_version_key(version: &str) -> (u64, u64, u64) {
    let trimmed = version.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    // отделяем prerelease/build-суффикс: 1.0.3-1 -> 1.0.3
    let base = stripped.split(['-', '+']).next().unwrap_or(stripped);
    let mut parts = base.split('.');
    let major = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
    // четвёртый сегмент и нечисловые части игнорируем, как regex в Python
    // (\d+)(\.(\d+))?(\.(\d+))? — максимум три группы.
    (major, minor, patch)
}

pub(crate) fn is_newer_version(candidate: &str, current: &str) -> bool {
    app_version_key(candidate) > app_version_key(current)
}

pub fn check_remote_release(request: &Value) -> Result<Value, String> {
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let config = super::config::ConfigStore::load()?;
    let http = super::net::client_from_settings(config.settings(), operation_id)?;
    let payload = http
        .get_json(
            GITHUB_RELEASES_API_URL,
            Some(&[("Accept".to_owned(), "application/vnd.github+json".to_owned())]),
        )
        .map_err(|error| format!("GitHub не ответил: {error}"))?;
    let tag = payload
        .get("tag_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GitHub вернул некорректный ответ о последнем релизе".to_owned())?;
    let version = tag.trim_start_matches(['v', 'V']).to_owned();
    let release_url = payload
        .get("html_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(GITHUB_RELEASES_PAGE_URL);
    let mut assets: Vec<Value> = Vec::new();
    if let Some(items) = payload.get("assets").and_then(Value::as_array) {
        for asset in items {
            let name = asset.get("name").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let url = asset
                .get("browser_download_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if !name.is_empty() && !url.is_empty() {
                assets.push(json!({ "name": name, "url": url }));
            }
        }
    }
    assets.truncate(MAX_RELEASE_ASSETS);
    let notes: String = payload
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(MAX_RELEASE_NOTES)
        .collect();
    Ok(json!({
        "update_available": is_newer_version(&version, APP_VERSION),
        "latest_version": version,
        "current_version": APP_VERSION,
        "release_url": release_url,
        "published_at": payload.get("published_at").and_then(Value::as_str).unwrap_or(""),
        "notes": notes,
        "assets": assets,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_keys_match_python_semantics() {
        assert_eq!(app_version_key("1.1"), (1, 1, 0));
        assert_eq!(app_version_key("v1.1.0"), (1, 1, 0));
        assert_eq!(app_version_key("1.0.3-1"), (1, 0, 3));
        assert_eq!(app_version_key("мусор"), (0, 0, 0));
        assert!(is_newer_version("1.2", "1.1.9"));
        assert!(!is_newer_version("1.1.0", "1.1.0"));
        assert!(!is_newer_version("0.9", "1.0"));
    }
}
