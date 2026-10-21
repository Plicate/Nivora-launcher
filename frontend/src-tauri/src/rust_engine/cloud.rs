// порт python-модулей cloud_storage.py + gui_cloud_ops.py: WebDAV-синхронизация
// данных лаунчера. Манифест подписан HMAC-SHA256 локальным ключом (файл
// cloud-manifest.key, формат байтов подписи совместим с Python-воркером);
// пароль хранится в libsecret (Linux) или в диспетчере учётных данных (Windows).
use base64::Engine;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::config::ConfigStore;
use super::net::HttpClient;
use super::turbo;

const MANIFEST_NAME: &str = ".trial-launcher-manifest.json";
const MANIFEST_KEY_FILE: &str = "cloud-manifest.key";
const MAX_REMOTE_KEY: usize = 2048;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_REMOTE_ENTRIES: usize = 10_000;
const SECRET_SERVICE: &str = "trial-launcher-webdav";

/// имена деревьев, не попадающих в облако (совпадает с Python-воркером).
const EXCLUDED_TREE_NAMES: [&str; 3] = ["logs", ".trial-cell", "plugins"];
/// серверные метаданные экземпляра не восстанавливаются в game/.
const SERVER_METADATA_NAMES: [&str; 3] = [
    ".trial-launcher-server.json",
    ".trial-launcher-live.json",
    ".trial-launcher-metrics.json",
];
/// никогда не выгружается и не восстанавливается: управляющие файлы,
/// аккаунты (токены) и ключ манифеста.
const EXCLUDED_FILE_NAMES: [&str; 8] = [
    ".trial-launcher.json",
    ".trial-launcher-instance.json",
    ".trial-launcher-server.json",
    ".trial-launcher-live.json",
    ".trial-launcher-metrics.json",
    MANIFEST_NAME,
    "accounts.json",
    MANIFEST_KEY_FILE,
];

/// настройки, восстанавливаемые из облака (перечень из cloud_storage.py).
const RESTORABLE_SETTINGS: &[&str] = &[
    "memory",
    "max_downloads",
    "startup_view",
    "last_view",
    "remember_last_view",
    "default_version_filter",
    "default_loader",
    "default_account_id",
    "open_last_instance",
    "launch_after_install",
    "auto_refresh_metadata",
    "metadata_refresh_hours",
    "download_timeout_seconds",
    "download_retries",
    "confirm_destructive_actions",
    "auto_repair_enabled",
    "auto_repair_muted",
    "show_activity_panel",
    "show_toasts",
    "minimize_on_launch",
    "close_to_tray",
    "keep_logs_days",
    "telemetry",
    "keyboard_shortcuts",
    "graphics_mode",
    "theme",
    "active_theme_id",
    "wallpaper_blur",
    "wallpaper_opacity",
    "accent_color",
    "sidebar_position",
    "sidebar_mode",
    "sidebar_width_mode",
    "sidebar_width",
    "sidebar_floating",
    "sidebar_visibility",
    "animations",
    "density",
    "radius_tokens",
    "page_themes",
    "page_layouts",
    "widget_visibility",
    "color_tokens",
    "instance_groups",
    "pinned_instance_groups",
];

fn is_excluded_tree(part: &str) -> bool {
    EXCLUDED_TREE_NAMES.contains(&part)
}

fn is_server_metadata(name: &str) -> bool {
    SERVER_METADATA_NAMES.contains(&name)
}

fn is_excluded_file(name: &str) -> bool {
    EXCLUDED_FILE_NAMES.contains(&name)
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

// ─── ключи путей ────────────────────────────────────────────────────────────

fn safe_key(value: &str) -> Result<String, String> {
    let key = value.replace('\\', "/");
    let key = key.trim_matches('/');
    if key.is_empty() || key.len() > MAX_REMOTE_KEY || key.contains('\0') {
        return Err("Недопустимый путь файла в облачном хранилище".to_owned());
    }
    if key.split('/').any(|part| part == "..") {
        return Err("Недопустимый путь файла в облачном хранилище".to_owned());
    }
    Ok(key
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/"))
}

fn safe_remote_path(value: &str) -> Result<String, String> {
    let raw = if value.is_empty() {
        "trial-launcher"
    } else {
        value
    };
    let raw = raw.trim().replace('\\', "/");
    let parts: Vec<&str> = raw
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts
        .iter()
        .any(|part| *part == ".." || part.contains('\0'))
    {
        return Err("Путь облачной папки содержит недопустимые сегменты".to_owned());
    }
    let joined = parts.join("/");
    Ok(joined.chars().take(512).collect())
}

/// https-only endpoint без учётных данных в URL; нормализация как
/// urlsplit/urlunsplit в Python: query и fragment отбрасываются, путь без
/// завершающих «/». Возвращает строку БЕЗ завершающего слэша.
fn validated_url(value: &str) -> Result<String, String> {
    const MESSAGE: &str = "WebDAV endpoint должен иметь вид https://host/remote.php/dav/files/user";
    let url = value.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(String::new());
    }
    let Some((scheme_raw, rest)) = url.split_once("://") else {
        return Err(MESSAGE.to_owned());
    };
    if !scheme_raw.eq_ignore_ascii_case("https") {
        return Err(MESSAGE.to_owned());
    }
    let netloc_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let netloc = &rest[..netloc_end];
    let path = rest[netloc_end..].split(['?', '#']).next().unwrap_or("");
    // учётные данные в netloc запрещены (user:pass@host)
    let host_part = match netloc.rfind('@') {
        Some(position) => &netloc[position + 1..],
        None => netloc,
    };
    if netloc.contains('@') {
        return Err(MESSAGE.to_owned());
    }
    let host = if let Some(stripped) = host_part.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((name, _)) => name,
            None => return Err(MESSAGE.to_owned()),
        }
    } else {
        host_part.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(MESSAGE.to_owned());
    }
    Ok(format!("https://{netloc}{path}"))
}

