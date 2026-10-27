use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::cloud::percent_encode_path_segment;
use super::config::ConfigStore;
use super::net::{client_from_settings, form_urlencoded, HttpClient};
use super::plugin_security::verify_manifest;
use super::plugins::{
    parse_plugin_yml, parse_v2_manifest_bytes, validate_manifest, validate_plugin_json_manifest,
    verify_unsigned_v2_integrity, PluginStore, PLUGIN_MANIFEST, PLUGIN_MAX_ARCHIVE_BYTES,
    PLUGIN_MAX_FILES, PLUGIN_MAX_MANIFEST_BYTES, PLUGIN_MAX_UNPACKED_BYTES, PLUGIN_V2_MANIFEST,
};
use super::turbo;

const GITHUB_PLUGIN_TOPIC: &str = "trial-launcher-plugin";
const GITHUB_API_URL: &str = "https://api.github.com";
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_CLIENT_ID_ENV: &str = "TRIAL_LAUNCHER_GITHUB_CLIENT_ID";
const GITHUB_OAUTH_SCOPE: &str = "public_repo";
const GITHUB_API_VERSION: &str = "2022-11-28";
const MAX_LICENSE_BYTES: u64 = 1024 * 1024;
const MAX_GITHUB_JSON_BYTES: u64 = 4 * 1024 * 1024;
// отдельная тема поиска и ограниченный размер ZIP для каталога тем
const THEME_GITHUB_TOPIC: &str = "trial-launcher-theme";
const THEME_MAX_ARCHIVE_BYTES: usize = 8 * 1024 * 1024;

const API_HOSTS: [&str; 1] = ["api.github.com"];
const OAUTH_HOSTS: [&str; 1] = ["github.com"];
const UPLOAD_HOSTS: [&str; 1] = ["uploads.github.com"];
const ASSET_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// матрица допустимых переходов редиректа при скачивании ассета
fn asset_redirect_targets(host: &str) -> &'static [&'static str] {
    match host {
        "api.github.com" => &[
            "api.github.com",
            "github.com",
            "objects.githubusercontent.com",
            "release-assets.githubusercontent.com",
        ],
        "github.com" => &[
            "github.com",
            "objects.githubusercontent.com",
            "release-assets.githubusercontent.com",
        ],
        "objects.githubusercontent.com" => &["objects.githubusercontent.com"],
        "release-assets.githubusercontent.com" => &["release-assets.githubusercontent.com"],
        _ => &[],
    }
}

fn owner_name_valid(owner: &str) -> bool {
    let bytes = owner.as_bytes();
    match bytes.first() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    let rest = &bytes[1..];
    rest.len() <= 38
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn repo_name_valid(repo: &str) -> bool {
    if repo == "." || repo == ".." {
        return false;
    }
    let bytes = repo.as_bytes();
    match bytes.first() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    let rest = &bytes[1..];
    rest.len() <= 99
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn search_query_valid(query: &str) -> bool {
    query.chars().count() <= 120
        && query.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | ' ' | '-')
        })
}

fn release_version_valid(version: &str) -> bool {
    let bytes = version.as_bytes();
    match bytes.first() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    let rest = &bytes[1..];
    rest.len() <= 63
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn valid_bearer_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| (33..=126).contains(&(character as u32)))
}

// чтение токена GitHub из системного хранилища

const GITHUB_SECRET_SERVICE: &str = "trial-launcher-github";

#[derive(Default)]
struct SecretToolTokenProvider;

impl SecretToolTokenProvider {
    fn available() -> bool {
        #[cfg(windows)]
        {
            // keyring-бэкенд Windows всегда активен, если Credential Manager
            // доступен; его приоритет проверяется при первой же операции
            return true;
        }
        #[cfg(not(windows))]
        {
            turbo::find_in_path("secret-tool").is_some()
        }
    }

    fn get_token(&self) -> Result<Option<String>, String> {
        #[cfg(windows)]
        {
            let token =
                super::accounts::windows_keyring::get(GITHUB_SECRET_SERVICE, "plugin-publishing");
            return Ok(if token.is_empty() { None } else { Some(token) });
        }
        #[cfg(not(windows))]
        {
            let output = run_secret_tool(
                &[
                    "lookup",
                    "service",
                    GITHUB_SECRET_SERVICE,
                    "purpose",
                    "plugin-publishing",
                ],
                None,
                true,
            )?;
            let token = output.trim().to_owned();
            Ok(if token.is_empty() { None } else { Some(token) })
        }
    }

    fn set_token(&self, token: &str) -> Result<(), String> {
        if !valid_bearer_token(token) {
            return Err("GitHub returned an invalid access token.".to_owned());
        }
        self.clear_token()?;
        #[cfg(windows)]
        {
            super::accounts::windows_keyring::set(GITHUB_SECRET_SERVICE, "plugin-publishing", token)
                .map_err(|_| {
                    "GitHub token storage is unavailable in Windows Credential Manager.".to_owned()
                })
        }
        #[cfg(not(windows))]
        {
            run_secret_tool(
                &[
                    "store",
                    "--label",
                    "Nivora Launcher GitHub",
                    "service",
                    GITHUB_SECRET_SERVICE,
                    "purpose",
                    "plugin-publishing",
                ],
                Some(token),
                false,
            )?;
            Ok(())
        }
    }

    fn clear_token(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            let existing =
                super::accounts::windows_keyring::get(GITHUB_SECRET_SERVICE, "plugin-publishing");
            if !existing.is_empty() {
                super::accounts::windows_keyring::delete(
                    GITHUB_SECRET_SERVICE,
                    "plugin-publishing",
                )
                .map_err(|_| {
                    "GitHub token storage is unavailable in Windows Credential Manager.".to_owned()
                })?;
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            run_secret_tool(
                &[
                    "clear",
                    "service",
                    GITHUB_SECRET_SERVICE,
                    "purpose",
                    "plugin-publishing",
                ],
                None,
                true,
            )?;
            Ok(())
        }
    }
}

/// secret-tool с таймаутом 10 секунд; allow_missing терпит код 1 (записи нет)
#[cfg(not(windows))]
fn run_secret_tool(
    arguments: &[&str],
    stdin: Option<&str>,
    allow_missing: bool,
) -> Result<String, String> {
    let binary = turbo::find_in_path("secret-tool").ok_or_else(|| {
        "GitHub sign-in needs Secret Service and the secret-tool command.".to_owned()
    })?;
    let mut child = super::platform::command(&binary)
        .args(arguments)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "GitHub token storage is unavailable in the system keyring.".to_owned())?;
    if let Some(text) = stdin {
        if let Some(pipe) = child.stdin.as_mut() {
            let _ = pipe.write_all(text.as_bytes());
            let _ = pipe.flush();
        }
    }
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                if status.success() || (allow_missing && status.code() == Some(1)) {
                    return Ok(stdout);
                }
                return Err("GitHub token storage could not access the system keyring.".to_owned());
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(
                        "GitHub token storage is unavailable in the system keyring.".to_owned()
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                return Err("GitHub token storage is unavailable in the system keyring.".to_owned())
            }
        }
    }
}


fn validate_https_url(url: &str, allowed_hosts: &[&str]) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|_| "A GitHub URL is malformed.".to_owned())?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_owned();
    let port_ok = parsed.port().is_none();
    let fragment_ok = parsed
        .fragment()
        .map(|value| value.is_empty())
        .unwrap_or(true);
    if parsed.scheme() != "https"
        || host.is_empty()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !fragment_ok
        || !port_ok
        || !allowed_hosts.contains(&host.as_str())
    {
        return Err("Only HTTPS GitHub service URLs are allowed.".to_owned());
    }
    Ok(host)
}

fn required_text(value: Option<&Value>, field: &str, maximum: usize) -> Result<String, String> {
    let text = value.and_then(Value::as_str).unwrap_or("");
    if text.is_empty() || text.len() > maximum {
        return Err(format!("GitHub returned an invalid {field}."));
    }
    Ok(text.to_owned())
}

/// преобразует число, строку или логическое значение в целое число
fn json_int(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        Some(Value::Bool(flag)) => Some(i64::from(*flag)),
        _ => None,
    }
}

fn bounded_int(
    value: Option<&Value>,
    minimum: i64,
    maximum: i64,
    field: &str,
) -> Result<i64, String> {
    let parsed =
        json_int(value).ok_or_else(|| format!("GitHub returned an invalid {field} value."))?;
    Ok(parsed.clamp(minimum, maximum))
}

fn positive_int(value: Option<&Value>, field: &str) -> Result<i64, String> {
    let parsed = json_int(value).ok_or_else(|| format!("GitHub returned an invalid {field}."))?;
    if parsed <= 0 {
        return Err(format!("GitHub returned an invalid {field}."));
    }
    Ok(parsed)
}

