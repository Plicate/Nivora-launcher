// проверка обновлений через GitHub Releases
use std::path::Path;

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
    // отбрасываем prerelease и build-суффиксы
    let base = stripped.split(['-', '+']).next().unwrap_or(stripped);
    let mut parts = base.split('.');
    let major = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
    // учитываем только три числовых сегмента версии
    (major, minor, patch)
}

pub(crate) fn is_newer_version(candidate: &str, current: &str) -> bool {
    app_version_key(candidate) > app_version_key(current)
}

// встроенный ключ подписи самообновления: должен совпадать с
// plugins.updater.pubkey в tauri.conf.json (файл .pub, закодированный целиком)
const UPDATER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDUxODRFRDRBMEU0RTI0MzYKUldRMkpFNE9TdTJFVVppaCtxelpHbGpiMDVZQjdXK0xodHNTYmdwN2xoVkxGNDAxSk9DcUozRGwK";

/// как эта сборка умеет обновляться: "tauri" (встроенный updater), "pacman"
/// (пакет скачивается и ставится через pkexec) или "none" (только вручную)
fn package_channel() -> &'static str {
    if !cfg!(target_os = "linux") {
        return "tauri";
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return "tauri";
    }
    if Path::new("/usr/bin/pacman").exists() {
        return "pacman";
    }
    "none"
}

fn command_exists(command: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {command} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn verify_minisign_signature(package: &Path, signature_path: &Path) -> Result<(), String> {
    use base64::Engine;
    use minisign_verify::{PublicKey, Signature};
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(UPDATER_PUBKEY.trim())
        .map_err(|_| "Встроенный ключ подписи повреждён".to_owned())?;
    let text = String::from_utf8(decoded).map_err(|_| "Встроенный ключ подписи повреждён".to_owned())?;
    let key_line = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("RW"))
        .ok_or_else(|| "Встроенный ключ подписи повреждён".to_owned())?;
    let public_key = PublicKey::from_base64(key_line)
        .map_err(|_| "Встроенный ключ подписи повреждён".to_owned())?;
    let signature_text = std::fs::read_to_string(signature_path)
        .map_err(|_| "Не удалось прочитать файл подписи пакета обновления".to_owned())?;
    let trimmed = signature_text.trim();
    let minisign_text = if trimmed.starts_with("untrusted comment:") {
        trimmed.to_owned()
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(trimmed)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|text| text.starts_with("untrusted comment:"))
            .ok_or_else(|| "Файл подписи пакета обновления повреждён".to_owned())?
    };
    let signature = Signature::decode(&minisign_text)
        .map_err(|_| "Файл подписи пакета обновления повреждён".to_owned())?;
    let package_bytes = std::fs::read(package)
        .map_err(|_| "Не удалось прочитать скачанный пакет обновления".to_owned())?;
    public_key.verify(&package_bytes, &signature, false).map_err(|_| {
        "Подпись пакета обновления не совпала со встроенным ключом: пакет подменён или подписан чужим ключом".to_owned()
    })
}

/// установка обновления для pacman-сборки: скачиваем подписанный пакет,
/// проверяем подпись встроенным ключом и ставим через pkexec
pub fn install_remote_release(request: &Value) -> Result<Value, String> {
    if !cfg!(target_os = "linux") {
        return Err("Установка пакета обновления доступна только в Linux".to_owned());
    }
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let url = request
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Не указан адрес пакета обновления".to_owned())?;
    if !url.starts_with("https://") {
        return Err("Пакет обновления можно скачивать только по https".to_owned());
    }
    let signature_url = request
        .get("signature_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "У пакета обновления нет подписи: установка отклонена".to_owned())?;
    let file_name = url.rsplit('/').next().unwrap_or_default();
    if file_name.is_empty() || !file_name.ends_with(".pkg.tar.zst") {
        return Err("Адрес не указывает на пакет обновления лаунчера".to_owned());
    }
    let temp_dir = std::env::temp_dir().join("nivora-launcher-update");
    let destination = temp_dir.join(file_name);
    let signature_path = temp_dir.join(format!("{file_name}.sig"));
    let config = super::config::ConfigStore::load()?;
    let http = super::net::client_from_settings(config.settings(), operation_id)?;
    super::events::emit_status("Загружаю пакет обновления лаунчера…");
    let _ = std::fs::remove_file(&destination);
    http.download(url, &destination, None, None, None, None, None, None, None)?;
    super::events::emit_status("Проверяю подпись пакета обновления…");
    let _ = std::fs::remove_file(&signature_path);
    http.download(signature_url, &signature_path, None, None, None, None, None, None, None)?;
    verify_minisign_signature(&destination, &signature_path)?;
    if !command_exists("pkexec") {
        return Err("pkexec не найден: невозможно запросить пароль администратора, установите пакет вручную со страницы релизов".to_owned());
    }
    super::events::emit_status("Устанавливаю пакет обновления: подтвердите пароль администратора…");
    let exit = std::process::Command::new("pkexec")
        .args(["pacman", "-U", "--noconfirm"])
        .arg(&destination)
        .status()
        .map_err(|error| format!("Не удалось запустить установку пакета: {error}"))?;
    if !exit.success() {
        return Err("Установка пакета обновления отменена или завершилась ошибкой".to_owned());
    }
    let _ = std::fs::remove_file(&destination);
    let _ = std::fs::remove_file(&signature_path);
    Ok(json!({
        "installed": true,
        "version": request.get("version").and_then(Value::as_str).unwrap_or(""),
    }))
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
        "package_channel": package_channel(),
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