/// urllib.parse.quote(part, safe=""): кодируется всё, кроме unreserved.
/// Используется и WebDAV-путями (cloud.rs), и путями GitHub (github_plugins.rs).
pub(crate) fn percent_encode_path_segment(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for byte in part.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// origin как в Python: (scheme, hostname, port or 443).
fn url_origin(url: &str) -> (String, String, u16) {
    match url::Url::parse(url) {
        Ok(parsed) => (
            parsed.scheme().to_ascii_lowercase(),
            parsed.host_str().unwrap_or_default().to_ascii_lowercase(),
            parsed.port().unwrap_or(443),
        ),
        Err(_) => (String::new(), String::new(), 443),
    }
}

fn format_http_error(status: u16) -> String {
    match status {
        401 | 403 => "WebDAV отклонил авторизацию. Проверьте логин и пароль или токен.".to_owned(),
        404 => "WebDAV endpoint или удалённая папка не найдены.".to_owned(),
        405 => "WebDAV-сервис не поддерживает нужную операцию.".to_owned(),
        other => format!("WebDAV вернул HTTP {other}."),
    }
}

// ─── секреты (libsecret / диспетчер учётных данных Windows) ─────────────────

fn secret_attributes(url: &str, username: &str) -> Vec<String> {
    vec![
        SECRET_SERVICE.to_owned(),
        "user".to_owned(),
        username.to_owned(),
        "url".to_owned(),
        url.to_owned(),
    ]
}

#[cfg(windows)]
fn webdav_keyring_username(url: &str, username: &str) -> String {
    format!("{url}\n{username}")
}

fn secret_get(url: &str, username: &str, soft: bool) -> Result<String, String> {
    if url.is_empty() || username.is_empty() {
        return Ok(String::new());
    }
    #[cfg(windows)]
    {
        let identity = webdav_keyring_username(url, username);
        let password = super::accounts::windows_keyring::get(SECRET_SERVICE, &identity);
        if password.is_empty() && !soft {
            return Err("Пароль WebDAV не найден в диспетчере учётных данных Windows".to_owned());
        }
        return Ok(password);
    }
    #[cfg(not(windows))]
    {
        let tool = turbo::find_in_path("secret-tool")
            .ok_or_else(|| "Для облачного пароля установите libsecret и secret-tool".to_owned())?;
        let mut arguments = vec!["lookup".to_owned()];
        arguments.extend(secret_attributes(url, username));
        match run_secret_tool(&tool, &arguments, None) {
            // secret-tool lookup печатает пароль и завершается нулём
            Ok(output) if output.success => {
                Ok(output.stdout.trim_end_matches(['\r', '\n']).to_owned())
            }
            Ok(_) => {
                if soft {
                    Ok(String::new())
                } else {
                    Err("Пароль WebDAV не найден в libsecret".to_owned())
                }
            }
            Err(_) => {
                if soft {
                    Ok(String::new())
                } else {
                    Err("Не удалось прочитать пароль WebDAV из libsecret".to_owned())
                }
            }
        }
    }
}

fn secret_set(url: &str, username: &str, password: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let identity = webdav_keyring_username(url, username);
        super::accounts::windows_keyring::set(SECRET_SERVICE, &identity, password).map_err(|_| {
            "Не удалось сохранить пароль WebDAV в диспетчере учётных данных Windows".to_owned()
        })
    }
    #[cfg(not(windows))]
    {
        let tool = turbo::find_in_path("secret-tool")
            .ok_or_else(|| "Для облачного пароля установите libsecret и secret-tool".to_owned())?;
        let mut arguments = vec![
            "store".to_owned(),
            "--label".to_owned(),
            "Nivora Launcher WebDAV".to_owned(),
        ];
        arguments.extend(secret_attributes(url, username));
        match run_secret_tool(&tool, &arguments, Some(password)) {
            Ok(output) if output.success => Ok(()),
            Ok(_) => Err("libsecret не сохранил пароль WebDAV".to_owned()),
            Err(_) => Err("Не удалось сохранить пароль WebDAV из libsecret".to_owned()),
        }
    }
}

fn secret_clear(url: &str, username: &str) -> Result<(), String> {
    if url.is_empty() || username.is_empty() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let identity = webdav_keyring_username(url, username);
        super::accounts::windows_keyring::delete(SECRET_SERVICE, &identity).map_err(|_| {
            "Не удалось удалить пароль WebDAV из диспетчера учётных данных Windows".to_owned()
        })
    }
    #[cfg(not(windows))]
    {
        let tool = turbo::find_in_path("secret-tool")
            .ok_or_else(|| "Для облачного пароля установите libsecret и secret-tool".to_owned())?;
        let mut arguments = vec!["clear".to_owned()];
        arguments.extend(secret_attributes(url, username));
        match run_secret_tool(&tool, &arguments, None) {
            // secret-tool clear возвращает 1, когда записи нет, — это успех
            Ok(output) if output.success || output.code == 1 => Ok(()),
            Ok(_) => Err("libsecret не удалил пароль WebDAV".to_owned()),
            Err(_) => Err("Не удалось удалить пароль WebDAV из libsecret".to_owned()),
        }
    }
}

#[cfg(not(windows))]
struct SecretToolOutput {
    success: bool,
    code: i32,
    stdout: String,
}