fn optional_int(value: Option<&Value>) -> Option<i64> {
    json_int(value).map(|parsed| parsed.max(0))
}

fn parse_json_bytes(raw: &[u8], source: &str) -> Result<Value, String> {
    serde_json::from_slice(raw).map_err(|_| format!("{source} returned invalid JSON."))
}

fn repository_identity(repository: &Value) -> Result<(String, String), String> {
    let owner_data = repository
        .get("owner")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or(json!({}));
    let owner = owner_data
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let repo = repository
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    validate_repository_name(&owner, &repo)?;
    Ok((owner, repo))
}

fn validate_repository_name(owner: &str, repo: &str) -> Result<(), String> {
    if !owner_name_valid(owner) {
        return Err("GitHub repository owner is invalid.".to_owned());
    }
    if !repo_name_valid(repo) {
        return Err("GitHub repository name is invalid.".to_owned());
    }
    Ok(())
}

fn repository_path(owner: &str, repo: &str) -> Result<String, String> {
    validate_repository_name(owner, repo)?;
    Ok(format!(
        "{}/{}",
        percent_encode_path_segment(owner),
        percent_encode_path_segment(repo)
    ))
}

fn repository_summary(
    repository: &Value,
    topics: Option<&Value>,
) -> Result<Map<String, Value>, String> {
    let (owner, repo) = repository_identity(repository)?;
    let default_topics = repository.get("topics").cloned().unwrap_or(json!([]));
    let topic_values = topics.unwrap_or(&default_topics);
    let mut normalized_topics: Vec<String> = topic_values
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|topic| topic.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    if !normalized_topics
        .iter()
        .any(|topic| topic == GITHUB_PLUGIN_TOPIC)
    {
        normalized_topics.push(GITHUB_PLUGIN_TOPIC.to_owned());
    }
    let mut summary = Map::new();
    summary.insert("owner".into(), json!(owner));
    summary.insert("repo".into(), json!(repo));
    summary.insert("full_name".into(), json!(format!("{owner}/{repo}")));
    summary.insert(
        "name".into(),
        json!(repository
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&repo)
            .chars()
            .take(100)
            .collect::<String>()),
    );
    summary.insert(
        "description".into(),
        json!(repository
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(500)
            .collect::<String>()),
    );
    summary.insert(
        "repository_url".into(),
        json!(format!(
            "https://github.com/{}/{}",
            percent_encode_path_segment(&owner),
            percent_encode_path_segment(&repo)
        )),
    );
    summary.insert(
        "stars".into(),
        json!(optional_int(repository.get("stargazers_count")).unwrap_or(0)),
    );
    summary.insert(
        "updated_at".into(),
        json!(repository
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or("")),
    );
    summary.insert("topics".into(), json!(normalized_topics));
    Ok(summary)
}

fn asset_download_url(owner: &str, repo: &str, tag: &str, name: &str) -> Result<String, String> {
    let path = [owner, repo, "releases", "download", tag, name]
        .iter()
        .map(|part| percent_encode_path_segment(part))
        .collect::<Vec<_>>()
        .join("/");
    let _ = repository_path(owner, repo)?;
    Ok(format!("https://github.com/{path}"))
}

fn release_url(owner: &str, repo: &str, tag: &str) -> Result<String, String> {
    let _ = repository_path(owner, repo)?;
    Ok(format!(
        "https://github.com/{}/{}/releases/tag/{}",
        percent_encode_path_segment(owner),
        percent_encode_path_segment(repo),
        percent_encode_path_segment(tag)
    ))
}

fn release_upload_url(
    value: Option<&Value>,
    owner: &str,
    repo: &str,
    release_id: i64,
) -> Result<String, String> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| "GitHub returned an invalid release upload URL.".to_owned())?;
    const TEMPLATE: &str = "{?name,label}";
    let clean_url = raw.strip_suffix(TEMPLATE).unwrap_or(raw);
    let host = validate_https_url(clean_url, &UPLOAD_HOSTS)
        .map_err(|_| "GitHub returned an unsafe release upload URL.".to_owned())?;
    let parsed = url::Url::parse(clean_url)
        .map_err(|_| "GitHub returned an unsafe release upload URL.".to_owned())?;
    let expected_path = format!(
        "/repos/{}/releases/{release_id}/assets",
        repository_path(owner, repo)?
    );
    let query_empty = parsed.query().map(|value| value.is_empty()).unwrap_or(true);
    let fragment_empty = parsed
        .fragment()
        .map(|value| value.is_empty())
        .unwrap_or(true);
    if host != "uploads.github.com"
        || !query_empty
        || !fragment_empty
        || parsed.path() != expected_path
    {
        return Err("GitHub returned an unsafe release upload URL.".to_owned());
    }
    Ok(format!("https://uploads.github.com{expected_path}"))
}

fn validate_reported_download_url(value: Option<&Value>, expected: &str) -> Result<(), String> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| "GitHub returned an invalid release download URL.".to_owned())?;
    validate_https_url(raw, &["github.com"])?;
    let parsed = url::Url::parse(raw)
        .map_err(|_| "GitHub returned an unexpected release download URL.".to_owned())?;
    let expected_parsed = url::Url::parse(expected)
        .map_err(|_| "GitHub returned an unexpected release download URL.".to_owned())?;
    let query_empty = parsed.query().map(|value| value.is_empty()).unwrap_or(true);
    let fragment_empty = parsed
        .fragment()
        .map(|value| value.is_empty())
        .unwrap_or(true);
    if !query_empty || !fragment_empty || parsed.path() != expected_parsed.path() {
        return Err("GitHub returned an unexpected release download URL.".to_owned());
    }
    Ok(())
}

fn verification_uri(value: Option<&Value>) -> Result<String, String> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| "GitHub returned an invalid device verification URL.".to_owned())?;
    validate_https_url(raw, &OAUTH_HOSTS)?;
    let parsed = url::Url::parse(raw)
        .map_err(|_| "GitHub returned an unexpected device verification URL.".to_owned())?;
    let path = parsed.path().trim_end_matches('/');
    let query_empty = parsed.query().map(|value| value.is_empty()).unwrap_or(true);
    let fragment_empty = parsed
        .fragment()
        .map(|value| value.is_empty())
        .unwrap_or(true);
    if path != "/login/device" || !query_empty || !fragment_empty {
        return Err("GitHub returned an unexpected device verification URL.".to_owned());
    }
    Ok("https://github.com/login/device".to_owned())
}


fn normalized_archive_name(name: &str, is_dir: bool) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') {
        return Err("Plugin ZIP contains an invalid file path.".to_owned());
    }
    let mut normalized = name.replace('\\', "/");
    if normalized.starts_with('/') || normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
        return Err("Plugin ZIP contains an absolute file path.".to_owned());
    }
    if is_dir && normalized.ends_with('/') {
        normalized.pop();
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err("Plugin ZIP contains an unsafe file path.".to_owned());
    }
    Ok(parts.join("/"))
}

fn read_zip_member<R: std::io::Read>(
    member_size: u64,
    limit: u64,
    source: &mut R,
) -> Result<Vec<u8>, String> {
    let mut content = Vec::new();
    source
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut content)
        .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
    if content.len() as u64 > limit || content.len() as u64 != member_size {
        return Err("A required plugin ZIP file exceeds its size limit.".to_owned());
    }
    Ok(content)
}

fn validate_plugin_archive(content: &[u8]) -> Result<(Value, String), String> {
    if content.is_empty() || content.len() > PLUGIN_MAX_ARCHIVE_BYTES {
        return Err("Plugin ZIP is empty or exceeds the archive size limit.".to_owned());
    }
    let outcome = validate_plugin_archive_inner(content);
    let manifest = outcome?;
    let digest = hex_lowercase(&Sha256::digest(content));
    Ok((manifest, digest))
}