/// secret-tool с таймаутом 5 секунд (subprocess.run(timeout=5) в Python):
/// по истечении процесс убивается. Ненулевой код возврата — не ошибка запуска.
#[cfg(not(windows))]
fn run_secret_tool(
    binary: &Path,
    args: &[String],
    stdin: Option<&str>,
) -> Result<SecretToolOutput, String> {
    use std::io::Read;
    let mut child = super::platform::command(binary)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    // пароль мал (сотни байт) — помещается в буфер пайпа, блокировки нет
    if let Some(password) = stdin {
        if let Some(pipe) = child.stdin.as_mut() {
            let _ = pipe.write_all(password.as_bytes());
            let _ = pipe.flush();
        }
    }
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                return Ok(SecretToolOutput {
                    success: status.success(),
                    code: status.code().unwrap_or(-1),
                    stdout,
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("таймаут".to_owned());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

// ─── HMAC-SHA256 (подпись манифеста) ────────────────────────────────────────

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = [0x36u8; BLOCK];
    let mut outer_key = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_key[index] ^= key_block[index];
        outer_key[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_key);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_key);
    outer.update(inner_digest);
    let mut result = [0u8; 32];
    result.copy_from_slice(&outer.finalize());
    result
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// hmac.compare_digest — сравнение за постоянное время.
fn constant_time_equal(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

// ─── WebDAV-клиент ──────────────────────────────────────────────────────────

struct CloudFile {
    key: String,
    size: i64,
    modified_at: f64,
}

impl CloudFile {
    fn as_dict(&self) -> Value {
        json!({ "key": self.key, "size": self.size, "modified_at": self.modified_at })
    }
}

struct WebDavStorage {
    http: HttpClient,
    root_url: String,
    username: String,
    password: String,
}

impl WebDavStorage {
    fn new(store: &ConfigStore) -> Result<Self, String> {
        let raw_url = store.str_setting("cloud_url");
        let url = validated_url(&raw_url)?;
        let username = store.str_setting("cloud_username").trim().to_owned();
        let remote_path = safe_remote_path(&store.str_setting("cloud_remote_path"))?;
        // пароль ищется по СЫРОМУ значению настройки — так же, как в Python:
        // атрибуты libsecret строятся из settings.cloud_url без нормализации.
        let password = secret_get(&raw_url, &username, false)?;
        if url.is_empty() || username.is_empty() || password.is_empty() {
            return Err("Сначала настройте WebDAV endpoint, логин и пароль или токен".to_owned());
        }
        let suffix = remote_path
            .split('/')
            .map(percent_encode_path_segment)
            .collect::<Vec<_>>()
            .join("/");
        let root_url = if suffix.is_empty() {
            format!("{url}/")
        } else {
            format!("{url}/{suffix}/")
        };
        // как в Python: WebDAV ходит напрямую (ProxyHandler({})), без прокси
        // лаунчера; таймаут — max(5, download_timeout_seconds), одна попытка.
        let timeout = store
            .int_setting("download_timeout_seconds")
            .clamp(5, i64::MAX)
            .max(1) as u32;
        let http = HttpClient::new(timeout, 1, "", None)?;
        Ok(Self {
            http,
            root_url,
            username,
            password,
        })
    }

    /// редиректы WebDAV: только тот же origin и только https.
    fn redirect_validator<'a>(original: &'a str) -> impl Fn(&str) -> Result<(), String> + 'a {
        let origin = url_origin(original);
        move |target: &str| {
            if url_origin(target) != origin {
                return Err("WebDAV перенаправил запрос на другой сервер".to_owned());
            }
            if !target
                .rsplit_once("://")
                .map(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
                .unwrap_or(false)
            {
                return Err("WebDAV перенаправил запрос на небезопасный адрес".to_owned());
            }
            Ok(())
        }
    }

    fn request(
        &self,
        method: &str,
        url: &str,
        data: Option<&[u8]>,
        extra_headers: &[(&str, String)],
        accepted: &[u16],
        max_bytes: u64,
    ) -> Result<Vec<u8>, String> {
        let token = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.username, self.password));
        let mut headers = vec![("Authorization".to_owned(), format!("Basic {token}"))];
        for (name, value) in extra_headers {
            headers.push(((*name).to_owned(), value.clone()));
        }
        let validator = Self::redirect_validator(url);
        let response = self
            .http
            .request_detailed(
                url,
                method,
                data,
                Some(&headers),
                Some(max_bytes),
                Some(&validator),
            )
            .map_err(|error| {
                if error.contains("слишком большой") {
                    // лимит ответа в терминах WebDAV, как в cloud_storage.py
                    "Ответ WebDAV слишком большой".to_owned()
                } else {
                    format!("Не удалось подключиться к WebDAV: {error}")
                }
            })?;
        if (200..300).contains(&response.status) {
            return Ok(response.body);
        }
        if accepted.contains(&response.status) {
            return Ok(Vec::new());
        }
        Err(format_http_error(response.status))
    }

    fn ensure_root(&self) -> Result<(), String> {
        self.make_collection(&self.root_url)
    }

    fn make_collection(&self, url: &str) -> Result<(), String> {
        let parsed =
            url::Url::parse(url).map_err(|error| format!("Некорректный сетевой адрес: {error}"))?;
        let mut base = format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or_default()
        );
        if let Some(port) = parsed.port() {
            base.push_str(&format!(":{port}"));
        }
        let mut path = String::new();
        for part in parsed.path().trim_end_matches('/').split('/') {
            if part.is_empty() {
                continue;
            }
            path.push('/');
            path.push_str(part);
            let collection = format!("{base}{path}/");
            self.request("MKCOL", &collection, None, &[], &[405, 409], MAX_FILE_BYTES)?;
        }
        Ok(())
    }

    fn file_url(&self, key: &str) -> Result<String, String> {
        let safe = safe_key(key)?;
        let encoded = safe
            .split('/')
            .map(percent_encode_path_segment)
            .collect::<Vec<_>>()
            .join("/");
        Ok(format!("{}{}", self.root_url, encoded))
    }

    fn ensure_parent_collection(&self, safe: &str) -> Result<(), String> {
        let Some(parent) = safe.rsplit_once('/').map(|(parent, _)| parent) else {
            return Ok(());
        };
        let encoded = parent
            .split('/')
            .map(percent_encode_path_segment)
            .collect::<Vec<_>>()
            .join("/");
        let root_url = self.root_url.clone();
        self.make_collection(&format!("{root_url}{encoded}/"))
    }

    fn put(&self, key: &str, content: &[u8]) -> Result<(), String> {
        let safe = safe_key(key)?;
        self.ensure_parent_collection(&safe)?;
        self.request(
            "PUT",
            &self.file_url(&safe)?,
            Some(content),
            &[("Content-Type", "application/octet-stream".to_owned())],
            &[],
            MAX_FILE_BYTES,
        )?;
        Ok(())
    }

    fn put_file(&self, key: &str, path: &Path) -> Result<(), String> {
        let safe = safe_key(key)?;
        let size = fs::metadata(path)
            .map_err(|error| format!("Не удалось прочитать файл {key}: {error}"))?
            .len();
        if size > MAX_FILE_BYTES {
            return Err(format!("Файл слишком большой для синхронизации: {safe}"));
        }
        // HttpClient шлёт тело байтами, поэтому файл читается в память (лимит
        // 512 МиБ выше). Python стримил дескриптором — различие только в пике
        // потребления памяти, контракт WebDAV тот же.
        let content =
            fs::read(path).map_err(|error| format!("Не удалось прочитать файл {key}: {error}"))?;
        self.ensure_parent_collection(&safe)?;
        self.request(
            "PUT",
            &self.file_url(&safe)?,
            Some(&content),
            &[("Content-Type", "application/octet-stream".to_owned())],
            &[],
            MAX_FILE_BYTES,
        )?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Vec<u8>, String> {
        self.request("GET", &self.file_url(key)?, None, &[], &[], MAX_FILE_BYTES)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.request(
            "DELETE",
            &self.file_url(key)?,
            None,
            &[],
            &[404],
            MAX_FILE_BYTES,
        )?;
        Ok(())
    }

    fn list_files(&self) -> Result<Vec<CloudFile>, String> {
        self.ensure_root()?;
        let base_path = {
            let parsed = url::Url::parse(&self.root_url)
                .map_err(|error| format!("Некорректный сетевой адрес: {error}"))?;
            let mut path = percent_decode_path(parsed.path());
            while path.ends_with('/') {
                path.pop();
            }
            path.push('/');
            path
        };
        let mut values: Vec<CloudFile> = Vec::new();
        let mut pending = vec![self.root_url.clone()];
        let mut visited: HashSet<String> = HashSet::new();
        let root_origin = url_origin(&self.root_url);
        while let Some(collection_url) = pending.pop() {
            if !visited.insert(collection_url.clone()) {
                continue;
            }
            if visited.len() > MAX_REMOTE_ENTRIES {
                return Err("В WebDAV слишком много каталогов".to_owned());
            }
            let body = self.request(
                "PROPFIND",
                &collection_url,
                None,
                &[
                    ("Depth", "1".to_owned()),
                    ("Content-Type", "application/xml".to_owned()),
                ],
                &[],
                MAX_MANIFEST_BYTES,
            )?;
            let upper = body.to_ascii_uppercase();
            if upper.windows(9).any(|window| window == b"<!DOCTYPE")
                || upper.windows(8).any(|window| window == b"<!ENTITY")
            {
                return Err("WebDAV вернул XML с запрещёнными декларациями".to_owned());
            }
            let entries = parse_propfind(&body)?;
            for entry in entries {
                if entry.href.is_empty() || !entry.has_prop {
                    continue;
                }
                let absolute = match url::Url::parse(&collection_url)
                    .ok()
                    .and_then(|base| base.join(&entry.href).ok())
                {
                    Some(absolute) => absolute.to_string(),
                    None => continue,
                };
                if url_origin(&absolute) != root_origin {
                    continue;
                }
                let remote_path = match url::Url::parse(&absolute) {
                    Ok(parsed) => percent_decode_path(parsed.path()),
                    Err(_) => continue,
                };
                if !remote_path.starts_with(&base_path) {
                    continue;
                }
                let key = remote_path[base_path.len()..].trim_matches('/');
                if key.is_empty() {
                    continue;
                }
                if entry.is_collection {
                    pending.push(format!("{}/", absolute.trim_end_matches('/')));
                    continue;
                }
                if values.len() >= MAX_REMOTE_ENTRIES {
                    return Err("В WebDAV слишком много файлов".to_owned());
                }
                if let Ok(safe) = safe_key(key) {
                    values.push(CloudFile {
                        key: safe,
                        size: entry.size,
                        modified_at: parse_http_date(&entry.modified),
                    });
                }
            }
        }
        values.sort_by_key(|item| item.key.to_lowercase());
        Ok(values)
    }
}

/// одна запись PROPFIND: href + первый propstat/prop.
struct DavEntry {
    href: String,
    has_prop: bool,
    is_collection: bool,
    size: i64,
    modified: String,
}

/// разбор multistatus-ответа: учитываются только элементы пространства имён
/// DAV: (как ElementTree-поиск "{DAV:}…" в Python).
fn parse_propfind(body: &[u8]) -> Result<Vec<DavEntry>, String> {
    use quick_xml::NsReader;
    let text = std::str::from_utf8(body)
        .map_err(|_| "WebDAV вернул некорректный список файлов".to_owned())?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut entries: Vec<DavEntry> = Vec::new();
    let mut current: Option<DavEntry> = None;
    let mut capture: Option<&'static str> = None;
    let (mut in_response, mut in_propstat, mut in_prop, mut in_resourcetype) =
        (false, false, false, false);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|_| "WebDAV вернул некорректный список файлов".to_owned())?;
        let is_dav = matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == "DAV:");
        match event {
            Event::Start(tag) => {
                if !is_dav {
                    buffer.clear();
                    continue;
                }
                match tag.local_name().as_ref() {
                    "response" if !in_response => {
                        in_response = true;
                        current = Some(DavEntry {
                            href: String::new(),
                            has_prop: false,
                            is_collection: false,
                            size: 0,
                            modified: String::new(),
                        });
                    }
                    "propstat" if in_response && !in_propstat => in_propstat = true,
                    "prop" if in_propstat && !in_prop => {
                        in_prop = true;
                        if let Some(entry) = current.as_mut() {
                            entry.has_prop = true;
                        }
                    }
                    "href"
                        if in_response
                            && !in_propstat
                            && current.as_ref().is_some_and(|entry| entry.href.is_empty()) =>
                    {
                        capture = Some("href");
                    }
                    "resourcetype" if in_prop => in_resourcetype = true,
                    "getcontentlength" if in_prop && !in_resourcetype => capture = Some("size"),
                    "getlastmodified" if in_prop && !in_resourcetype => capture = Some("modified"),
                    _ => {}
                }
            }
            Event::Empty(tag) => {
                if !is_dav {
                    buffer.clear();
                    continue;
                }
                // самозакрытый элемент не имеет текста и потомков
                if tag.local_name().as_ref() == "collection" && in_resourcetype {
                    if let Some(entry) = current.as_mut() {
                        entry.is_collection = true;
                    }
                }
            }
            Event::Text(text) => {
                if let Some(target) = capture {
                    // текст уже расэнскейплен ридером (trim_text + decode),
                    // дополнительно декодируем сущности XML 1.0
                    let value = text.xml10_content().into_owned();
                    if let Some(entry) = current.as_mut() {
                        match target {
                            "href" => entry.href = value,
                            "size" => entry.size = value.trim().parse().unwrap_or(0),
                            "modified" => entry.modified = value,
                            _ => {}
                        }
                    }
                }
            }
            Event::End(tag) => {
                if !is_dav {
                    buffer.clear();
                    continue;
                }
                match tag.local_name().as_ref() {
                    "response" => {
                        in_response = false;
                        if let Some(entry) = current.take() {
                            entries.push(entry);
                        }
                    }
                    "propstat" => in_propstat = false,
                    "prop" => in_prop = false,
                    "resourcetype" => in_resourcetype = false,
                    "href" | "getcontentlength" | "getlastmodified" => capture = None,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(entries)
}

/// RFC 2822/1123 дата («Sun, 01 Nov 2026 12:00:00 GMT») → unix-время.
fn parse_http_date(value: &str) -> f64 {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 5 {
        return 0.0;
    }
    let day: i64 = parts[1].parse().unwrap_or(0);
    if day == 0 {
        return 0.0;
    }
    let month = match parts[2].to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return 0.0,
    };
    let year: i64 = parts[3].parse().unwrap_or(0);
    let clock: Vec<&str> = parts[4].split(':').collect();
    if clock.len() != 3 {
        return 0.0;
    }
    let (hour, minute, second): (i64, i64, i64) = match (
        clock[0].parse::<i64>(),
        clock[1].parse::<i64>(),
        clock[2].parse::<i64>(),
    ) {
        (Ok(hour), Ok(minute), Ok(second)) => (hour, minute, second),
        _ => return 0.0,
    };
    // дни с начала эпохи (гражданский алгоритм Хиннанта, UTC)
    let years = if month <= 2 { year - 1 } else { year };
    let era = years.div_euclid(400);
    let year_of_era = years - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    (days * 86_400 + hour * 3_600 + minute * 60 + second) as f64
}

// ─── публичное состояние и пароль ───────────────────────────────────────────

fn cloud_has_password(store: &ConfigStore) -> bool {
    secret_get(
        &store.str_setting("cloud_url"),
        &store.str_setting("cloud_username"),
        true,
    )
    .map(|value| !value.is_empty())
    .unwrap_or(false)
}

pub(crate) fn save_cloud_password(store: &ConfigStore, password: &str) -> Result<(), String> {
    if password.is_empty() {
        secret_clear(
            &store.str_setting("cloud_url"),
            &store.str_setting("cloud_username"),
        )
    } else {
        secret_set(
            &store.str_setting("cloud_url"),
            &store.str_setting("cloud_username"),
            password,
        )
    }
}

fn cloud_settings_public(store: &ConfigStore) -> Value {
    json!({
        "provider": "webdav",
        "configured": !store.str_setting("cloud_url").is_empty()
            && !store.str_setting("cloud_username").is_empty()
            && cloud_has_password(store),
        "endpoint": store.str_setting("cloud_url"),
        "username": store.str_setting("cloud_username"),
        "remote_path": store.str_setting("cloud_remote_path"),
        "include_accounts": store.bool_setting("cloud_include_accounts"),
        "last_sync": store.settings().get("cloud_last_sync").and_then(Value::as_f64).unwrap_or(0.0),
    })
}

// ─── источники синхронизации ────────────────────────────────────────────────

fn is_protected_local_path(store: &ConfigStore, path: &Path) -> bool {
    let resolved = match fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(_) => return true,
    };
    let protected = [
        store.paths.settings_file.clone(),
        store.paths.accounts_file.clone(),
        store.paths.config_home.join(MANIFEST_KEY_FILE),
    ];
    for candidate in &protected {
        if let Ok(candidate) = fs::canonicalize(candidate) {
            if resolved == candidate {
                return true;
            }
        }
    }
    if let Ok(security_dir) = fs::canonicalize(&store.paths.instance_security_dir) {
        if resolved == security_dir || resolved.starts_with(&security_dir) {
            return true;
        }
    }
    false
}

/// файлы, попадающие в облачный бэкап: (ключ, путь).
fn sync_sources(store: &ConfigStore) -> Result<Vec<(String, PathBuf)>, String> {
    let mut roots: Vec<(&str, PathBuf)> = vec![("data", store.paths.data_home.clone())];
    let game_dir = store.game_dir()?;
    let game_dir = fs::canonicalize(&game_dir).unwrap_or(game_dir);
    let data_home =
        fs::canonicalize(&store.paths.data_home).unwrap_or_else(|_| store.paths.data_home.clone());
    if !game_dir.starts_with(&data_home) {
        roots.push(("game", game_dir));
    }
    roots.push(("config/settings.json", store.paths.settings_file.clone()));
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    for (prefix, root) in roots {
        let is_symlink = fs::symlink_metadata(&root)
            .map(|meta| meta.is_symlink())
            .unwrap_or(true);
        if is_symlink || !root.exists() {
            continue;
        }
        if root.is_file() {
            if root
                .file_name()
                .map(|name| name.to_string_lossy().ends_with(".part"))
                .unwrap_or(false)
            {
                continue;
            }
            sources.push((prefix.to_owned(), root));
            continue;
        }
        for path in walk_sorted(&root)? {
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if meta.is_symlink() || !meta.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.ends_with(".part") {
                continue;
            }
            let relative = match path.strip_prefix(&root) {
                Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if is_protected_local_path(store, &path) {
                continue;
            }
            if relative.split('/').any(is_excluded_tree) {
                continue;
            }
            if is_server_metadata(&name) || is_excluded_file(&name) {
                continue;
            }
            sources.push((format!("{prefix}/{relative}"), path));
        }
    }
    Ok(sources)
}

/// аналог sorted(root.rglob("*")) — глобальная сортировка путей.
fn walk_sorted(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if fs::symlink_metadata(&path)
                .map(|meta| meta.is_dir())
                .unwrap_or(false)
            {
                stack.push(path.clone());
            }
            out.push(path);
        }
    }
    out.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    Ok(out)
}