fn validate_plugin_archive_inner(content: &[u8]) -> Result<Value, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(content))
        .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
    let members_len = archive.len();
    if members_len == 0 || members_len > PLUGIN_MAX_FILES {
        return Err("Plugin ZIP has no files or contains too many entries.".to_owned());
    }
    struct Member {
        name: String,
        is_dir: bool,
        size: u64,
        mode: u32,
        encrypted: bool,
    }
    let mut members: Vec<Member> = Vec::with_capacity(members_len);
    for index in 0..members_len {
        let file = archive
            .by_index(index)
            .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
        members.push(Member {
            name: file.name().map(|v| v.to_string()).unwrap_or_default(),
            is_dir: file.is_dir(),
            size: file.size(),
            mode: file.unix_mode().unwrap_or(0),
            encrypted: file.encrypted(),
        });
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut files: BTreeMap<String, usize> = BTreeMap::new();
    let mut file_sizes: BTreeMap<String, u64> = BTreeMap::new();
    let mut total_size: u64 = 0;
    for (index, member) in members.iter().enumerate() {
        let name = normalized_archive_name(&member.name, member.is_dir)?;
        if !seen.insert(name.clone()) {
            return Err("Plugin ZIP contains duplicate paths.".to_owned());
        }
        let mode = member.mode & 0o170000;
        if mode == 0o120000 {
            return Err("Plugin ZIP may not contain symbolic links.".to_owned());
        }
        if member.encrypted {
            return Err("Encrypted files are not allowed in a plugin ZIP.".to_owned());
        }
        if member.is_dir {
            if mode != 0 && mode != 0o040000 {
                return Err("Plugin ZIP contains a non-directory entry.".to_owned());
            }
            continue;
        }
        if mode != 0 && mode != 0o100000 {
            return Err("Plugin ZIP contains a non-regular file.".to_owned());
        }
        total_size += member.size;
        if member.size > PLUGIN_MAX_UNPACKED_BYTES as u64
            || total_size > PLUGIN_MAX_UNPACKED_BYTES as u64
        {
            return Err("Plugin ZIP expands beyond the unpacked size limit.".to_owned());
        }
        files.insert(name.clone(), index);
        file_sizes.insert(name, member.size);
    }
    for name in files.keys() {
        let parts: Vec<&str> = name.split('/').collect();
        for index in 1..parts.len() {
            if files.contains_key(&parts[..index].join("/")) {
                return Err("Plugin ZIP contains a file path used as a directory.".to_owned());
            }
        }
    }
    let legacy_index = files.get(PLUGIN_MANIFEST).copied();
    let v2_index = files.get(PLUGIN_V2_MANIFEST).copied();
    if legacy_index.is_none() == v2_index.is_none() {
        return Err(
            "Plugin ZIP must contain exactly one root plugin.yml or plugin.json.".to_owned(),
        );
    }
    let license_size = match file_sizes.get("LICENSE") {
        Some(size) if *size > 0 && *size <= MAX_LICENSE_BYTES => *size,
        _ => {
            return Err(
                "Plugin ZIP must contain a nonempty root LICENSE of at most 1 MiB.".to_owned(),
            )
        }
    };
    let (manifest, license_content) = if let Some(index) = legacy_index {
        let manifest_bytes = {
            let mut source = archive
                .by_index(index)
                .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
            read_zip_member(
                file_sizes[PLUGIN_MANIFEST],
                PLUGIN_MAX_MANIFEST_BYTES as u64,
                &mut source,
            )?
        };
        let manifest_text = std::str::from_utf8(&manifest_bytes)
            .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
        let manifest = validate_manifest(&Value::Object(parse_plugin_yml(manifest_text)?))?;
        let license = {
            let mut source = archive
                .by_index(files["LICENSE"])
                .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
            read_zip_member(license_size, MAX_LICENSE_BYTES, &mut source)?
        };
        (manifest, license)
    } else {
        let index = v2_index.expect("проверено выше");
        let manifest_bytes = {
            let mut source = archive
                .by_index(index)
                .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
            read_zip_member(
                file_sizes[PLUGIN_V2_MANIFEST],
                PLUGIN_MAX_MANIFEST_BYTES as u64,
                &mut source,
            )?
        };
        let raw_manifest = parse_v2_manifest_bytes(&manifest_bytes)?;
        let manifest = validate_plugin_json_manifest(&raw_manifest)?;
        let mut package_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (name, file_index) in &files {
            if name == PLUGIN_V2_MANIFEST {
                continue;
            }
            let mut source = archive
                .by_index(*file_index)
                .map_err(|_| "Plugin ZIP is invalid or contains unreadable files.".to_owned())?;
            package_files.insert(
                name.clone(),
                read_zip_member(
                    file_sizes[name],
                    PLUGIN_MAX_UNPACKED_BYTES as u64,
                    &mut source,
                )?,
            );
        }
        let license = package_files.get("LICENSE").cloned().ok_or_else(|| {
            "Plugin ZIP must contain a nonempty root LICENSE of at most 1 MiB.".to_owned()
        })?;
        if raw_manifest.get("signature").is_some() {
            let verification = verify_manifest(&raw_manifest, &package_files);
            if verification.get("valid").and_then(Value::as_bool) != Some(true) {
                let reason = verification
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                return Err(format!(
                    "Plugin ZIP signature or integrity verification failed: {}.",
                    if reason.is_empty() {
                        "invalid package"
                    } else {
                        reason
                    }
                ));
            }
        } else {
            verify_unsigned_v2_integrity(&raw_manifest, &package_files)?;
        }
        (manifest, license)
    };
    if license_content
        .iter()
        .all(|byte| byte.is_ascii_whitespace())
    {
        return Err("Plugin ZIP root LICENSE must not be empty.".to_owned());
    }
    if manifest
        .get("entrypoint")
        .and_then(Value::as_str)
        .map(|path| !files.contains_key(path))
        .unwrap_or(true)
    {
        return Err("Plugin ZIP does not contain the manifest entrypoint.".to_owned());
    }
    Ok(manifest)
}

fn hex_lowercase(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}


struct CatalogContext {
    repository: Value,
    owner: String,
    repo: String,
    topics: Vec<String>,
    release: Value,
    tag_name: String,
    asset: Value,
}

struct GitHubPluginService<'a> {
    http: &'a HttpClient,
    client_id: String,
}

impl<'a> GitHubPluginService<'a> {
    fn new(http: &'a HttpClient) -> Self {
        let client_id = std::env::var(GITHUB_CLIENT_ID_ENV)
            .unwrap_or_default()
            .trim()
            .to_owned();
        Self { http, client_id }
    }

    fn status(&self) -> Result<Value, String> {
        let provider = SecretToolTokenProvider;
        let storage_available = SecretToolTokenProvider::available();
        let authorized = if storage_available {
            provider.get_token()?.is_some()
        } else {
            false
        };
        Ok(json!({
            "client_configured": !self.client_id.is_empty(),
            "authorized": authorized,
            "secure_storage_available": storage_available,
        }))
    }

    fn search(&self, query: &str, page: i64, per_page: i64) -> Result<Vec<Value>, String> {
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if query.chars().count() > 120 || !search_query_valid(&query) {
            return Err(
                "GitHub plugin search text contains unsupported characters or is too long."
                    .to_owned(),
            );
        }
        let page = page.clamp(1, 10);
        let per_page = per_page.clamp(1, 100);
        let mut terms = vec![
            format!("topic:{GITHUB_PLUGIN_TOPIC}"),
            "is:public".to_owned(),
        ];
        if !query.is_empty() {
            terms.push(query);
        }
        let params = form_urlencoded(&[
            ("q".to_owned(), terms.join(" ")),
            ("sort".to_owned(), "updated".to_owned()),
            ("order".to_owned(), "desc".to_owned()),
            ("page".to_owned(), page.to_string()),
            ("per_page".to_owned(), per_page.to_string()),
        ]);
        let (response, _) = self.api_json(
            &format!("/search/repositories?{params}"),
            "GET",
            None,
            None,
            &[],
            &[],
        )?;
        let items = response
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "GitHub returned an invalid plugin search response.".to_owned())?;
        let mut result: Vec<Value> = Vec::new();
        for item in items.iter().take(per_page as usize) {
            if !item.is_object() || item.get("private").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let visibility = item
                .get("visibility")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if !visibility.is_empty() && visibility != "public" {
                continue;
            }
            let topics = item.get("topics");
            if let Some(list) = topics.and_then(Value::as_array) {
                if !list
                    .iter()
                    .any(|topic| topic.as_str() == Some(GITHUB_PLUGIN_TOPIC))
                {
                    continue;
                }
            }
            match repository_summary(item, topics) {
                Ok(summary) => result.push(Value::Object(summary)),
                Err(_) => continue,
            }
        }
        Ok(result)
    }

    // поиск тем на GitHub

    fn theme_search(&self, query: &str, page: i64) -> Result<Vec<Value>, String> {
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if query.chars().count() > 120 || !search_query_valid(&query) {
            return Err("Поисковый запрос содержит недопустимые символы или слишком длинный".to_owned());
        }
        if !(1..=10).contains(&page) {
            return Err("Номер страницы должен быть от 1 до 10".to_owned());
        }
        let mut terms = vec![
            format!("topic:{THEME_GITHUB_TOPIC}"),
            "is:public".to_owned(),
        ];
        if !query.is_empty() {
            terms.push(query);
        }
        let params = form_urlencoded(&[
            ("q".to_owned(), terms.join(" ")),
            ("sort".to_owned(), "updated".to_owned()),
            ("order".to_owned(), "desc".to_owned()),
            ("page".to_owned(), page.to_string()),
            ("per_page".to_owned(), "30".to_owned()),
        ]);
        let (response, _) = self.api_json(
            &format!("/search/repositories?{params}"),
            "GET",
            None,
            None,
            &[],
            &[],
        )?;
        let items = response
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "GitHub вернул некорректный список тем".to_owned())?;
        let mut result: Vec<Value> = Vec::new();
        for item in items.iter().take(30) {
            if !item.is_object() || item.get("private").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            // если GitHub прислал topics, проверяем их; иначе оставляем результат
            let topics = item.get("topics");
            if let Some(list) = topics.and_then(Value::as_array) {
                if !list.iter().any(|topic| topic.as_str() == Some(THEME_GITHUB_TOPIC)) {
                    continue;
                }
            }
            let owner_name = item
                .get("owner")
                .filter(|value| value.is_object())
                .and_then(|value| value.get("login"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let repo = item.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            if !owner_name_valid(&owner_name) || !repo_name_valid(&repo) {
                continue;
            }
            let stars = theme_bounded_int(item.get("stargazers_count"), 0, 2_000_000_000)?;
            let topics = topics
                .and_then(Value::as_array)
                .map(|list| {
                    Value::Array(
                        list.iter()
                            .filter_map(|topic| topic.as_str())
                            .map(|topic| json!(topic.chars().take(100).collect::<String>()))
                            .collect(),
                    )
                })
                .unwrap_or_else(|| json!([]));
            result.push(json!({
                "owner": owner_name,
                "repo": repo,
                "full_name": format!("{owner_name}/{repo}"),
                "name": item.get("name").and_then(Value::as_str).unwrap_or(&repo).chars().take(100).collect::<String>(),
                "description": item.get("description").and_then(Value::as_str).unwrap_or("").chars().take(500).collect::<String>(),
                "url": format!("https://github.com/{owner_name}/{repo}"),
                "stars": stars,
                "updated_at": item.get("updated_at").and_then(Value::as_str).unwrap_or("").chars().take(40).collect::<String>(),
                "topics": topics,
            }));
        }
        Ok(result)
    }

    fn theme_install(&self, owner: &str, repo: &str, replace: bool) -> Result<(Value, Value), String> {
        theme_validate_repository_name(owner, repo)?;
        let repo_path = repository_path(owner, repo)?;
        let (repository, _) = self.api_json(&format!("/repos/{repo_path}"), "GET", None, None, &[], &[])?;
        if !repository.is_object()
            || repository.get("private").and_then(Value::as_bool) == Some(true)
            || repository.get("visibility").and_then(Value::as_str).unwrap_or("public").to_lowercase() != "public"
        {
            return Err("Репозиторий с темой должен быть публичным".to_owned());
        }
        let mut topics_value = repository.get("topics").cloned();
        if !topics_value.as_ref().map(Value::is_array).unwrap_or(false) {
            let (topic_response, _) = self.api_json(
                &format!("/repos/{repo_path}/topics"),
                "GET",
                None,
                None,
                &[],
                &[],
            )?;
            topics_value = Some(topic_response.get("names").cloned().unwrap_or(json!([])));
        }
        let topics = topics_value
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .map(|topic| topic.to_lowercase())
            .collect::<Vec<String>>();
        if !topics.iter().any(|topic| topic == THEME_GITHUB_TOPIC) {
            return Err("В репозитории отсутствует тема GitHub topic trial-launcher-theme".to_owned());
        }
        let (release, _) = self.api_json(
            &format!("/repos/{repo_path}/releases/latest"),
            "GET",
            None,
            None,
            &[],
            &[],
        )?;
        if !release.is_object()
            || release.get("draft").and_then(Value::as_bool) == Some(true)
            || release.get("prerelease").and_then(Value::as_bool) == Some(true)
        {
            return Err("У темы нет опубликованного стабильного GitHub Release".to_owned());
        }
        let tag = release.get("tag_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        if tag.is_empty() || tag.chars().count() > 128 {
            return Err("GitHub Release не содержит корректный тег версии".to_owned());
        }
        let asset = theme_select_asset(release.get("assets"), repo, &tag)?;
        let asset_id = theme_bounded_int(asset.get("id"), 1, 2_147_483_647)?;
        let asset_size = theme_bounded_int(asset.get("size"), 1, THEME_MAX_ARCHIVE_BYTES as i64)? as usize;
        let asset_url = format!("{GITHUB_API_URL}/repos/{repo_path}/releases/assets/{asset_id}");
        // заголовки github_headers, но Accept переопределён на octet-stream
        let mut request_headers = self.github_headers(None);
        for header in request_headers.iter_mut() {
            if header.0 == "Accept" {
                header.1 = "application/octet-stream".to_owned();
            }
        }
        let (content, _) = self.request_raw(
            &asset_url,
            "GET",
            None,
            &request_headers,
            THEME_MAX_ARCHIVE_BYTES as u64,
            &ASSET_HOSTS,
            &[],
            Some(asset_redirect_targets),
        )?;
        if content.len() != asset_size {
            return Err("Размер ZIP темы не совпадает с данными GitHub".to_owned());
        }
        let source_url = format!("https://github.com/{owner}/{repo}");
        let archive = super::themes::read_archive(&content)?;
        let theme = super::themes::normalize_theme(&archive.theme, &archive.metadata, "github", &source_url)?;
        if theme.get("id").and_then(Value::as_str).unwrap_or_default().to_lowercase() != repo.to_lowercase() {
            return Err("id в theme.json должен совпадать с именем репозитория GitHub".to_owned());
        }
        let release_version = if tag.starts_with('v') || tag.starts_with('V') { tag[1..].to_owned() } else { tag.clone() };
        if theme.get("version").and_then(Value::as_str).unwrap_or_default() != release_version {
            return Err("Версия в theme.yml должна совпадать с тегом GitHub Release".to_owned());
        }
        let installed = super::themes::persist(&super::themes::themes_root()?, &theme, replace)?;
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(&content);
        let metadata = json!({
            "full_name": format!("{owner}/{repo}"),
            "version": theme.get("version").cloned().unwrap_or_default(),
            "size": content.len(),
            "sha256": digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            "url": source_url,
        });
        Ok((installed, metadata))
    }

    fn details(&self, owner: &str, repo: &str) -> Result<Value, String> {
        let context = self.load_catalog_release(owner, repo)?;
        let download = self.verified_download_asset(&context)?.0;
        let repository = &context.repository;
        let release = &context.release;
        let mut summary = repository_summary(repository, Some(&json!(context.topics)))?;
        summary.insert("manifest".into(), download["manifest"].clone());
        summary.insert(
            "latest_release".into(),
            json!({
                "tag_name": context.tag_name,
                "name": release.get("name").and_then(Value::as_str).unwrap_or("").chars().take(200).collect::<String>(),
                "published_at": release.get("published_at").and_then(Value::as_str).unwrap_or(""),
                "url": release_url(&context.owner, &context.repo, &context.tag_name)?,
            }),
        );
        let mut download_public = download.as_object().cloned().unwrap_or_default();
        download_public.remove("manifest");
        summary.insert("download".into(), Value::Object(download_public));
        Ok(Value::Object(summary))
    }

    fn download_plugin_archive(&self, owner: &str, repo: &str) -> Result<(Value, Vec<u8>), String> {
        let context = self.load_catalog_release(owner, repo)?;
        self.verified_download_asset(&context)
    }

    fn load_catalog_release(&self, owner: &str, repo: &str) -> Result<CatalogContext, String> {
        validate_repository_name(owner, repo)?;
        let requested_path = repository_path(owner, repo)?;
        let (repository, _) = self.api_json(
            &format!("/repos/{requested_path}"),
            "GET",
            None,
            None,
            &[],
            &[],
        )?;
        if !repository.is_object()
            || repository.get("private").and_then(Value::as_bool) == Some(true)
        {
            return Err("This GitHub repository is not public.".to_owned());
        }
        let visibility = repository
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("public")
            .to_ascii_lowercase();
        if visibility != "public" {
            return Err("This GitHub repository is not public.".to_owned());
        }
        let (actual_owner, actual_repo) = repository_identity(&repository)?;
        let mut topics_value = repository.get("topics").cloned();
        if !topics_value.as_ref().map(Value::is_array).unwrap_or(false) {
            let (topic_response, _) = self.api_json(
                &format!(
                    "/repos/{}/topics",
                    repository_path(&actual_owner, &actual_repo)?
                ),
                "GET",
                None,
                None,
                &[],
                &[],
            )?;
            topics_value = Some(topic_response.get("names").cloned().unwrap_or(json!([])));
        }
        let normalized_topics: Vec<String> = topics_value
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .map(|topic| topic.to_ascii_lowercase())
            .collect();
        if !normalized_topics
            .iter()
            .any(|topic| topic == GITHUB_PLUGIN_TOPIC)
        {
            return Err("This repository is not tagged as a Nivora Launcher plugin.".to_owned());
        }
        let repo_path = repository_path(&actual_owner, &actual_repo)?;
        let (release, _) = self.api_json(
            &format!("/repos/{repo_path}/releases/latest"),
            "GET",
            None,
            None,
            &[],
            &[],
        )?;
        if !release.is_object()
            || release.get("draft").and_then(Value::as_bool) == Some(true)
            || release.get("prerelease").and_then(Value::as_bool) == Some(true)
        {
            return Err("This plugin has no published latest release.".to_owned());
        }
        let tag_name = required_text(release.get("tag_name"), "release tag", 128)?;
        let asset = select_zip_asset(release.get("assets"))?;
        Ok(CatalogContext {
            repository,
            owner: actual_owner,
            repo: actual_repo,
            topics: normalized_topics,
            release,
            tag_name,
            asset,
        })
    }

    fn verified_download_asset(
        &self,
        context: &CatalogContext,
    ) -> Result<(Value, Vec<u8>), String> {
        let owner = &context.owner;
        let repo = &context.repo;
        let asset = &context.asset;
        let asset_id = positive_int(asset.get("id"), "release asset id")?;
        let asset_url = format!(
            "{GITHUB_API_URL}/repos/{}/releases/assets/{asset_id}",
            repository_path(owner, repo)?
        );
        // запрашиваем содержимое ZIP как двоичные данные
        let mut request_headers = self.github_headers(None);
        for header in request_headers.iter_mut() {
            if header.0 == "Accept" {
                header.1 = "application/octet-stream".to_owned();
            }
        }
        let (content, _) = self.request_raw(
            &asset_url,
            "GET",
            None,
            &request_headers,
            PLUGIN_MAX_ARCHIVE_BYTES as u64,
            &ASSET_HOSTS,
            &[],
            Some(asset_redirect_targets),
        )?;
        if content.is_empty() {
            return Err("The latest plugin release ZIP is empty.".to_owned());
        }
        let expected_size = asset.get("size").and_then(Value::as_i64).unwrap_or(-1);
        if content.len() as i64 != expected_size {
            return Err(
                "The latest plugin release ZIP size does not match GitHub metadata.".to_owned(),
            );
        }
        let (manifest, digest) = validate_plugin_archive(&content)?;
        let asset_name = asset.get("name").and_then(Value::as_str).unwrap_or("");
        let download_url = asset_download_url(owner, repo, &context.tag_name, asset_name)?;
        validate_reported_download_url(asset.get("browser_download_url"), &download_url)?;
        let mut metadata = Map::new();
        metadata.insert("owner".into(), json!(owner));
        metadata.insert("repo".into(), json!(repo));
        metadata.insert("full_name".into(), json!(format!("{owner}/{repo}")));
        metadata.insert("tag_name".into(), json!(context.tag_name));
        metadata.insert(
            "version".into(),
            manifest.get("version").cloned().unwrap_or(Value::Null),
        );
        metadata.insert("file_name".into(), json!(asset_name));
        metadata.insert("download_url".into(), json!(download_url));
        metadata.insert("size".into(), json!(content.len()));
        metadata.insert("sha256".into(), json!(digest));
        metadata.insert("manifest".into(), manifest);
        Ok((Value::Object(metadata), content))
    }

    fn start_oauth_device_flow(&self) -> Result<Value, String> {
        let client_id = self.require_client_id()?;
        let (response, _) = self.form_json(
            GITHUB_DEVICE_CODE_URL,
            &[
                ("client_id".to_owned(), client_id),
                ("scope".to_owned(), GITHUB_OAUTH_SCOPE.to_owned()),
            ],
            &OAUTH_HOSTS,
            &[],
        )?;
        if !response.is_object() {
            return Err("GitHub returned an invalid device authorization response.".to_owned());
        }
        let device_code = required_text(response.get("device_code"), "device_code", 512)?;
        let user_code = required_text(response.get("user_code"), "user_code", 64)?;
        if device_code
            .chars()
            .any(|character| character.is_whitespace() || (character as u32) < 32)
        {
            return Err("GitHub returned an invalid device authorization code.".to_owned());
        }
        if user_code
            .chars()
            .any(|character| character.is_whitespace() || (character as u32) < 32)
        {
            return Err("GitHub returned an invalid user code.".to_owned());
        }
        let verification = verification_uri(response.get("verification_uri"))?;
        let expires_in = bounded_int(response.get("expires_in"), 1, 86_400, "expires_in")?;
        let interval = bounded_int(
            response.get("interval").or(Some(&json!(5))),
            1,
            60,
            "interval",
        )?;
        Ok(json!({
            "device_code": device_code,
            "user_code": user_code,
            "verification_uri": verification,
            "expires_in": expires_in,
            "interval": interval,
        }))
    }

    fn poll_oauth_device_flow(&self, device_code: &str) -> Result<Value, String> {
        let client_id = self.require_client_id()?;
        let provider = SecretToolTokenProvider;
        let device_code = if device_code.is_empty() || device_code.len() > 512 {
            return Err("GitHub returned an invalid device_code.".to_owned());
        } else {
            device_code.to_owned()
        };
        if device_code
            .chars()
            .any(|character| character.is_whitespace() || (character as u32) < 32)
        {
            return Err("The GitHub device code is invalid.".to_owned());
        }
        let (response, _) = self.form_json(
            GITHUB_ACCESS_TOKEN_URL,
            &[
                ("client_id".to_owned(), client_id),
                ("device_code".to_owned(), device_code),
                (
                    "grant_type".to_owned(),
                    "urn:ietf:params:oauth:grant-type:device_code".to_owned(),
                ),
            ],
            &OAUTH_HOSTS,
            &[400],
        )?;
        if !response.is_object() {
            return Err("GitHub returned an invalid OAuth response.".to_owned());
        }
        let oauth_error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        match oauth_error.as_str() {
            "authorization_pending" => return Ok(json!({ "status": "pending" })),
            "slow_down" => {
                let interval = bounded_int(
                    response.get("interval").or(Some(&json!(5))),
                    1,
                    60,
                    "interval",
                )?;
                return Ok(json!({ "status": "slow_down", "interval": interval + 5 }));
            }
            "access_denied" => return Ok(json!({ "status": "denied" })),
            "expired_token" => return Ok(json!({ "status": "expired" })),
            "" => {}
            _ => return Err("GitHub device authorization failed.".to_owned()),
        }
        let access_token = required_text(response.get("access_token"), "access_token", 4096)?;
        if !valid_bearer_token(&access_token) {
            return Err("GitHub returned an invalid access token.".to_owned());
        }
        provider.set_token(&access_token).map_err(|_| {
            "GitHub authorization could not be saved by the configured token provider.".to_owned()
        })?;
        Ok(json!({ "status": "authorized" }))
    }

    fn publish_plugin(&self, archive: &[u8], description: Option<&str>) -> Result<Value, String> {
        let token = self.get_access_token()?;
        let (manifest, digest) = validate_plugin_archive(archive)?;
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !release_version_valid(version) {
            return Err("Plugin version must use letters, numbers, dots, underscores, plus signs, or hyphens.".to_owned());
        }
        let repo_name = manifest
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let repo_description = match description {
            Some(text) => text.to_owned(),
            None => manifest
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        };
        let repo_description = repo_description
            .trim()
            .chars()
            .take(350)
            .collect::<String>();
        let (created, _) = self.api_json(
            "/user/repos",
            "POST",
            Some(&json!({
                "name": repo_name,
                "description": repo_description,
                "private": false,
                "auto_init": true,
                "has_issues": false,
                "has_projects": false,
                "has_wiki": false,
            })),
            Some(&token),
            &[],
            &[],
        )?;
        if !created.is_object()
            || created.get("private").and_then(Value::as_bool) != Some(false)
            || !created
                .get("visibility")
                .and_then(Value::as_str)
                .unwrap_or("public")
                .eq_ignore_ascii_case("public")
        {
            return Err("GitHub did not create a public plugin repository.".to_owned());
        }
        let owner_data = created
            .get("owner")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or(json!({}));
        let owner = owner_data
            .get("login")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let repo = created
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        validate_repository_name(&owner, &repo)?;
        if !repo.eq_ignore_ascii_case(&repo_name) {
            return Err("GitHub created a repository with an unexpected name.".to_owned());
        }
        let repo_path = repository_path(&owner, &repo)?;
        let (topic_response, _) = self.api_json(
            &format!("/repos/{repo_path}/topics"),
            "PUT",
            Some(&json!({ "names": [GITHUB_PLUGIN_TOPIC] })),
            Some(&token),
            &[],
            &[],
        )?;
        let names = topic_response
            .get("names")
            .and_then(Value::as_array)
            .cloned();
        if !names
            .map(|items| {
                items
                    .iter()
                    .any(|name| name.as_str() == Some(GITHUB_PLUGIN_TOPIC))
            })
            .unwrap_or(false)
        {
            return Err("GitHub did not apply the plugin catalog topic.".to_owned());
        }
        let tag_name = format!("v{version}");
        let release_body = json!({
            "tag_name": tag_name,
            "target_commitish": created.get("default_branch").and_then(Value::as_str).unwrap_or("main"),
            "name": format!("{} {version}", manifest.get("name").and_then(Value::as_str).unwrap_or("")).chars().take(200).collect::<String>(),
            "body": repo_description,
            "draft": false,
            "prerelease": false,
        });
        let (release, _) = self.api_json(
            &format!("/repos/{repo_path}/releases"),
            "POST",
            Some(&release_body),
            Some(&token),
            &[],
            &[],
        )?;
        if !release.is_object() {
            return Err("GitHub returned an invalid release response.".to_owned());
        }
        if release.get("tag_name").and_then(Value::as_str) != Some(tag_name.as_str())
            || release.get("draft").and_then(Value::as_bool) == Some(true)
            || release.get("prerelease").and_then(Value::as_bool) == Some(true)
        {
            return Err("GitHub did not create the expected published release.".to_owned());
        }
        let release_id = positive_int(release.get("id"), "release id")?;
        let plugin_id = manifest.get("id").and_then(Value::as_str).unwrap_or("");
        let asset_name = format!("{plugin_id}-{tag_name}.zip");
        let upload_base = release_upload_url(release.get("upload_url"), &owner, &repo, release_id)?;
        let upload_url = format!(
            "{upload_base}?{}",
            form_urlencoded(&[("name".to_owned(), asset_name.clone())])
        );
        let mut upload_headers = self.github_headers(Some(&token));
        upload_headers.push(("Content-Type".to_owned(), "application/zip".to_owned()));
        let (uploaded_raw, _) = self.request_raw(
            &upload_url,
            "POST",
            Some(archive),
            &upload_headers,
            MAX_GITHUB_JSON_BYTES,
            &UPLOAD_HOSTS,
            &[],
            None,
        )?;
        let uploaded = parse_json_bytes(&uploaded_raw, "release asset")?;
        if !uploaded.is_object() {
            return Err("release asset response must be an object.".to_owned());
        }
        if let Some(reported) = optional_int(uploaded.get("size")) {
            if reported != archive.len() as i64 {
                return Err("GitHub reported an unexpected release asset size.".to_owned());
            }
        }
        let reported_digest = uploaded
            .get("digest")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if let Some(digest_part) = reported_digest.strip_prefix("sha256:") {
            if digest_part.to_ascii_lowercase() != digest {
                return Err(
                    "GitHub reported a different SHA-256 for the uploaded plugin.".to_owned(),
                );
            }
        }
        let download_url = asset_download_url(&owner, &repo, &tag_name, &asset_name)?;
        validate_reported_download_url(uploaded.get("browser_download_url"), &download_url)?;
        Ok(json!({
            "owner": owner,
            "repo": repo,
            "full_name": format!("{owner}/{repo}"),
            "repository_url": format!(
                "https://github.com/{}/{}",
                percent_encode_path_segment(&owner),
                percent_encode_path_segment(&repo)
            ),
            "release_url": release_url(&owner, &repo, &tag_name)?,
            "download_url": download_url,
            "tag_name": tag_name,
            "file_name": asset_name,
            "size": archive.len(),
            "sha256": digest,
            "manifest": manifest,
        }))
    }

    fn api_json(
        &self,
        path: &str,
        method: &str,
        body: Option<&Value>,
        token: Option<&str>,
        headers: &[(String, String)],
        allowed_statuses: &[u16],
    ) -> Result<(Value, u16), String> {
        let url = format!("{GITHUB_API_URL}{path}");
        let mut request_headers: Vec<(String, String)> = self.github_headers(token);
        request_headers.extend(headers.iter().cloned());
        let data = match body {
            Some(value) => {
                // setdefault: чужой Content-Type не перетирается
                if !request_headers
                    .iter()
                    .any(|header| header.0 == "Content-Type")
                {
                    request_headers
                        .push(("Content-Type".to_owned(), "application/json".to_owned()));
                }
                Some(
                    serde_json::to_string(value)
                        .map_err(|_| "Не удалось сериализовать запрос к GitHub".to_owned())?
                        .into_bytes(),
                )
            }
            None => None,
        };
        let (raw, status) = self.request_raw(
            &url,
            method,
            data.as_deref(),
            &request_headers,
            MAX_GITHUB_JSON_BYTES,
            &API_HOSTS,
            allowed_statuses,
            None,
        )?;
        Ok((parse_json_bytes(&raw, "GitHub")?, status))
    }

    fn form_json(
        &self,
        url: &str,
        values: &[(String, String)],
        allowed_hosts: &[&str],
        allowed_statuses: &[u16],
    ) -> Result<(Value, u16), String> {
        let data = form_urlencoded(values).into_bytes();
        let headers = vec![
            ("Accept".to_owned(), "application/json".to_owned()),
            (
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            ),
        ];
        let (raw, status) = self.request_raw(
            url,
            "POST",
            Some(&data),
            &headers,
            MAX_GITHUB_JSON_BYTES,
            allowed_hosts,
            allowed_statuses,
            None,
        )?;
        Ok((parse_json_bytes(&raw, "GitHub OAuth")?, status))
    }

    /// выполняет HTTP-запрос с ограничениями размера, хостов и кодов ответа
    #[allow(clippy::too_many_arguments)]
    fn request_raw(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: &[(String, String)],
        max_bytes: u64,
        allowed_hosts: &[&str],
        allowed_statuses: &[u16],
        redirect_transitions: Option<fn(&str) -> &'static [&'static str]>,
    ) -> Result<(Vec<u8>, u16), String> {
        let initial_host = validate_https_url(url, allowed_hosts)?;
        let state = std::cell::RefCell::new((initial_host, 0usize));
        let validator = move |target: &str| -> Result<(), String> {
            let target_host = validate_https_url(target, allowed_hosts)?;
            let (current_host, count) = {
                let mut state = state.borrow_mut();
                state.1 += 1;
                state.clone()
            };
            let transition_list: &[&str] = match redirect_transitions {
                Some(transitions) => transitions(&current_host),
                None => &[],
            };
            let fallback = [current_host.as_str()];
            let allowed: &[&str] = if transition_list.is_empty() {
                &fallback
            } else {
                transition_list
            };
            if count > 4 || !allowed.contains(&target_host.as_str()) {
                return Err("GitHub returned an unsafe or excessive redirect.".to_owned());
            }
            *state.borrow_mut() = (target_host, count);
            Ok(())
        };
        let response = self.http.request_detailed(
            url,
            method,
            data,
            Some(headers),
            Some(max_bytes),
            Some(&validator),
        )?;
        let status = response.status;
        if (200..300).contains(&status) {
            return Ok((response.body, 200));
        }
        if allowed_statuses.contains(&status) {
            return Ok((response.body, status));
        }
        Err(format!("GitHub returned HTTP {status}."))
    }

    fn github_headers(&self, token: Option<&str>) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "Accept".to_owned(),
                "application/vnd.github+json".to_owned(),
            ),
            (
                "X-GitHub-Api-Version".to_owned(),
                GITHUB_API_VERSION.to_owned(),
            ),
        ];
        if let Some(token) = token {
            headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
        }
        headers
    }

    fn require_client_id(&self) -> Result<String, String> {
        if self.client_id.is_empty() {
            return Err(format!(
                "GitHub OAuth is not configured. Set {GITHUB_CLIENT_ID_ENV} or pass client_id to GitHubPluginService."
            ));
        }
        Ok(self.client_id.clone())
    }

    fn get_access_token(&self) -> Result<String, String> {
        let provider = SecretToolTokenProvider;
        let token = provider.get_token().map_err(|_| {
            "Could not retrieve GitHub authorization from the configured token provider.".to_owned()
        })?;
        let token = match token {
            Some(token) => token,
            None => return Err("Sign in to GitHub before publishing a plugin.".to_owned()),
        };
        if token.len() > 4096 || token != token.trim() {
            return Err("Sign in to GitHub before publishing a plugin.".to_owned());
        }
        if !valid_bearer_token(&token) {
            return Err("The configured GitHub authorization is invalid.".to_owned());
        }
        Ok(token)
    }
}