/// локальный путь для ключа; None — ключ не восстанавливается.
fn local_target(store: &ConfigStore, key: &str) -> Result<Option<PathBuf>, String> {
    let safe = safe_key(key)?;
    if safe == "config/settings.json" {
        return Ok(Some(store.paths.settings_file.clone()));
    }
    // account tokens are never restored from DAV, even when the upload opt-in is set.
    if safe == "config/accounts.json" {
        return Ok(None);
    }
    if let Some(relative) = safe.strip_prefix("data/") {
        if relative.split('/').any(is_excluded_tree) || relative.split('/').any(is_excluded_file) {
            return Ok(None);
        }
        let target = store.paths.data_home.join(relative);
        return Ok(if is_protected_local_path(store, &target) {
            None
        } else {
            Some(target)
        });
    }
    if let Some(relative) = safe.strip_prefix("game/") {
        if relative.split('/').any(is_excluded_tree)
            || relative.split('/').any(is_server_metadata)
            || relative.split('/').any(is_excluded_file)
        {
            return Ok(None);
        }
        let target = store.game_dir()?.join(relative);
        return Ok(if is_protected_local_path(store, &target) {
            None
        } else {
            Some(target)
        });
    }
    Ok(None)
}

fn atomic_file_write(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать каталог {}: {error}", parent.display()))?;
    }
    static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = path.parent().unwrap_or(Path::new(".")).join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id(),
        unique
    ));
    let write_result = (|| {
        let mut handle = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|error| format!("Не удалось записать файл {}: {error}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = handle.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        handle
            .write_all(content)
            .and_then(|_| handle.sync_all())
            .map_err(|error| format!("Не удалось записать файл {}: {error}", path.display()))
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Не удалось записать файл {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

// ─── манифест ───────────────────────────────────────────────────────────────

/// байты = json.dumps(filtered, ensure_ascii=False, sort_keys=True, indent=2):
/// ключи вставляются по алфавиту, чтобы порядок совпадал и при BTreeMap, и при
/// IndexMap (preserve_order).
fn filtered_settings_bytes(store: &ConfigStore) -> Result<Vec<u8>, String> {
    let settings = store.settings();
    let mut keys: Vec<&str> = RESTORABLE_SETTINGS
        .iter()
        .copied()
        .filter(|key| settings.get(*key).is_some())
        .collect();
    keys.sort_unstable();
    let mut filtered = Map::new();
    for key in keys {
        filtered.insert(key.to_owned(), settings[key].clone());
    }
    let content = serde_json::to_string_pretty(&Value::Object(filtered))
        .map_err(|error| format!("Не удалось сериализовать настройки: {error}"))?
        .into_bytes();
    if content.len() as u64 > MAX_SETTINGS_BYTES {
        return Err("Отфильтрованные настройки слишком большие для синхронизации".to_owned());
    }
    Ok(content)
}

fn manifest_key(store: &ConfigStore) -> Result<Vec<u8>, String> {
    let path = store.paths.config_home.join(MANIFEST_KEY_FILE);
    loop {
        if fs::symlink_metadata(&path)
            .map(|meta| meta.is_symlink())
            .unwrap_or(false)
        {
            return Err("Ключ проверки облачного манифеста является ссылкой".to_owned());
        }
        match fs::read(&path) {
            Ok(value) => {
                if value.len() == 32 {
                    return Ok(value);
                }
                return Err("Ключ проверки облачного манифеста повреждён".to_owned());
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "Не удалось сохранить ключ проверки облачного манифеста: {error}"
                ));
            }
            Err(_) => {}
        }
        let mut value = [0u8; 32];
        getrandom::fill(&mut value).map_err(|error| {
            format!("Не удалось сохранить ключ проверки облачного манифеста: {error}")
        })?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            options.custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(&path) {
            Ok(mut handle) => {
                handle.write_all(&value).map_err(|error| {
                    format!("Не удалось сохранить ключ проверки облачного манифеста: {error}")
                })?;
                return Ok(value.to_vec());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Не удалось сохранить ключ проверки облачного манифеста: {error}"
                ));
            }
        }
    }
}

/// подписываемые байты: json.dumps(..., sort_keys=True, separators=(",", ":")).
/// Ключи вставляются в алфавитном порядке (files, format; внутри файла —
/// key, modified_at, sha256, size) — байты совпадают с Python-воркером.
fn manifest_payload(files: &[Value]) -> Result<Vec<u8>, String> {
    let mut normalized: Vec<Value> = Vec::with_capacity(files.len());
    for item in files {
        let mut entry = Map::new();
        entry.insert(
            "key".to_owned(),
            json!(item.get("key").and_then(Value::as_str).unwrap_or("")),
        );
        entry.insert(
            "modified_at".to_owned(),
            json!(item
                .get("modified_at")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)),
        );
        entry.insert(
            "sha256".to_owned(),
            json!(item
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase()),
        );
        entry.insert(
            "size".to_owned(),
            json!(item.get("size").and_then(Value::as_i64).unwrap_or(0)),
        );
        normalized.push(Value::Object(entry));
    }
    let mut payload = Map::new();
    payload.insert("files".to_owned(), Value::Array(normalized));
    payload.insert("format".to_owned(), json!(2));
    serde_json::to_string(&Value::Object(payload))
        .map_err(|error| format!("Не удалось сериализовать манифест: {error}"))
        .map(|text| text.into_bytes())
}

fn manifest_signature(store: &ConfigStore, files: &[Value]) -> Result<String, String> {
    let key = manifest_key(store)?;
    Ok(hex(&hmac_sha256(&key, &manifest_payload(files)?)))
}

fn sha256_of(content: &[u8]) -> String {
    hex(&Sha256::digest(content))
}

struct LoadedManifest {
    #[allow(dead_code)]
    updated_at: f64,
    files: Vec<Value>,
}

/// int() из Python: строки с пробелами, числа; иначе — ошибка записи.
fn manifest_int(item: &Value, key: &str) -> Result<i64, String> {
    match item.get(key) {
        None | Some(Value::Null) => Err("некорректную запись".to_owned()),
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .ok_or_else(|| "некорректную запись".to_owned()),
        Some(Value::String(text)) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| "некорректную запись".to_owned()),
        Some(_) => Err("некорректную запись".to_owned()),
    }
}