fn select_zip_asset(assets: Option<&Value>) -> Result<Value, String> {
    let assets = assets
        .and_then(Value::as_array)
        .ok_or_else(|| "The latest plugin release has no ZIP asset.".to_owned())?;
    for asset in assets {
        if !asset.is_object() {
            continue;
        }
        let name = asset.get("name").and_then(Value::as_str).unwrap_or("");
        if !name.to_ascii_lowercase().ends_with(".zip") {
            continue;
        }
        if name.is_empty()
            || name.len() > 200
            || name.contains('/')
            || name.contains('\\')
            || name.chars().any(|character| (character as u32) < 32)
        {
            continue;
        }
        let size = match optional_int(asset.get("size")) {
            Some(size) if size >= 1 && size <= PLUGIN_MAX_ARCHIVE_BYTES as i64 => size,
            _ => continue,
        };
        let asset_id = match positive_int(asset.get("id"), "release asset id") {
            Ok(asset_id) => asset_id,
            Err(_) => continue,
        };
        let mut selected = asset.as_object().cloned().unwrap_or_default();
        selected.insert("id".into(), json!(asset_id));
        selected.insert("name".into(), json!(name));
        selected.insert("size".into(), json!(size));
        return Ok(Value::Object(selected));
    }
    Err("The latest plugin release must contain a bounded ZIP asset.".to_owned())
}