fn load_manifest(
    storage: &WebDavStorage,
    store: &ConfigStore,
) -> Result<(LoadedManifest, String), String> {
    let raw_bytes = storage
        .request(
            "GET",
            &storage.file_url(MANIFEST_NAME)?,
            None,
            &[],
            &[],
            MAX_MANIFEST_BYTES,
        )
        .map_err(|_| "Облачный манифест отсутствует или повреждён".to_owned())?;
    let raw: Value = serde_json::from_slice(&raw_bytes)
        .map_err(|_| "Облачный манифест отсутствует или повреждён".to_owned())?;
    let signature = raw
        .get("signature")
        .and_then(Value::as_str)
        .ok_or_else(|| "Облачный манифест имеет неподдерживаемый формат".to_owned())?
        .to_owned();
    if raw.get("format").and_then(Value::as_i64) != Some(2)
        || !raw.get("files").map(Value::is_array).unwrap_or(false)
        || signature.is_empty()
    {
        return Err("Облачный манифест имеет неподдерживаемый формат".to_owned());
    }
    let mut files: Vec<Value> = Vec::new();
    let mut total: i64 = 0;
    let mut seen_keys: HashSet<String> = HashSet::new();
    for item in raw["files"].as_array().unwrap_or(&Vec::new()) {
        if !item.is_object() {
            return Err("Облачный манифест содержит некорректный файл".to_owned());
        }
        let key = match item.get("key").and_then(Value::as_str) {
            Some(key) => key,
            None => return Err("Облачный манифест содержит некорректную запись".to_owned()),
        };
        let key = safe_key(key)
            .map_err(|_| "Облачный манифест содержит некорректную запись".to_owned())?;
        let size = manifest_int(item, "size")
            .map_err(|message| format!("Облачный манифест содержит {message}"))?;
        let digest = item
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        if !(0..=MAX_FILE_BYTES as i64).contains(&size)
            || digest.len() != 64
            || !digest
                .chars()
                .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
        {
            return Err("Облачный манифест содержит небезопасный размер или хеш".to_owned());
        }
        if !seen_keys.insert(key.clone()) {
            return Err("Облачный манифест содержит повторяющийся путь".to_owned());
        }
        total += size;
        if total > MAX_TOTAL_BYTES as i64 {
            return Err("Облачный бэкап превышает общий лимит размера".to_owned());
        }
        files.push(json!({
            "key": key,
            "size": size,
            "modified_at": item.get("modified_at").and_then(Value::as_f64).unwrap_or(0.0),
            "sha256": digest,
        }));
    }
    let expected = manifest_signature(store, &files)?;
    if !constant_time_equal(&expected, &signature) {
        return Err("Подпись облачного манифеста не совпадает с локальным ключом".to_owned());
    }
    let payload_digest = sha256_of(&manifest_payload(&files)?);
    Ok((
        LoadedManifest {
            updated_at: raw.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0),
            files,
        },
        payload_digest,
    ))
}

fn reject_symlink_target(path: &Path) -> Result<(), String> {
    let mut current = path.to_owned();
    loop {
        if fs::symlink_metadata(&current)
            .map(|meta| meta.is_symlink())
            .unwrap_or(false)
        {
            return Err(format!(
                "Локальный путь восстановления является ссылкой: {}",
                path.display()
            ));
        }
        if !current.pop() {
            return Ok(());
        }
    }
}

fn restore_plan(store: &ConfigStore, storage: &WebDavStorage) -> Result<(Value, String), String> {
    let (manifest, manifest_id) = load_manifest(storage, store)?;
    let settings_file = store.paths.settings_file.clone();
    let mut changes: Vec<Value> = Vec::new();
    let mut settings_preview: Option<Map<String, Value>> = None;
    let mut skipped = 0;
    let mut bytes: i64 = 0;
    for item in &manifest.files {
        let key = item["key"].as_str().unwrap_or_default();
        let Some(target) = local_target(store, key)? else {
            skipped += 1;
            continue;
        };
        bytes += item["size"].as_i64().unwrap_or(0);
        if target == settings_file {
            if item["size"].as_i64().unwrap_or(0) > MAX_SETTINGS_BYTES as i64 {
                return Err("Удалённый settings.json слишком большой".to_owned());
            }
            let content = storage.get(key)?;
            let remote_settings: Value = serde_json::from_slice(&content)
                .map_err(|_| "Удалённые настройки имеют некорректный формат".to_owned())?;
            if !remote_settings.is_object() {
                return Err("Удалённые настройки имеют неожиданный формат".to_owned());
            }
            let mut preview = Map::new();
            for name in RESTORABLE_SETTINGS {
                if let Some(value) = remote_settings.get(name) {
                    preview.insert((*name).to_owned(), value.clone());
                }
            }
            let mut keys: Vec<&str> = preview.keys().map(String::as_str).collect();
            keys.sort_unstable();
            changes.push(json!({ "key": key, "action": "merge-settings", "keys": keys }));
            settings_preview = Some(preview);
            continue;
        }
        reject_symlink_target(&target)?;
        changes.push(json!({
            "key": key,
            "action": if target.exists() { "replace" } else { "create" },
            "size": item["size"],
        }));
    }
    let files_count = changes.len();
    Ok((
        json!({
            "preview": true,
            "manifest_id": manifest_id,
            "changes": changes,
            "skipped": skipped,
            "files": files_count,
            "bytes": bytes,
            "settings": settings_preview.map(Value::Object).unwrap_or_else(|| json!({})),
        }),
        manifest_id,
    ))
}

// ─── синхронизация ──────────────────────────────────────────────────────────

pub(crate) fn sync_cloud(store: &mut ConfigStore) -> Result<Value, String> {
    let storage = WebDavStorage::new(store)?;
    storage.ensure_root()?;
    let mut uploaded = 0;
    let mut total_bytes: u64 = 0;
    let mut manifest: Vec<Value> = Vec::new();
    let sources: Vec<(String, PathBuf)> = sync_sources(store)?
        .into_iter()
        .filter(|(key, _)| key != "config/settings.json")
        .collect();
    let settings_path = store.paths.settings_file.clone();
    let settings_content = if settings_path.exists() {
        filtered_settings_bytes(store)?
    } else {
        Vec::new()
    };
    if sources.len() + usize::from(!settings_content.is_empty()) > MAX_REMOTE_ENTRIES {
        return Err("В облачный бэкап попадает слишком много файлов".to_owned());
    }
    let planned_total: u64 = sources
        .iter()
        .filter(|(_, path)| path.exists())
        .map(|(_, path)| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
        .sum::<u64>()
        + settings_content.len() as u64;
    if planned_total > MAX_TOTAL_BYTES {
        return Err("Облачный бэкап превышает общий лимит размера".to_owned());
    }
    // дайджесты для манифеста считаются одной пачкой до выгрузки: nivora-turbo
    let existing: Vec<PathBuf> = sources
        .iter()
        .filter(|(_, path)| path.exists())
        .map(|(_, path)| path.clone())
        .collect();
    let batch = turbo::sha256_batch(&existing);
    for (key, path) in &sources {
        let meta = fs::metadata(path)
            .map_err(|error| format!("Не удалось прочитать файл {key}: {error}"))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(format!("Файл слишком большой для синхронизации: {key}"));
        }
        storage.put_file(key, path)?;
        let digest = match &batch {
            Some(map) => map.get(path).map(|(digest, _)| digest.clone()),
            None => None,
        };
        let digest = match digest {
            Some(digest) => digest,
            None => sha256_of(
                &fs::read(path)
                    .map_err(|error| format!("Не удалось прочитать файл {key}: {error}"))?,
            ),
        };
        manifest.push(json!({
            "key": key,
            "size": meta.len(),
            "modified_at": meta.modified().ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0),
            "sha256": digest,
        }));
        uploaded += 1;
        total_bytes += meta.len();
    }
    if !settings_content.is_empty() {
        let modified = fs::metadata(&settings_path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_secs_f64())
            .unwrap_or(0.0);
        storage.put("config/settings.json", &settings_content)?;
        manifest.push(json!({
            "key": "config/settings.json",
            "size": settings_content.len(),
            "modified_at": modified,
            "sha256": sha256_of(&settings_content),
        }));
        uploaded += 1;
        total_bytes += settings_content.len() as u64;
    }
    let signature = manifest_signature(store, &manifest)?;
    let mut data = Map::new();
    data.insert("format".to_owned(), json!(2));
    data.insert("updated_at".to_owned(), json!(unix_now()));
    data.insert("files".to_owned(), Value::Array(manifest));
    data.insert("signature".to_owned(), json!(signature));
    let manifest_content = serde_json::to_string_pretty(&Value::Object(data))
        .map_err(|error| format!("Не удалось сериализовать манифест: {error}"))?
        .into_bytes();
    if manifest_content.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("Облачный манифест слишком большой".to_owned());
    }
    storage.put(MANIFEST_NAME, &manifest_content)?;
    store.set_values(&json!({ "cloud_last_sync": unix_now() }))?;
    Ok(json!({ "uploaded": uploaded, "bytes": total_bytes }))
}