fn service_for(request: &Value) -> Result<(ConfigStore, std::sync::Arc<HttpClient>), String> {
    let store = ConfigStore::load()?;
    let operation_id = request
        .get("operation_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let http = client_from_settings(store.settings(), operation_id)?;
    Ok((store, http))
}

/// декодирует base64 с проверкой максимального размера до и после декодирования
fn decode_base64_bounded(
    value: Option<&Value>,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    let encoded = value.and_then(Value::as_str).unwrap_or("");
    if encoded.len() > 4 * limit.div_ceil(3) {
        return Err(format!("{label} слишком большой"));
    }
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| format!("{label} повреждён"))?;
    if content.len() > limit {
        return Err(format!("{label} слишком большой"));
    }
    Ok(content)
}

pub(crate) fn github_plugin_status(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    GitHubPluginService::new(&http).status()
}

pub(crate) fn github_plugin_search(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    let service = GitHubPluginService::new(&http);
    let page = request.get("page").and_then(Value::as_i64).unwrap_or(1);
    let query = request.get("query").and_then(Value::as_str).unwrap_or("");
    let results = service.search(query, page, 30)?;
    Ok(json!({ "results": results, "page": page }))
}


/// проверяет целое значение темы на допустимый диапазон и отвергает логические типы
fn theme_bounded_int(value: Option<&Value>, minimum: i64, maximum: i64) -> Result<i64, String> {
    let invalid = "GitHub вернул некорректное числовое значение".to_owned();
    if value.is_some_and(Value::is_boolean) {
        return Err(invalid);
    }
    let number = value
        .and_then(|value| json_int(Some(value)))
        .ok_or_else(|| invalid)?;
    if number < minimum || number > maximum {
        return Err("GitHub вернул числовое значение вне допустимого диапазона".to_owned());
    }
    Ok(number)
}