pub(crate) fn restore_cloud(
    store: &mut ConfigStore,
    confirmed: bool,
    manifest_id: &str,
) -> Result<Value, String> {
    let storage = WebDavStorage::new(store)?;
    let (plan, current_manifest_id) = restore_plan(store, &storage)?;
    if !confirmed {
        return Ok(plan);
    }
    if manifest_id.is_empty() || !constant_time_equal(manifest_id, &current_manifest_id) {
        return Err("Список облачного восстановления изменился. Получите новый preview и подтвердите его снова.".to_owned());
    }
    let mut restored = 0;
    let mut total_bytes: u64 = 0;
    let restored_at = unix_now();
    let (manifest, restored_manifest_id) = load_manifest(&storage, store)?;
    if !constant_time_equal(&restored_manifest_id, &current_manifest_id) {
        return Err("Список облачного восстановления изменился. Получите новый preview и подтвердите его снова.".to_owned());
    }
    let settings_file = store.paths.settings_file.clone();
    let mut remote_settings: Map<String, Value> = Map::new();
    for item in &manifest.files {
        let key = item["key"].as_str().unwrap_or_default();
        let Some(target) = local_target(store, key)? else {
            continue;
        };
        if target == settings_file && item["size"].as_i64().unwrap_or(0) > MAX_SETTINGS_BYTES as i64
        {
            return Err("Удалённый settings.json слишком большой".to_owned());
        }
        let content = storage.get(key)?;
        if content.len() as i64 != item["size"].as_i64().unwrap_or(-1)
            || sha256_of(&content) != item["sha256"].as_str().unwrap_or_default()
        {
            return Err(format!(
                "Проверка целостности облачного файла не пройдена: {key}"
            ));
        }
        if target == settings_file {
            let settings: Value = serde_json::from_slice(&content)
                .map_err(|_| "Удалённый settings.json имеет некорректный формат".to_owned())?;
            if !settings.is_object() {
                return Err("Удалённый settings.json имеет неожиданный формат".to_owned());
            }
            for name in RESTORABLE_SETTINGS {
                if let Some(value) = settings.get(name) {
                    remote_settings.insert((*name).to_owned(), value.clone());
                }
            }
            restored += 1;
            continue;
        }
        reject_symlink_target(&target)?;
        atomic_file_write(&target, &content)?;
        restored += 1;
        total_bytes += content.len() as u64;
    }
    if !remote_settings.is_empty() {
        store.set_values(&Value::Object(remote_settings))?;
    }
    store.set_values(&json!({ "cloud_last_sync": restored_at }))?;
    Ok(json!({
        "preview": false,
        "restored": restored,
        "bytes": total_bytes,
        "manifest_id": current_manifest_id,
        "skipped": plan.get("skipped").and_then(Value::as_i64).unwrap_or(0),
    }))
}

pub(crate) fn delete_cloud_file(store: &ConfigStore, key: &str) -> Result<(), String> {
    if safe_key(key)? == MANIFEST_NAME {
        return Err("Служебный manifest нельзя удалить".to_owned());
    }
    WebDavStorage::new(store)?.delete(key)
}

// ─── операции GUI (порт gui_cloud_ops.py) ───────────────────────────────────

pub(crate) fn cloud_status(_request: &Value) -> Result<Value, String> {
    let store = ConfigStore::load()?;
    Ok(json!({ "status": cloud_settings_public(&store) }))
}

pub(crate) fn cloud_configure(request: &Value) -> Result<Value, String> {
    let mut store = ConfigStore::load()?;
    let next_url = request
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_owned();
    let next_username = request
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    // только проверка: в настройках сохраняется сырая (обрезанная) строка,
    // как в Python — по ней строятся атрибуты libsecret.
    validated_url(&next_url)?;
    let credentials_changed = next_url != store.str_setting("cloud_url")
        || next_username != store.str_setting("cloud_username");
    // в Python фолбэк — legacy-имя «trial-launcher»; движок использует
    // переименованный дефолт настроек «nivora-launcher».
    const DEFAULT_REMOTE_PATH: &str = "nivora-launcher";
    let values = json!({
        "cloud_url": next_url,
        "cloud_username": next_username,
        "cloud_remote_path": request.get("remote_path").and_then(Value::as_str).unwrap_or(DEFAULT_REMOTE_PATH),
        "cloud_include_accounts": request.get("include_accounts").and_then(Value::as_bool).unwrap_or(false),
    });
    // remove the old key before changing its libsecret attributes. Otherwise
    // changing endpoint or username would leave an orphaned credential.
    if credentials_changed {
        save_cloud_password(&store, "")?;
    }
    store.set_values(&values)?;
    let password = request
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !password.is_empty() {
        save_cloud_password(&store, password)?;
    } else if credentials_changed {
        save_cloud_password(&store, "")?;
    }
    Ok(json!({
        "status": cloud_settings_public(&store),
        "settings": store.settings_public(),
    }))
}

pub(crate) fn cloud_test(_request: &Value) -> Result<Value, String> {
    let store = ConfigStore::load()?;
    let storage = WebDavStorage::new(&store)?;
    let files = storage.list_files()?;
    Ok(json!({
        "status": cloud_settings_public(&store),
        "files": files.iter().map(|file| file.as_dict()).collect::<Vec<_>>(),
    }))
}

pub(crate) fn cloud_list(_request: &Value) -> Result<Value, String> {
    let store = ConfigStore::load()?;
    let storage = WebDavStorage::new(&store)?;
    let files = storage.list_files()?;
    Ok(json!({
        "status": cloud_settings_public(&store),
        "files": files.iter().map(|file| file.as_dict()).collect::<Vec<_>>(),
    }))
}

pub(crate) fn cloud_sync(_request: &Value) -> Result<Value, String> {
    let mut store = ConfigStore::load()?;
    let result = sync_cloud(&mut store)?;
    let storage = WebDavStorage::new(&store)?;
    let files = storage.list_files()?;
    super::events::emit_status_event("cloud-synced", "Файлы сохранены в облако.");
    Ok(json!({
        "result": result,
        "status": cloud_settings_public(&store),
        "files": files.iter().map(|file| file.as_dict()).collect::<Vec<_>>(),
    }))
}

pub(crate) fn cloud_restore(request: &Value) -> Result<Value, String> {
    let mut store = ConfigStore::load()?;
    let result = restore_cloud(
        &mut store,
        request
            .get("confirmed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        request
            .get("manifest_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    if result
        .get("preview")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(json!({ "result": result, "status": cloud_settings_public(&store) }));
    }
    super::events::emit_status_event("cloud-restored", "Файлы восстановлены из облака.");
    Ok(json!({ "result": result, "status": cloud_settings_public(&store) }))
}

pub(crate) fn cloud_delete(request: &Value) -> Result<Value, String> {
    let store = ConfigStore::load()?;
    delete_cloud_file(
        &store,
        request.get("key").and_then(Value::as_str).unwrap_or(""),
    )?;
    let storage = WebDavStorage::new(&store)?;
    let files = storage.list_files()?;
    Ok(json!({
        "status": cloud_settings_public(&store),
        "files": files.iter().map(|file| file.as_dict()).collect::<Vec<_>>(),
    }))
}

pub(crate) fn cloud_disconnect(_request: &Value) -> Result<Value, String> {
    let mut store = ConfigStore::load()?;
    save_cloud_password(&store, "")?;
    store.set_values(&json!({
        "cloud_url": "",
        "cloud_username": "",
        "cloud_remote_path": "nivora-launcher",
        "cloud_include_accounts": false,
        "cloud_last_sync": 0,
    }))?;
    Ok(json!({
        "status": cloud_settings_public(&store),
        "settings": store.settings_public(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc4231_vectors() {
        let key = [0x0bu8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // ключ длиннее блока (case 6 RFC 4231)
        let key = [0xaau8; 131];
        let mac = hmac_sha256(
            &key,
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn key_validation_rejects_traversal_and_bad_length() {
        assert_eq!(safe_key("a/b/c.png").unwrap(), "a/b/c.png");
        assert_eq!(safe_key("a//b/./c").unwrap(), "a/b/c");
        assert_eq!(safe_key("/leading/").unwrap(), "leading");
        assert!(safe_key("").is_err());
        assert!(safe_key("../escape").is_err());
        assert!(safe_key("a/../../b").is_err());
        assert!(safe_key(&"x".repeat(MAX_REMOTE_KEY + 1)).is_err());
        assert!(safe_key("a\0b").is_err());
    }

    #[test]
    fn remote_path_normalizes_and_caps_length() {
        assert_eq!(safe_remote_path("  /a//b/.///  ").unwrap(), "a/b");
        assert_eq!(safe_remote_path("").unwrap(), "trial-launcher");
        assert!(safe_remote_path("..").is_err());
        let long = "a".repeat(600);
        assert_eq!(safe_remote_path(&long).unwrap().chars().count(), 512);
    }

    #[test]
    fn url_validation_requires_https_without_credentials() {
        assert_eq!(
            validated_url("https://dav.example/remote.php/dav/files/u/").unwrap(),
            "https://dav.example/remote.php/dav/files/u"
        );
        // без пути слэша не появляется (urlunsplit-семантика)
        assert_eq!(
            validated_url("https://dav.example").unwrap(),
            "https://dav.example"
        );
        assert_eq!(validated_url("  ").unwrap(), "");
        assert!(validated_url("http://dav.example/").is_err());
        assert!(validated_url("https://user:pass@dav.example/").is_err());
        assert!(validated_url("ftp://dav.example/").is_err());
        assert!(validated_url("not a url").is_err());
    }

    #[test]
    fn percent_roundtrip_matches_urllib_quote() {
        assert_eq!(percent_encode_path_segment("a b"), "a%20b");
        assert_eq!(percent_encode_path_segment("мир"), "%D0%BC%D0%B8%D1%80");
        assert_eq!(percent_encode_path_segment("a~b-c_d.e"), "a~b-c_d.e");
        assert_eq!(percent_decode_path("a%20%D0%BC"), "a м");
        assert_eq!(percent_decode_path("без-кодов"), "без-кодов");
    }

    #[test]
    fn http_date_parses_known_timestamps() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), 0.0);
        // 2026-01-01 = 1767225600; +304 дня (янв–окт) + 12:34:56
        assert_eq!(
            parse_http_date("Sun, 01 Nov 2026 12:34:56 GMT"),
            1_793_536_496.0
        );
        assert_eq!(parse_http_date("мусор"), 0.0);
        assert_eq!(parse_http_date(""), 0.0);
    }

    #[test]
    fn propfind_parses_collections_and_files() {
        let body = br#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/remote.php/dav/files/u/trial-launcher/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/u/trial-launcher/saves%20world/level.dat</d:href>
    <d:propstat><d:prop>
      <d:resourcetype/>
      <d:getcontentlength>1024</d:getcontentlength>
      <d:getlastmodified>Sun, 01 Nov 2026 12:00:00 GMT</d:getlastmodified>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/u/trial-launcher/no-prop.xml</d:href>
  </d:response>
  <x:response xmlns:x="urn:other">
    <x:href>/remote.php/dav/files/u/trial-launcher/alien.dat</x:href>
    <x:propstat><x:prop><x:resourcetype><x:collection/></x:resourcetype><x:getcontentlength>7</x:getcontentlength></x:prop></x:propstat>
  </x:response>
</d:multistatus>"#;
        let entries = parse_propfind(body).expect("разбор PROPFIND");
        assert_eq!(entries.len(), 3, "чужое пространство имён игнорируется");
        assert!(entries[0].is_collection);
        assert_eq!(entries[1].size, 1024);
        assert!(!entries[1].is_collection);
        assert!(entries[1].href.ends_with("saves%20world/level.dat"));
        assert!(!entries[2].has_prop, "response без propstat пропускается");
    }

    #[test]
    fn propfind_accepts_default_namespace() {
        let body = br#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/f/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>
</D:multistatus>"#;
        let entries = parse_propfind(body).expect("разбор");
        assert!(entries[0].is_collection);
        let body = br#"<multistatus xmlns="DAV:">
  <response><href>/dav/f/a.dat</href><propstat><prop><getcontentlength>3</getcontentlength></prop></propstat></response>
</multistatus>"#;
        let entries = parse_propfind(body).expect("разбор");
        assert_eq!(entries[0].size, 3);
        assert!(!entries[0].is_collection);
    }

    #[test]
    fn constant_time_compare_behaves_like_compare_digest() {
        assert!(constant_time_equal("abcdef", "abcdef"));
        assert!(!constant_time_equal("abcdef", "abcdeg"));
        assert!(!constant_time_equal("abc", "abcd"));
    }

    #[test]
    fn manifest_payload_is_compact_and_sorted_like_python() {
        let files = vec![json!({ "key": "data/x", "size": 5, "modified_at": 1.5, "sha256": "AB" })];
        let payload = String::from_utf8(manifest_payload(&files).unwrap()).unwrap();
        assert_eq!(
            payload,
            r#"{"files":[{"key":"data/x","modified_at":1.5,"sha256":"ab","size":5}],"format":2}"#
        );
    }

    #[test]
    fn manifest_int_accepts_python_int_shapes() {
        let item = json!({ "a": 5, "b": " 7 ", "c": 5.9, "d": "x", "e": null });
        assert_eq!(manifest_int(&item, "a").unwrap(), 5);
        assert_eq!(manifest_int(&item, "b").unwrap(), 7);
        assert_eq!(manifest_int(&item, "c").unwrap(), 5);
        assert!(manifest_int(&item, "d").is_err());
        assert!(manifest_int(&item, "e").is_err());
        assert!(manifest_int(&item, "missing").is_err());
    }
}