/// проверяет имя владельца и репозитория GitHub
fn theme_validate_repository_name(owner: &str, repo: &str) -> Result<(), String> {
    if !owner_name_valid(owner) || !repo_name_valid(repo) || repo == "." || repo == ".." {
        return Err("Некорректный адрес GitHub-репозитория".to_owned());
    }
    Ok(())
}

/// выбирает единственный ZIP-архив темы из релиза
fn theme_select_asset(assets: Option<&Value>, repo: &str, tag: &str) -> Result<Value, String> {
    let invalid = "GitHub Release не содержит ZIP темы".to_owned();
    let assets = assets.and_then(Value::as_array).ok_or_else(|| invalid.clone())?;
    let candidates: Vec<&Value> = assets
        .iter()
        .filter(|asset| {
            asset.is_object()
                && asset.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase().ends_with(".zip")
                && asset.get("state").and_then(Value::as_str) != Some("starter")
        })
        .collect();
    let stripped = tag.trim_start_matches(['v', 'V']);
    let expected = [
        format!("{}-{}.zip", repo, tag).to_lowercase(),
        format!("{}-v{}.zip", repo, stripped).to_lowercase(),
    ];
    let preferred: Vec<&Value> = candidates
        .iter()
        .copied()
        .filter(|asset| {
            expected.contains(
                &asset.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase(),
            )
        })
        .collect();
    let chosen = if !preferred.is_empty() { preferred } else { candidates };
    if chosen.len() != 1 {
        return Err("В GitHub Release должен быть один ZIP темы с однозначным именем".to_owned());
    }
    Ok(chosen[0].clone())
}

pub(crate) fn github_theme_search(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    let service = GitHubPluginService::new(&http);
    let page = request.get("page").and_then(Value::as_i64).unwrap_or(1);
    let query = request.get("query").and_then(Value::as_str).unwrap_or("");
    let results = service.theme_search(query, page)?;
    Ok(json!({ "results": results, "page": page }))
}

pub(crate) fn github_theme_install(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    let service = GitHubPluginService::new(&http);
    let owner = request.get("owner").and_then(Value::as_str).unwrap_or_default();
    let repo = request.get("repo").and_then(Value::as_str).unwrap_or_default();
    let replace = request.get("replace").and_then(Value::as_bool).unwrap_or(false);
    let (theme, metadata) = service.theme_install(owner, repo, replace)?;
    super::events::emit_status_event(
        "theme-imported",
        &format!(
            "Тема «{}» загружена из GitHub.",
            theme.get("name").and_then(Value::as_str).unwrap_or_default()
        ),
    );
    Ok(json!({ "theme": theme, "themes": super::themes::list()?, "metadata": metadata }))
}

pub(crate) fn github_plugin_details(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    GitHubPluginService::new(&http).details(
        request.get("owner").and_then(Value::as_str).unwrap_or(""),
        request.get("repo").and_then(Value::as_str).unwrap_or(""),
    )
}

pub(crate) fn github_plugin_install(request: &Value) -> Result<Value, String> {
    let (config, http) = service_for(request)?;
    let service = GitHubPluginService::new(&http);
    let (metadata, content) = service.download_plugin_archive(
        request.get("owner").and_then(Value::as_str).unwrap_or(""),
        request.get("repo").and_then(Value::as_str).unwrap_or(""),
    )?;
    let store = PluginStore::new(&config)?;
    let replace = request
        .get("replace")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let plugin = store.install_archive(&content, false, replace)?;
    super::events::emit_status_event(
        "plugins-changed",
        &format!(
            "Архив плагина «{}» сохранён.",
            plugin.get("name").and_then(Value::as_str).unwrap_or("")
        ),
    );
    Ok(json!({
        "plugin": plugin,
        "plugins": store.list()?,
        "metadata": {
            "full_name": metadata.get("full_name").cloned().unwrap_or(Value::Null),
            "version": metadata.get("version").cloned().unwrap_or(Value::Null),
            "size": metadata.get("size").cloned().unwrap_or(Value::Null),
            "sha256": metadata.get("sha256").cloned().unwrap_or(Value::Null),
        },
    }))
}

pub(crate) fn github_plugin_oauth_start(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    GitHubPluginService::new(&http).start_oauth_device_flow()
}

pub(crate) fn github_plugin_oauth_poll(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    GitHubPluginService::new(&http).poll_oauth_device_flow(
        request
            .get("device_code")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )
}

pub(crate) fn github_plugin_sign_out(_request: &Value) -> Result<Value, String> {
    SecretToolTokenProvider.clear_token()?;
    Ok(json!({ "authorized": false }))
}

pub(crate) fn github_plugin_publish(request: &Value) -> Result<Value, String> {
    let (_, http) = service_for(request)?;
    let archive = decode_base64_bounded(
        request.get("data"),
        PLUGIN_MAX_ARCHIVE_BYTES,
        "Архив плагина",
    )?;
    GitHubPluginService::new(&http)
        .publish_plugin(&archive, request.get("description").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_name_rules_match_python_regexes() {
        assert!(owner_name_valid("a"));
        assert!(owner_name_valid("User-9"));
        assert!(!owner_name_valid("-user"));
        assert!(!owner_name_valid(&"x".repeat(40)));
        assert!(repo_name_valid("my.plugin_v2"));
        assert!(!repo_name_valid("."));
        assert!(!repo_name_valid(".."));
        assert!(!repo_name_valid("sl/ash"));
        assert!(!repo_name_valid(&"r".repeat(102)));
    }

    #[test]
    fn search_query_rules_mirror_python() {
        assert!(search_query_valid("hello world_1.0+beta-rc"));
        assert!(!search_query_valid("запрос"));
        assert!(!search_query_valid("a\nb"));
        assert!(!search_query_valid(&"x".repeat(121)));
    }

    #[test]
    fn bearer_token_validation() {
        assert!(valid_bearer_token("ghp_abc123"));
        assert!(!valid_bearer_token(""));
        assert!(!valid_bearer_token("token with space"));
        assert!(!valid_bearer_token("токен"));
    }

    #[test]
    fn https_url_validation_pins_hosts() {
        assert_eq!(
            validate_https_url("https://api.github.com/repos", &API_HOSTS).unwrap(),
            "api.github.com"
        );
        assert_eq!(
            validate_https_url("https://GitHub.com:443/login/device", &OAUTH_HOSTS).unwrap(),
            "github.com"
        );
        assert!(validate_https_url("http://api.github.com/", &API_HOSTS).is_err());
        assert!(validate_https_url("https://evil.com/", &API_HOSTS).is_err());
        assert!(validate_https_url("https://user@api.github.com/", &API_HOSTS).is_err());
        assert!(validate_https_url("https://api.github.com:8443/", &API_HOSTS).is_err());
        assert!(validate_https_url("https://api.github.com/#frag", &API_HOSTS).is_err());
    }

    #[test]
    fn int_helpers_match_python_semantics() {
        assert_eq!(json_int(Some(&json!(5))), Some(5));
        assert_eq!(json_int(Some(&json!(5.9))), Some(5));
        assert_eq!(json_int(Some(&json!(" 7 "))), Some(7));
        assert_eq!(json_int(Some(&json!(true))), Some(1));
        assert_eq!(json_int(Some(&json!("x"))), None);
        assert_eq!(json_int(None), None);
        assert_eq!(bounded_int(Some(&json!(99)), 1, 10, "page").unwrap(), 10);
        assert!(positive_int(Some(&json!(0)), "release id").is_err());
        assert_eq!(optional_int(Some(&json!(-4))), Some(0));
    }

    #[test]
    fn normalized_archive_name_rejects_unsafe_paths() {
        assert_eq!(
            normalized_archive_name("a/b.txt", false).unwrap(),
            "a/b.txt"
        );
        assert_eq!(normalized_archive_name("dir/", true).unwrap(), "dir");
        assert!(normalized_archive_name("a/../b", false).is_err());
        assert!(normalized_archive_name("/abs", false).is_err());
        assert!(normalized_archive_name("C:/win", false).is_err());
        assert!(normalized_archive_name("a//b", false).is_err());
        assert!(normalized_archive_name("", false).is_err());
    }

    #[test]
    fn archive_validation_requires_license_and_single_manifest() {
        fn make_zip(entries: &[(&str, &str, u32)]) -> Vec<u8> {
            let mut cursor = std::io::Cursor::new(Vec::new());
            let mut writer = zip::ZipWriter::new(&mut cursor);
            use zip::write::SimpleFileOptions;
            for (name, content, mode) in entries {
                let options = SimpleFileOptions::default().unix_permissions(*mode);
                writer.start_file(*name, options).unwrap();
                writer.write_all(content.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
            cursor.into_inner()
        }
        let manifest_yml =
            "id: demo\nname: Demo\nversion: 1.0.0\nentrypoint: main.js\ndescription: тест\n";
        // нет LICENSE
        let archive = make_zip(&[
            (PLUGIN_MANIFEST, manifest_yml, 0o100644),
            ("main.js", "x", 0o100644),
        ]);
        let error = validate_plugin_archive(&archive).unwrap_err();
        assert_eq!(
            error,
            "Plugin ZIP must contain a nonempty root LICENSE of at most 1 MiB."
        );
        // корректный минимальный архив: manifest + entrypoint + LICENSE
        let archive = make_zip(&[
            (PLUGIN_MANIFEST, manifest_yml, 0o100644),
            ("main.js", "console.log(1)", 0o100644),
            ("LICENSE", "MIT", 0o100644),
        ]);
        let (manifest, digest) = validate_plugin_archive(&archive).unwrap();
        assert_eq!(manifest["id"], json!("demo"));
        assert_eq!(digest.len(), 64);
        // два манифеста сразу
        let archive = make_zip(&[
            (PLUGIN_MANIFEST, manifest_yml, 0o100644),
            (PLUGIN_V2_MANIFEST, "{}", 0o100644),
            ("main.js", "x", 0o100644),
            ("LICENSE", "MIT", 0o100644),
        ]);
        let error = validate_plugin_archive(&archive).unwrap_err();
        assert_eq!(
            error,
            "Plugin ZIP must contain exactly one root plugin.yml or plugin.json."
        );
        // entrypoint отсутствует
        let archive = make_zip(&[
            (PLUGIN_MANIFEST, manifest_yml, 0o100644),
            ("other.js", "x", 0o100644),
            ("LICENSE", "MIT", 0o100644),
        ]);
        let error = validate_plugin_archive(&archive).unwrap_err();
        assert_eq!(
            error,
            "Plugin ZIP does not contain the manifest entrypoint."
        );
        // пустой LICENSE (только пробелы)
        let archive = make_zip(&[
            (PLUGIN_MANIFEST, manifest_yml, 0o100644),
            ("main.js", "x", 0o100644),
            ("LICENSE", " \n\t", 0o100644),
        ]);
        let error = validate_plugin_archive(&archive).unwrap_err();
        assert_eq!(error, "Plugin ZIP root LICENSE must not be empty.");
    }

    #[test]
    fn zip_asset_selection_bounds() {
        let assets = json!([
            { "name": "notes.txt", "size": 10, "id": 1 },
            { "name": "bad/name.zip", "size": 10, "id": 2 },
            { "name": "plugin.zip", "size": 100, "id": 3 },
        ]);
        let selected = select_zip_asset(Some(&assets)).unwrap();
        assert_eq!(selected["name"], json!("plugin.zip"));
        assert!(select_zip_asset(Some(&json!("not-a-list"))).is_err());
        assert!(select_zip_asset(Some(&json!([]))).is_err());
    }

    #[test]
    fn bounded_base64_decode_matches_gui_service() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert_eq!(
            decode_base64_bounded(Some(&json!(encoded)), 1024, "Архив").unwrap(),
            b"hello"
        );
        let error = decode_base64_bounded(Some(&json!("!!!")), 1024, "Архив").unwrap_err();
        assert_eq!(error, "Архив повреждён");
        let big = "x".repeat(4 * ((16 + 2) / 3) + 1);
        let error = decode_base64_bounded(Some(&json!(big)), 16, "Архив").unwrap_err();
        assert_eq!(error, "Архив слишком большой");
    }

    #[test]
    fn upload_url_template_stripped_and_pinned() {
        let value = json!("https://uploads.github.com/repos/o/r/releases/123/assets{?name,label}");
        let url = release_upload_url(Some(&value), "o", "r", 123).unwrap();
        assert_eq!(
            url,
            "https://uploads.github.com/repos/o/r/releases/123/assets"
        );
        assert!(release_upload_url(
            Some(&json!("https://evil.com/repos/o/r/releases/1/assets")),
            "o",
            "r",
            1
        )
        .is_err());
        assert!(release_upload_url(
            Some(&json!("https://uploads.github.com/other/path")),
            "o",
            "r",
            1
        )
        .is_err());
    }

    #[test]
    fn reported_download_url_must_match_expected_path() {
        let expected = "https://github.com/o/r/releases/download/v1/p.zip";
        assert!(validate_reported_download_url(
            Some(&json!("https://github.com/o/r/releases/download/v1/p.zip")),
            expected
        )
        .is_ok());
        assert!(validate_reported_download_url(
            Some(&json!(
                "https://github.com/o/r/releases/download/v1/other.zip"
            )),
            expected
        )
        .is_err());
        assert!(validate_reported_download_url(
            Some(&json!("https://evil.com/o/r/releases/download/v1/p.zip")),
            expected
        )
        .is_err());
        assert!(validate_reported_download_url(
            Some(&json!(
                "https://github.com/o/r/releases/download/v1/p.zip?x=1"
            )),
            expected
        )
        .is_err());
    }

    #[test]
    fn verification_uri_pins_login_device() {
        assert_eq!(
            verification_uri(Some(&json!("https://github.com/login/device"))).unwrap(),
            "https://github.com/login/device"
        );
        assert!(verification_uri(Some(&json!("https://github.com/evil"))).is_err());
        assert!(verification_uri(Some(&json!("https://github.com/login/device?x=1"))).is_err());
        assert!(verification_uri(None).is_err());
    }
}
