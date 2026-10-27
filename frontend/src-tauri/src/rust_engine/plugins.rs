use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use super::config::ConfigStore;
use super::mods::random_hex;
use super::net::atomic_json_write;
use super::plugin_security::{canonical_json_bytes, parse_strict_json, PluginTrustStore};

mod manifest;
mod package;
mod permission_state;
mod store;

// дочерние модули обращаются к общим помощникам через `super::*`,
// поэтому общие импорты и функции обязаны быть видимыми в родителе

// `super::*` в дочерних модулях видит только приватные элементы родителя;
// соседние помощники подключаются напрямую: см. plugins/manifest.rs и plugins/store.rs

use package::{read_regular_package_file, read_v2_package, reject_v2_runtime_paths, V2Package};
pub(crate) use package::verify_unsigned_v2_integrity;
use permission_state::{read_permission_state, write_permission_state};

pub(crate) const PLUGIN_MANIFEST: &str = "plugin.yml";
pub(crate) const PLUGIN_V2_MANIFEST: &str = "plugin.json";
pub(crate) const PLUGIN_MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub(crate) const PLUGIN_MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const PLUGIN_MAX_UNPACKED_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const PLUGIN_MAX_FILES: usize = 4096;
const PLUGIN_RUNTIME_MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const PLUGIN_PERMISSION_FILE: &str = "plugin-permissions.json";
const PLUGIN_PERMISSION_SCHEMA: i64 = 1;
const V2_DATA_DIRECTORIES: &[&str] = &["storage", "files", "config", "database"];
const MAX_LISTED_FILES: usize = 2000;
const MAX_PERMISSION_STATE_BYTES: usize = 1024 * 1024;
const MAX_STORAGE_BYTES: usize = 1024 * 1024;
const MAX_CONFIG_BYTES: usize = 256 * 1024;
const MAX_PLUGIN_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_LAUNCHER_FILE_BYTES: usize = 2 * 1024 * 1024;

// шаблон с `\z` отклоняет завершающий перевод строки
const PLUGIN_ID_PATTERN: &str = r"^[a-z][a-z0-9._-]{1,63}\z";
const STORAGE_KEY_PATTERN: &str = r"^[A-Za-z0-9._-]{1,120}\z";
const CONFIG_KEY_PATTERN: &str = r"^[A-Za-z0-9._-]{1,120}\z";
const HEX64_PATTERN: &str = r"^[0-9a-f]{64}\z";
const HOSTNAME_PATTERN: &str =
    r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+\z";
const YAML_KEY_PATTERN: &str = r"^[A-Za-z][A-Za-z0-9_.-]{0,63}\z";
const SCHEMA_IDENTIFIER_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]{0,79}\z";
const YAML_INT_PATTERN: &str = r"^-?\d+\z";
const YAML_FLOAT_PATTERN: &str = r"^-?(?:\d+\.\d*|\d*\.\d+)\z";
const WINDOWS_DRIVE_PATTERN: &str = r"^[A-Za-z]:";
const COLOR_PATTERN: &str = r"^#[0-9A-Fa-f]{6}\z";

static PLUGIN_ID_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(PLUGIN_ID_PATTERN).expect("валидный regex id плагина"));
static STORAGE_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(STORAGE_KEY_PATTERN).expect("валидный regex ключа storage"));
static CONFIG_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(CONFIG_KEY_PATTERN).expect("валидный regex ключа config"));
static HEX64_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(HEX64_PATTERN).expect("валидный regex hex64"));
static HOSTNAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(HOSTNAME_PATTERN).expect("валидный regex хоста"));
static YAML_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(YAML_KEY_PATTERN).expect("валидный regex ключа yaml"));
static SCHEMA_IDENTIFIER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(SCHEMA_IDENTIFIER_PATTERN).expect("валидный regex идентификатора"));
static YAML_INT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(YAML_INT_PATTERN).expect("валидный regex целого"));
static YAML_FLOAT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(YAML_FLOAT_PATTERN).expect("валидный regex дробного"));
static WINDOWS_DRIVE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(WINDOWS_DRIVE_PATTERN).expect("валидный regex диска"));
static COLOR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(COLOR_PATTERN).expect("валидный regex цвета"));

const LEGACY_PERMISSIONS: &[&str] = &[
    "links:open",
    "network:fetch",
    "network:dns",
    "network:ping",
    "network:listen",
    "launcher:servers",
    "launcher:dialogs",
    "storage:workspace",
    "storage:database",
    "files:instance",
    "files:plugin",
    "ui:compose",
    "events:launcher",
];
const V3_EXTRA_PERMISSIONS: &[&str] = &[
    "runtime:observe",
    "lifecycle:observe",
    "process:observe",
    "files:launcher-root-read",
    "files:launcher-config-projected",
    "network:http",
    "network:https",
    "network:websocket",
    "network:tcp-client",
    "network:udp-client",
    "network:proxy-control",
    "network:traffic-observe",
    "network:traffic-intercept",
    "network:tunnel",
    "os:open-url",
    "os:open-file",
    "os:reveal-in-file-manager",
    "os:clipboard-read",
    "os:clipboard-write",
    "os:spawn-external",
    "os:browser-session",
    "auth:oauth",
    "auth:token-use",
    "secrets:store",
    "secrets:use",
    "ai:invoke-cloud",
    "ai:invoke-local",
    "downloads:manage",
    "updates:manage",
    "dependencies:resolve",
    "integrity:scan",
    "metrics:emit",
    "logs:write",
    "presence:discord",
    "scheduler:jobs",
    "commands:register",
    "shortcuts:register",
    "instances:read",
    "instances:mutate",
    "mods:read",
    "mods:install",
    "packs:manage",
    "accounts:projected-read",
    "backup:export",
    "backup:restore",
    "sync:cloud",
    "ipc:sidecar",
    "policy:read",
    "crypto:sign",
    "crypto:verify",
];

fn permission_set(permissions: &[&'static str]) -> HashSet<&'static str> {
    permissions.iter().copied().collect()
}

static PLUGIN_PERMISSION_NAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut names = permission_set(LEGACY_PERMISSIONS);
    names.extend(V3_EXTRA_PERMISSIONS.iter().copied());
    names
});
static PLUGIN_V2_PERMISSION_NAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut names = permission_set(LEGACY_PERMISSIONS);
    names.remove("network:listen");
    names
});
static PLUGIN_V3_PERMISSION_NAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut names = permission_set(LEGACY_PERMISSIONS);
    names.remove("network:listen");
    names.extend(V3_EXTRA_PERMISSIONS.iter().copied());
    names
});

pub(crate) const PLUGIN_V2_EVENTS: &[&str] = &[
    "instance.afterExit",
    "instance.beforeLaunch",
    "instance.crash",
    "instance.selected",
    "launcher.ready",
    "log.line",
    "mods.beforeInstall",
    "plugin.enabled",
    "account.changed",
    "settings.changed",
    "snapshot.created",
];

pub(crate) const PLUGIN_V3_EVENTS: &[&str] = &[
    "files.changed",
    "instance.afterExit",
    "instance.beforeLaunch",
    "instance.crash",
    "instance.hung",
    "instance.launch.requested",
    "instance.preflight.completed",
    "instance.preflight.started",
    "instance.ready",
    "instance.selected",
    "instance.starting",
    "launcher.ready",
    "launcher.shuttingDown",
    "log.line",
    "mods.beforeInstall",
    "network.proxy.connected",
    "network.proxy.failed",
    "plugin.enabled",
    "process.memory.thresholdExceeded",
    "account.changed",
    "settings.changed",
    "snapshot.created",
];

const RUNTIME_CAPABILITIES: &[&str] = &[
    "runtime.capabilityNegotiation",
    "runtime.quotaTelemetry",
    "files.launcherRootReadonly",
    "config.projectedLauncherRead",
    "network.http",
    "network.proxyPolicies",
    "network.trafficPolicies",
    "ui.composition.v2",
    "ui.composition.extended",
    "ui.fragments",
    "ui.dataBindings",
    "runtime.operationLimits",
    "runtime.operationAudit",
    "events.listenerDebug",
    "lifecycle.processMetrics",
];

const V2_UI_SLOTS: &[&str] = &["sidebar_tab", "instance_tab", "settings_section", "home_widget"];
const V2_UI_TONES: &[&str] = &["default", "muted", "success", "warning", "danger"];
const V2_UI_DIRECTIONS: &[&str] = &["vertical", "horizontal"];

const V2_FIELDS: &[&str] = &[
    "id", "name", "version", "api_version", "entrypoint", "author", "description", "icon", "category",
    "permissions", "network_hosts", "config", "ui", "i18n", "theme", "actions", "events", "commands",
    "shortcuts", "integrity", "signature",
];
const V3_EXTRA_FIELDS: &[&str] = &[
    "capabilities",
    "consent_scopes",
    "extension_points",
    "dependency_graph",
    "update_channel",
    "sandbox_profile",
    "compatibility_matrix",
    "host_requirements",
];

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// сохраняет совместимое представление `null` и `bool` при преобразовании в строки
fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(number) => number.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// пустые значения и ноль считаются ложными
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|float| float != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn field_str(fields: &Map<String, Value>, key: &str, default: &str) -> String {
    match fields.get(key) {
        Some(value) => py_str(value),
        None => default.to_owned(),
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn char_len(value: &str) -> usize {
    value.chars().count()
}

/// лимит измеряется в кодовых точках, а не в байтах
fn text_fits(value: &str, limit: usize) -> bool {
    char_len(value) <= limit
}

fn py_printable(value: &str) -> bool {
    value.chars().all(|character| !character.is_control() && (character == ' ' || !character.is_whitespace()))
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

fn path_is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|metadata| metadata.is_symlink()).unwrap_or(false)
}

fn path_is_file(path: &Path) -> bool {
    fs::metadata(path).map(|metadata| metadata.is_file()).unwrap_or(false)
}

fn path_is_dir(path: &Path) -> bool {
    fs::metadata(path).map(|metadata| metadata.is_dir()).unwrap_or(false)
}

fn path_size(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn file_mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|moment| moment.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn restrict_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

fn create_dir_private(path: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(path)?;
    restrict_mode(path, 0o700);
    Ok(())
}

fn remove_tree(path: &Path) -> Result<(), std::io::Error> {
    if path_is_symlink(path) {
        return fs::remove_file(path);
    }
    fs::remove_dir_all(path)
}

fn within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn dedup_strings(values: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.as_str()) {
            out.push(value.clone());
        }
    }
    out
}

fn sorted_set(values: &[String]) -> Vec<String> {
    let unique: BTreeSet<&str> = values.iter().map(String::as_str).collect();
    unique.into_iter().map(str::to_owned).collect()
}

fn default_permission_record() -> Value {
    json!({"grants": [], "untrusted_execution_hash": ""})
}

pub(crate) struct PluginStore {
    pub(crate) root: PathBuf,
    pub(crate) trust_store: PluginTrustStore,
    permission_path: PathBuf,
    settings: Value,
    access_lock: Mutex<()>,
}

pub fn plugin_list(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    Ok(json!({ "plugins": PluginStore::new(&config)?.list()? }))
}

fn discover_plugins_inner(root: &Path, trust_store: &PluginTrustStore, permission_state: &Value) -> Result<Vec<Value>, String> {
    if path_is_symlink(root) {
        return Err("Каталог плагинов является символьной ссылкой".to_owned());
    }
    create_dir_private(root).map_err(|error| error.to_string())?;
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        names.push((entry.file_name().to_string_lossy().to_lowercase(), entry.path()));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));

    let mut result: Vec<Value> = Vec::new();
    for (_, directory) in names.iter().take(256) {
        if path_is_symlink(directory) || !path_is_dir(directory) {
            continue;
        }
        let directory_name = directory.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let legacy_path = directory.join(PLUGIN_MANIFEST);
        let v2_path = directory.join(PLUGIN_V2_MANIFEST);
        let has_legacy = path_is_file(&legacy_path) && !path_is_symlink(&legacy_path);
        let has_v2 = path_is_file(&v2_path) && !path_is_symlink(&v2_path);
        let static_json = if has_v2 { manifest::static_json_manifest(directory)? } else { None };
        if let Some(static_json) = &static_json {
            if static_json.get("id").and_then(Value::as_str) == Some(directory_name.as_str()) {
                result.push(static_json.clone());
                continue;
            }
        }
        if has_v2 || path_exists(&v2_path) || path_is_symlink(&v2_path) {
            result.push(v2_plugin_status(directory, trust_store, permission_state));
            continue;
        }
        if !has_legacy {
            continue;
        }
        // некорректные манифесты пропускаются
        let outcome = (|| -> Result<Value, String> {
            if path_size(&legacy_path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64 {
                return Err(String::new());
            }
            let text = fs::read_to_string(&legacy_path).map_err(|error| error.to_string())?;
            validate_manifest(&Value::Object(parse_plugin_yml(&text)?))
        })();
        let Ok(manifest) = outcome else { continue };
        if manifest.get("id").and_then(Value::as_str) != Some(directory_name.as_str()) {
            continue;
        }
        if has_legacy && path_exists(&v2_path) {
            continue;
        }
        let plugin_id = manifest.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut fields = manifest.as_object().cloned().unwrap_or_default();
        fields.insert("trust".to_owned(), json!("local"));
        fields.insert("installed".to_owned(), json!(true));
        fields.insert("enabled".to_owned(), json!(enabled_state(directory)));
        fields.insert(
            "revoked".to_owned(),
            json!(trust_store.is_plugin_revoked(&plugin_id)?),
        );
        fields.insert(
            "revocation_reason".to_owned(),
            trust_store.plugin_revocation_reason(&plugin_id)?.map_or(Value::Null, Value::String),
        );
        result.push(Value::Object(fields));
    }
    Ok(result)
}

pub(crate) fn parse_plugin_yml(text: &str) -> Result<Map<String, Value>, String> {
    manifest::parse_plugin_yml(text)
}

pub(crate) fn validate_manifest(value: &Value) -> Result<Value, String> {
    manifest::validate_manifest(value)
}

pub(crate) fn validate_plugin_json_manifest(value: &Value) -> Result<Value, String> {
    manifest::validate_plugin_json_manifest(value)
}

#[allow(dead_code)] // публичный API проверки манифестов
pub(crate) fn validate_v2_manifest(value: &Value) -> Result<Value, String> {
    manifest::validate_v2_manifest(value)
}

#[allow(dead_code)] // публичный API проверки манифестов
pub(crate) fn validate_v3_manifest(value: &Value) -> Result<Value, String> {
    manifest::validate_v3_manifest(value)
}

fn open_private_file(path: &Path) -> Result<File, std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        restrict_mode(path, 0o600);
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        fs::OpenOptions::new().write(true).create_new(true).open(path)
    }
}

pub(crate) fn parse_v2_manifest_bytes(content: &[u8]) -> Result<Value, String> {
    if content.len() > PLUGIN_MAX_MANIFEST_BYTES {
        return Err("plugin.json exceeds the manifest size limit".to_owned());
    }
    let value = parse_strict_json(content, "plugin.json")
        .map_err(|message| format!("plugin.json is not valid UTF-8 JSON: {message}"))?;
    if !value.is_object() {
        return Err("plugin.json must contain an object".to_owned());
    }
    Ok(value)
}

fn try_v2_manifest(root: &Path) -> Option<Value> {
    let path = root.join(PLUGIN_V2_MANIFEST);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > PLUGIN_MAX_MANIFEST_BYTES as u64 {
        return None;
    }
    let bytes = read_regular_package_file(&path, PLUGIN_MAX_MANIFEST_BYTES).ok()?;
    validate_plugin_json_manifest(&parse_v2_manifest_bytes(&bytes).ok()?).ok()
}

fn runtime_capabilities_for_manifest(manifest: &Value) -> Vec<String> {
    let mut capabilities: BTreeSet<String> = RUNTIME_CAPABILITIES.iter().map(|item| item.to_string()).collect();
    let permissions: HashSet<&str> = manifest
        .get("permissions")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if manifest.get("api_version").and_then(Value::as_str) == Some("3") {
        capabilities.insert("runtime.manifest.v3".to_owned());
    }
    if permissions.contains("files:launcher-root-read") {
        capabilities.insert("files.launcherRootReadonly.read".to_owned());
    }
    if permissions.contains("files:launcher-config-projected") {
        capabilities.insert("config.projectedLauncherRead.schemas".to_owned());
    }
    if permissions.contains("network:proxy-control") || permissions.contains("network:tunnel") {
        capabilities.insert("network.proxyPolicies.attach".to_owned());
    }
    if permissions.contains("network:traffic-observe") {
        capabilities.insert("network.trafficObserve".to_owned());
    }
    if permissions.contains("network:traffic-intercept") {
        capabilities.insert("network.trafficIntercept.experimental".to_owned());
    }
    capabilities.into_iter().collect()
}

fn v2_plugin_status(directory: &Path, trust_store: &PluginTrustStore, permission_state: &Value) -> Value {
    let plugin_id = directory
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let fallback = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "",
        "author": "",
        "description": "",
        "icon": "plug",
        "category": "Local",
        "api_version": "2",
        "entrypoint": "",
        "permissions": [],
        "network_hosts": [],
    });
    let mut package_error = String::new();
    let package: Option<V2Package>;
    let manifest: Value;
    let outcome = || -> Result<V2Package, String> {
        if !PLUGIN_ID_RE.is_match(&plugin_id) {
            return Err("Plugin directory has an unsafe id".to_owned());
        }
        read_v2_package(directory, Some(&plugin_id))
    }();
    match outcome {
        Ok(parsed) => {
            manifest = parsed.manifest.clone();
            package = Some(parsed);
        }
        Err(message) => {
            package_error = truncate_chars(&message, 500);
            manifest = try_v2_manifest(directory).unwrap_or(fallback);
            package = None;
        }
    }

    let signature = manifest.get("signature").filter(|value| value.is_object()).cloned();
    let signed = package
        .as_ref()
        .map(|item| item.signed)
        .unwrap_or_else(|| signature.is_some());
    let signature_fields = signature.as_ref().and_then(Value::as_object).cloned().unwrap_or_default();
    let key_id: Option<String> = package
        .as_ref()
        .map(|item| item.key_id.clone())
        .unwrap_or_else(|| signature_fields.get("key_id").and_then(Value::as_str).map(str::to_owned));
    let public_key: Option<String> = package
        .as_ref()
        .map(|item| item.public_key.clone())
        .unwrap_or_else(|| signature_fields.get("public_key").and_then(Value::as_str).map(str::to_owned));
    let fingerprint: Option<String> = package.as_ref().and_then(|item| item.fingerprint.clone());

    let mut publisher_status = json!({"pinned": false, "trusted": false, "revoked": false, "reason": null});
    if signed {
        if let (Some(key), Some(key_value)) = (&key_id, &public_key) {
            if let Ok(status) = trust_store.publisher_status(key, Some(&json!(key_value))) {
                publisher_status = status;
            }
        }
    }
    let package_valid = package.is_some();
    let publisher_trusted = package_valid && publisher_status.get("trusted").and_then(Value::as_bool).unwrap_or(false);
    let plugin_revoked =
        PLUGIN_ID_RE.is_match(&plugin_id) && trust_store.is_plugin_revoked(&plugin_id).unwrap_or(false);
    let revoked = plugin_revoked || publisher_status.get("revoked").and_then(Value::as_bool).unwrap_or(false);
    let record = permission_state
        .pointer("/plugins")
        .and_then(Value::as_object)
        .and_then(|fields| fields.get(plugin_id.as_str()))
        .cloned()
        .unwrap_or_else(default_permission_record);
    let declared: Vec<String> = if package_valid {
        manifest
            .get("permissions")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let stored_grants: Vec<String> = record
        .get("grants")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let granted = sorted_set(&declared.iter().filter(|item| stored_grants.contains(item)).cloned().collect::<Vec<_>>());
    let stored_hash = record
        .get("untrusted_execution_hash")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let local_execution_allowed = package_valid
        && !signed
        && package
            .as_ref()
            .map(|item| stored_hash == item.package_digest)
            .unwrap_or(false);
    let enabled = enabled_state(directory);
    let execution_allowed =
        package_valid && enabled && !revoked && (if signed { publisher_trusted } else { local_execution_allowed });
    let trust = if !package_valid {
        if signed { "community" } else { "local" }
    } else if signed {
        if publisher_trusted { "verified" } else { "community" }
    } else {
        "local"
    };
    let revocation_reason: Value = if PLUGIN_ID_RE.is_match(&plugin_id) && plugin_revoked {
        trust_store
            .plugin_revocation_reason(&plugin_id)
            .ok()
            .flatten()
            .map_or(Value::Null, Value::String)
    } else {
        publisher_status.get("reason").cloned().unwrap_or(Value::Null)
    };

    let mut fields = manifest.as_object().cloned().unwrap_or_default();
    fields.insert("trust".to_owned(), json!(trust));
    fields.insert("installed".to_owned(), json!(true));
    fields.insert("enabled".to_owned(), json!(enabled));
    fields.insert("signed".to_owned(), json!(signed));
    fields.insert("signature_valid".to_owned(), json!(package_valid && signed));
    fields.insert(
        "publisher_key_id".to_owned(),
        key_id.clone().map_or(Value::Null, Value::String),
    );
    fields.insert(
        "publisher_public_key".to_owned(),
        public_key.clone().map_or(Value::Null, Value::String),
    );
    fields.insert("publisher_fingerprint".to_owned(), fingerprint.map_or(Value::Null, Value::String));
    fields.insert(
        "publisher_pinned".to_owned(),
        json!(publisher_status.get("pinned").and_then(Value::as_bool).unwrap_or(false)),
    );
    fields.insert(
        "publisher_revocation_authority".to_owned(),
        json!(publisher_status.get("revocation_authority").and_then(Value::as_bool).unwrap_or(false)),
    );
    fields.insert("publisher_trusted".to_owned(), json!(publisher_trusted));
    fields.insert("package_valid".to_owned(), json!(package_valid));
    fields.insert(
        "package_error".to_owned(),
        if package_error.is_empty() { Value::Null } else { json!(package_error) },
    );
    fields.insert("revoked".to_owned(), json!(revoked));
    fields.insert("revocation_reason".to_owned(), revocation_reason);
    fields.insert(
        "requested_permissions".to_owned(),
        manifest.get("permissions").cloned().unwrap_or_else(|| json!([])),
    );
    fields.insert("granted_permissions".to_owned(), json!(granted));
    fields.insert("local_untrusted_execution_allowed".to_owned(), json!(local_execution_allowed));
    fields.insert("execution_allowed".to_owned(), json!(execution_allowed));
    fields.insert(
        "execution_blocked_reason".to_owned(),
        json!(execution_block_reason(
            package_valid,
            enabled,
            revoked,
            signed,
            publisher_trusted,
            local_execution_allowed
        )),
    );
    Value::Object(fields)
}

fn execution_block_reason(
    package_valid: bool,
    enabled: bool,
    revoked: bool,
    signed: bool,
    publisher_trusted: bool,
    local_execution_allowed: bool,
) -> Option<&'static str> {
    if !package_valid {
        return Some("invalid-package");
    }
    if revoked {
        return Some("revoked");
    }
    if signed && !publisher_trusted {
        return Some("publisher-not-trusted");
    }
    if !signed && !local_execution_allowed {
        return Some("local-untrusted-execution-not-approved");
    }
    if !enabled {
        return Some("disabled");
    }
    None
}

fn enabled_state(directory: &Path) -> bool {
    let path = directory.join(".enabled");
    path_exists(&path) && !path_is_symlink(&path)
}

fn write_enabled_marker(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(path)?;
        restrict_mode(path, 0o600);
    }
    #[cfg(not(unix))]
    {
        fs::OpenOptions::new().write(true).create(true).open(path)?;
    }
    Ok(())
}

/// при каталоге назначения включает имя источника в конечный путь
fn move_path(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    if fs::metadata(destination).map(|metadata| metadata.is_dir()).unwrap_or(false) {
        if let Some(name) = source.file_name() {
            return fs::rename(source, destination.join(name));
        }
    }
    fs::rename(source, destination)
}

/// не проходит по символьным ссылкам
fn collect_regular_files(directory: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = fs::read_dir(directory).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path_is_symlink(&path) {
            continue;
        }
        if path_is_dir(&path) {
            collect_regular_files(&path, out)?;
        } else if path_is_file(&path) {
            out.push((path.to_string_lossy().to_lowercase(), path));
        }
    }
    Ok(())
}

fn directory_usage(path: &Path) -> Result<Value, String> {
    if !path_exists(path) {
        return Ok(json!({"bytes": 0, "files": 0}));
    }
    if path_is_symlink(path) {
        return Err("Каталог данных плагина является символьной ссылкой".to_owned());
    }
    let mut total_bytes: u64 = 0;
    let mut file_count: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let child = entry.path();
            if path_is_symlink(&child) || !path_is_file(&child) {
                continue;
            }
            if let Ok(metadata) = fs::metadata(&child) {
                total_bytes += metadata.len();
                file_count += 1;
            }
        }
        for entry in fs::read_dir(&current).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let child = entry.path();
            if !path_is_symlink(&child) && path_is_dir(&child) {
                stack.push(child);
            }
        }
    }
    Ok(json!({"bytes": total_bytes, "files": file_count}))
}

fn safe_child(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let normalized = relative_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|part| !part.is_empty()).collect();
    if normalized.starts_with('/') || parts.is_empty() || parts.iter().any(|part| *part == "." || *part == "..") {
        return Err("Небезопасный путь файла плагина".to_owned());
    }
    let mut candidate = root.to_path_buf();
    for part in &parts {
        candidate.push(part);
        if path_is_symlink(&candidate) {
            return Err("Символьные ссылки в файлах плагина запрещены".to_owned());
        }
    }
    // проверка компонентов на символьные ссылки позволяет нормализовать путь лексически
    let root_resolved = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let target = root_resolved.join(parts.join("/"));
    if !within(&target, &root_resolved) {
        return Err("Путь файла плагина выходит за пределы namespace".to_owned());
    }
    Ok(target)
}

fn archive_member_path(root: &Path, member_name: &str) -> Result<PathBuf, InstallFail> {
    let normalized = member_name.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|part| !part.is_empty()).collect();
    if normalized.starts_with('/') || parts.is_empty() || parts.iter().any(|part| *part == "." || *part == "..") {
        return Err(InstallFail::User("Архив плагина содержит небезопасный путь".to_owned()));
    }
    let candidate = root.join(parts.join("/"));
    // каталог установки ещё не создан, поэтому путь нормализуется лексически
    let root_resolved = resolve_non_strict(root);
    let parent_resolved = resolve_non_strict(candidate.parent().unwrap_or(root));
    if !within(&parent_resolved, &root_resolved) {
        return Err(InstallFail::User(
            "Архив плагина выходит за пределы staging-каталога".to_owned(),
        ));
    }
    Ok(candidate)
}

fn resolve_non_strict(path: &Path) -> PathBuf {
    if let Ok(resolved) = fs::canonicalize(path) {
        return resolved;
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => {
                let candidate = resolved.join(part);
                resolved = fs::canonicalize(&candidate).unwrap_or(candidate);
            }
        }
    }
    resolved
}

fn assert_no_symlink_chain(path: &Path, root: &Path) -> Result<(), String> {
    let root_resolved = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut current = path.to_path_buf();
    loop {
        if path_is_symlink(&current) {
            return Err("Символьные ссылки в файлах плагина запрещены".to_owned());
        }
        if current == root_resolved {
            return Ok(());
        }
        if current.parent().is_none() || !within(&current, &root_resolved) {
            return Err("Путь файла плагина выходит за пределы namespace".to_owned());
        }
        current = current.parent().expect("проверено выше").to_path_buf();
    }
}

fn validate_config_key(key: &str) -> Result<(), String> {
    if !CONFIG_KEY_RE.is_match(key.trim()) {
        return Err("Небезопасный ключ конфигурации плагина".to_owned());
    }
    Ok(())
}

fn dump_yaml(value: &Map<String, Value>) -> Result<String, String> {
    let mut lines: Vec<String> = Vec::new();
    for (key, item) in value {
        validate_config_key(key)?;
        dump_yaml_value(&mut lines, key, item, 0)?;
    }
    if lines.is_empty() {
        return Ok(String::new());
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn dump_yaml_value(lines: &mut Vec<String>, key: &str, value: &Value, indent: usize) -> Result<(), String> {
    let prefix = " ".repeat(indent);
    match value {
        Value::Object(fields) => {
            lines.push(format!("{prefix}{key}:"));
            for (child_key, child_value) in fields {
                validate_config_key(child_key)?;
                dump_yaml_value(lines, child_key, child_value, indent + 2)?;
            }
        }
        Value::Array(items) => {
            lines.push(format!("{prefix}{key}:"));
            for item in items {
                lines.push(format!("{}- {}", " ".repeat(indent + 2), yaml_dump_scalar(item)));
            }
        }
        other => lines.push(format!("{prefix}{key}: {}", yaml_dump_scalar(other))),
    }
    Ok(())
}

fn yaml_dump_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => if *flag { "true" } else { "false" }.to_owned(),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                integer.to_string()
            } else if let Some(float) = number.as_f64() {
                // сохраняет `.0` у целых значений `f64`
                if float.fract() == 0.0 && float.is_finite() && float.abs() < 1e16 {
                    format!("{float:.1}")
                } else {
                    float.to_string()
                }
            } else {
                number.to_string()
            }
        }
        other => serde_json::to_string(other).unwrap_or_else(|_| "\"\"".to_owned()),
    }
}

enum InstallFail {
    /// ошибка для пользователя с исходным текстом
    User(String),
    /// системная ошибка с префиксом установки
    Os(String),
}

impl From<String> for InstallFail {
    fn from(message: String) -> Self {
        InstallFail::User(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::permission_state::validate_permission_state;
    use std::collections::BTreeMap;

    use base64::Engine as _;
    use ed25519_dalek::SigningKey;

    fn temp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("nivora-plugins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).expect("создать временный каталог");
        home
    }

    fn test_store(tag: &str) -> PluginStore {
        let home = temp_home(tag);
        let data_home = home.join("data");
        create_dir_private(&data_home).unwrap();
        let root = data_home.join("plugins");
        create_dir_private(&root).unwrap();
        PluginStore {
            root,
            trust_store: PluginTrustStore::new(&data_home.join("plugin-trust")).unwrap(),
            permission_path: data_home.join(PLUGIN_PERMISSION_FILE),
            settings: json!({"theme": "dark", "max_downloads": 4, "client_id": "secret"}),
            access_lock: Mutex::new(()),
        }
    }

    fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).expect("записать член архива");
            writer.write_all(data).expect("записать данные члена");
        }
        let cursor = writer.finish().expect("закрыть архив");
        cursor.into_inner()
    }

    fn unsigned_v2_manifest(id: &str, permissions: &[&str], files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut integrity = Map::new();
        for (path, content) in files {
            integrity.insert(path.to_string(), json!(sha256_hex(content)));
        }
        json!({
            "id": id,
            "name": "Demo Plugin",
            "version": "1.0.0",
            "api_version": "2",
            "entrypoint": "main.js",
            "permissions": permissions,
            "integrity": {"files": integrity},
        })
        .to_string()
        .into_bytes()
    }

    fn seed_pem(seed: [u8; 32]) -> String {
        let der: Vec<u8> = [
            0x30u8, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
        ]
        .iter()
        .copied()
        .chain(seed.iter().copied())
        .collect();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            base64::engine::general_purpose::STANDARD.encode(der)
        )
    }

    #[test]
    fn parse_plugin_yml_builds_nested_structure() {
        let text = "---\nid: demo-plugin\nname: Demo\nnetwork_hosts:\n  - Example.COM.\n  - example.com\nsidebar:\n  label: 'текст с ''кавычками'''\nflags:\n  yes: yes\n  count: 42\n  ratio: 2.5\n  empty: ~\ninline: [1, \"two\"]\n";
        let parsed = parse_plugin_yml(text).unwrap();
        assert_eq!(parsed["id"], json!("demo-plugin"));
        assert_eq!(parsed["network_hosts"], json!(["Example.COM.", "example.com"]));
        assert_eq!(parsed["sidebar"]["label"], json!("текст с 'кавычками'"));
        assert_eq!(parsed["flags"]["yes"], json!(true));
        assert_eq!(parsed["flags"]["count"], json!(42));
        assert_eq!(parsed["flags"]["ratio"], json!(2.5));
        assert_eq!(parsed["flags"]["empty"], json!(null));
        assert_eq!(parsed["inline"], json!([1, "two"]));
        // комментарии распознаются только вне кавычек
        let with_comment = parse_plugin_yml("id: demo # идентификатор\nname: 'Demo # не комментарий'\n").unwrap();
        assert_eq!(with_comment["id"], json!("demo"));
        assert_eq!(with_comment["name"], json!("Demo # не комментарий"));
    }

    #[test]
    fn parse_plugin_yml_rejects_malformed_input() {
        assert_eq!(
            parse_plugin_yml("id: demo\n\tname: x\n").unwrap_err(),
            "plugin.yml содержит tab на строке 2"
        );
        assert_eq!(
            parse_plugin_yml("id: |\n").unwrap_err(),
            "plugin.yml не поддерживает YAML tags, anchors и blocks"
        );
        assert_eq!(parse_plugin_yml("id: *anchor\n").unwrap_err(), "plugin.yml не поддерживает YAML tags, anchors и blocks");
        assert_eq!(
            parse_plugin_yml("1bad: x\n").unwrap_err(),
            "Небезопасный ключ plugin.yml: 1bad"
        );
        assert_eq!(parse_plugin_yml("just text\n").unwrap_err(), "Некорректная строка plugin.yml: 1");
        assert_eq!(
            parse_plugin_yml("- item\n").unwrap_err(),
            "Список plugin.yml находится не под ключом на строке 1"
        );
        let too_large = format!("id: {}\n", "a".repeat(PLUGIN_MAX_MANIFEST_BYTES));
        assert_eq!(parse_plugin_yml(&too_large).unwrap_err(), "plugin.yml слишком большой");
    }

    #[test]
    fn validate_manifest_normalizes_legacy_manifest() {
        let value = json!({
            "id": "Demo-Plugin",
            "name": "Демо",
            "version": "1.0",
            "entrypoint": "scripts\\main.js",
            "permissions": ["UI:Compose", "ui:compose", "links:open"],
            "network_hosts": ["Example.COM.", "example.com", "example.com"],
            "author": "Автор",
            "featured": "yes",
        });
        let manifest = validate_manifest(&value).unwrap();
        assert_eq!(manifest["id"], json!("demo-plugin"));
        assert_eq!(manifest["name"], json!("Демо"));
        assert_eq!(manifest["author"], json!("Автор"));
        assert_eq!(manifest["entrypoint"], json!("scripts/main.js"));
        assert_eq!(manifest["permissions"], json!(["ui:compose", "links:open"]));
        assert_eq!(manifest["network_hosts"], json!(["example.com"]));
        assert_eq!(manifest["featured"], json!(true));
        assert_eq!(manifest["icon"], json!("plug"));
        assert_eq!(manifest["category"], json!("Local"));
        assert_eq!(manifest["api_version"], json!("1"));
    }

    #[test]
    fn validate_manifest_rejects_insecure_values() {
        let cases: Vec<(Value, &str)> = vec![
            (json!({"id": "1demo", "name": "N", "version": "1"}), "plugin.yml содержит небезопасный id"),
            (json!({"id": "demo", "version": "1"}), "plugin.yml требует name и version"),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "/abs/main.js"}),
                "entrypoint должен быть относительным .js-файлом",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "../up.js"}),
                "entrypoint должен быть относительным .js-файлом",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.txt"}),
                "entrypoint должен быть относительным .js-файлом",
            ),
            // порядок проверок сохраняет ошибку entrypoint при нескольких нарушениях
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.js", "permissions": "all"}),
                "permissions в plugin.yml должен быть списком",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.js", "permissions": ["nope:all"]}),
                "plugin.yml содержит неизвестное permission",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.js", "network_hosts": ["10.0.0.1"]}),
                "network_hosts не может содержать IP-адреса",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.js", "network_hosts": ["bad_host"]}),
                "network_hosts должен содержать только имена хостов",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.js", "sidebar_tab": {"label": "X", "extra": 1}}),
                "sidebar_tab должен содержать только поле label",
            ),
            (
                json!({"id": "demo", "name": "N", "version": "1", "entrypoint": "main.js", "sidebar_tab": {"label": "Таб"}}),
                "sidebar_tab требует permission ui:compose",
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(validate_manifest(&value).unwrap_err(), expected, "значение: {value}");
        }
    }

    #[test]
    fn validate_v2_manifest_accepts_minimal_unsigned_package() {
        let value = json!({
            "id": "demo-plugin",
            "name": "Demo",
            "version": "1.0.0",
            "api_version": "2",
            "entrypoint": "main.js",
            "permissions": ["storage:workspace"],
            "integrity": {"files": {"main.js": "a".repeat(64)}},
        });
        let manifest = validate_plugin_json_manifest(&value).unwrap();
        assert_eq!(manifest["api_version"], json!("2"));
        assert_eq!(manifest["network_hosts"], json!([]));
        // v2 не выдаёт `network:listen`, но общий набор разрешений сохраняет его
        assert!(!PLUGIN_V2_PERMISSION_NAMES.contains("network:listen"));
        assert!(PLUGIN_PERMISSION_NAMES.contains("network:listen"));
        assert!(PLUGIN_V3_PERMISSION_NAMES.contains("crypto:sign"));
        assert!(PLUGIN_V3_EVENTS.len() > PLUGIN_V2_EVENTS.len());
    }

    #[test]
    #[ignore = "сверить ожидаемые результаты"]
    fn validate_v2_manifest_rejects_schema_violations() {
        let base = json!({
            "id": "demo-plugin",
            "name": "Demo",
            "version": "1.0.0",
            "api_version": "2",
            "entrypoint": "main.js",
            "permissions": [],
            "integrity": {"files": {}},
        });
        let broken = |mut value: Value, patch: Value| {
            let fields = value.as_object_mut().unwrap();
            match patch {
                Value::Object(entries) => {
                    for (key, item) in entries {
                        if item.is_null() {
                            fields.remove(&key);
                        } else {
                            fields.insert(key, item);
                        }
                    }
                }
                _ => unreachable!(),
            }
            value
        };
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"oops": 1}))).unwrap_err(),
            "plugin.json contains unsupported field(s): oops"
        );
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"integrity": null}))).unwrap_err(),
            "plugin.json is missing a required v2 field"
        );
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"api_version": "3"}))).unwrap_err(),
            "plugin.json api_version must be \"2\""
        );
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"permissions": ["crypto:sign"]}))).unwrap_err(),
            "plugin.json contains an unknown permission"
        );
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"events": ["nope.event"]}))).unwrap_err(),
            "plugin.json.events contains an unsupported launcher event"
        );
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"entrypoint": "../up.js"}))).unwrap_err(),
            "Plugin package contains an unsafe path"
        );
        assert_eq!(
            validate_v2_manifest(&broken(base.clone(), json!({"integrity": {"files": {}, "extra": 1}}))).unwrap_err(),
            "plugin.json.integrity has an invalid object shape"
        );
        assert_eq!(
            validate_plugin_json_manifest(&json!({"api_version": "9"})).unwrap_err(),
            "plugin.json must contain a JSON object"
        );
        let mut v3 = base.clone();
        v3["api_version"] = json!("3");
        assert_eq!(
            validate_plugin_json_manifest(&v3).unwrap_err(),
            "plugin.json api_version must be \"3\""
        );
    }

    #[test]
    fn validate_v2_ui_rejects_unknown_slot_and_deep_trees() {
        let ui_view = |slot: &str| {
            json!({
                "views": [{
                    "id": "view-1", "slot": slot, "title": "Заголовок",
                    "root": {"type": "text", "text": "привет"},
                }]
            })
        };
        let mut base = json!({
            "id": "demo-plugin", "name": "Demo", "version": "1.0.0", "api_version": "2",
            "entrypoint": "main.js", "permissions": [], "integrity": {"files": {}},
        });
        base["ui"] = ui_view("sidebar_tab");
        assert!(validate_v2_manifest(&base).is_ok());
        base["ui"] = ui_view("modal");
        assert_eq!(
            validate_v2_manifest(&base).unwrap_err(),
            "plugin.json.ui.views[].slot is unsupported"
        );

        // ограничивает вложенность до десяти уровней
        let mut node = json!({"type": "text", "text": "x"});
        for _ in 0..12 {
            node = json!({"type": "card", "children": [node]});
        }
        let mut deep = json!({
            "id": "demo-plugin", "name": "Demo", "version": "1.0.0", "api_version": "2",
            "entrypoint": "main.js", "permissions": [], "integrity": {"files": {}},
        });
        deep["ui"] = json!({"views": [{"id": "v", "slot": "sidebar_tab", "title": "T", "root": node}]});
        assert_eq!(
            validate_v2_manifest(&deep).unwrap_err(),
            "plugin.json UI tree exceeds its depth or node limit"
        );
    }

    #[test]
    fn legacy_plugin_lifecycle_in_store() {
        let store = test_store("legacy");
        let directory = store.root.join("demo-plugin");
        create_dir_private(&directory).unwrap();
        let manifest = "id: demo-plugin\nname: Demo\nversion: '1.0'\nentrypoint: main.js\npermissions:\n  - links:open\n";
        fs::write(directory.join(PLUGIN_MANIFEST), manifest).unwrap();
        fs::write(directory.join("main.js"), "console.log(1)").unwrap();

        let plugins = store.list().unwrap();
        assert_eq!(plugins.len(), 1);
        let plugin = &plugins[0];
        assert_eq!(plugin["id"], json!("demo-plugin"));
        assert_eq!(plugin["trust"], json!("local"));
        assert_eq!(plugin["installed"], json!(true));
        assert_eq!(plugin["enabled"], json!(false));
        assert_eq!(plugin["revoked"], json!(false));

        assert!(!store.is_enabled("demo-plugin").unwrap());
        let enabled = store.set_enabled("demo-plugin", true).unwrap();
        assert_eq!(enabled["enabled"], json!(true));
        assert!(store.is_enabled("demo-plugin").unwrap());
        assert_eq!(store.manifest("demo-plugin").unwrap()["entrypoint"], json!("main.js"));

        assert!(store.set_enabled("other-plugin", true).is_err());
        assert_eq!(
            store.authorize_operation("demo-plugin", "links:open").unwrap()["id"],
            json!("demo-plugin")
        );
        assert_eq!(
            store.authorize_operation("demo-plugin", "storage:workspace").unwrap_err(),
            "Плагину не выдано разрешение storage:workspace"
        );

        store.uninstall("demo-plugin").unwrap();
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(temp_home("legacy"));
    }

    #[test]
    #[ignore = "сверить ожидаемые результаты"]
    fn storage_config_and_files_namespaces() {
        let store = test_store("namespace");
        store.write_storage("demo-plugin", "routes", &json!({"a": [1, 2], "имя": "тест"})).unwrap();
        assert_eq!(
            store.read_storage("demo-plugin", "routes").unwrap(),
            json!({"a": [1, 2], "имя": "тест"})
        );
        assert_eq!(store.read_storage("demo-plugin", "missing").unwrap(), json!(null));
        assert!(store.write_storage("demo-plugin", "плохой ключ", &json!(1)).is_err());
        store.remove_storage("demo-plugin", "routes").unwrap();
        assert_eq!(store.read_storage("demo-plugin", "routes").unwrap(), json!(null));

        store.config_set("demo-plugin", "limit", &json!(7)).unwrap();
        store.config_set("demo-plugin", "nested.enabled", &json!(true)).unwrap();
        store.config_set("demo-plugin", "items", &json!(["a", "b"])).unwrap();
        assert_eq!(store.config_get("demo-plugin", "limit").unwrap(), json!(7));
        assert_eq!(store.config_get("demo-plugin", "nested.enabled").unwrap(), json!(true));
        assert_eq!(store.config_get("demo-plugin", "items").unwrap(), json!(["a", "b"]));
        store.config_remove("demo-plugin", "limit").unwrap();
        assert_eq!(store.config_get("demo-plugin", "limit").unwrap(), json!(null));

        store.write_file("demo-plugin", "data/notes.txt", "текст").unwrap();
        assert_eq!(store.list_files("demo-plugin", "").unwrap(), vec!["data/notes.txt"]);
        let info = store.list_file_info("demo-plugin", "").unwrap();
        assert_eq!(info[0]["path"], json!("data/notes.txt"));
        assert_eq!(info[0]["size"], json!("текст".len()));
        assert_eq!(store.read_file("demo-plugin", "data/notes.txt").unwrap(), "текст");
        assert!(store.write_file("demo-plugin", "../escape.txt", "x").is_err());
        store.remove_file("demo-plugin", "data/notes.txt").unwrap();
        assert!(store.list_files("demo-plugin", "").unwrap().is_empty());

        let quota = store.quota_usage("demo-plugin").unwrap();
        assert_eq!(quota["plugin_id"], json!("demo-plugin"));
        assert_eq!(quota["totals"]["bytes"], json!(0));
        assert!(quota["directories"]["storage"].is_object());
    }

    #[test]
    fn projected_config_slices_settings() {
        let store = test_store("projected");
        let projected = store.projected_launcher_config("launcher-public-config").unwrap();
        assert_eq!(projected["schema_id"], json!("launcher-public-config"));
        assert_eq!(projected["data"]["theme"], json!("dark"));
        assert!(projected["data"].get("client_id").is_none());
        assert!(projected["removed_fields"].as_array().unwrap().contains(&json!("client_id")));
        let diagnostics = store.projected_launcher_config("network-diagnostics").unwrap();
        assert_eq!(diagnostics["data"]["max_downloads"], json!(4));
        assert_eq!(
            store.projected_launcher_config("unknown").unwrap_err(),
            "Неизвестная схема projected config"
        );
    }

    #[test]
    fn permission_state_rejects_invalid_records() {
        let invalid = json!({
            "schema_version": 1,
            "plugins": {"demo-plugin": {"grants": ["nope:permission"], "untrusted_execution_hash": ""}},
        });
        assert_eq!(
            validate_permission_state(&invalid).unwrap_err(),
            "Plugin permission grants contain an invalid permission"
        );
        let duplicated = json!({
            "schema_version": 1,
            "plugins": {"demo-plugin": {"grants": ["links:open", "links:open"], "untrusted_execution_hash": ""}},
        });
        assert_eq!(
            validate_permission_state(&duplicated).unwrap_err(),
            "Plugin permission grants contain duplicate permissions"
        );
        let bad_hash = json!({
            "schema_version": 1,
            "plugins": {"demo-plugin": {"grants": [], "untrusted_execution_hash": "zz"}},
        });
        assert_eq!(
            validate_permission_state(&bad_hash).unwrap_err(),
            "Plugin permission grants contain an invalid local execution hash"
        );
        let bad_shape = json!({"schema_version": 1});
        assert_eq!(
            validate_permission_state(&bad_shape).unwrap_err(),
            "Plugin permission grants have an unsupported format"
        );
    }

    #[test]
    #[ignore = "сверить ожидаемые результаты"]
    fn install_unsigned_v2_archive_requires_local_consent() {
        let store = test_store("unsigned-v2");
        let main_js = b"console.log('demo')".to_vec();
        let files = [("main.js", main_js.clone())];
        let manifest_bytes = unsigned_v2_manifest("demo-plugin", &["storage:workspace", "links:open"], &files);
        let archive = zip_bytes(&[("plugin.json", manifest_bytes), ("main.js", main_js)]);

        let plugin = store.install_archive(&archive, false, false).unwrap();
        assert_eq!(plugin["id"], json!("demo-plugin"));
        assert_eq!(plugin["signed"], json!(false));
        assert_eq!(plugin["package_valid"], json!(true));
        assert_eq!(plugin["enabled"], json!(false));
        assert_eq!(plugin["execution_allowed"], json!(false));
        assert_eq!(plugin["execution_blocked_reason"], json!("local-untrusted-execution-not-approved"));

        assert_eq!(
            store.set_enabled("demo-plugin", true).unwrap_err(),
            "Для unsigned-плагина требуется явное согласие на локальный untrusted-запуск"
        );

        let approved = store.set_local_untrusted_execution("demo-plugin", true).unwrap();
        assert_eq!(approved["local_untrusted_execution_allowed"], json!(true));

        let enabled = store.set_enabled("demo-plugin", true).unwrap();
        assert_eq!(enabled["execution_allowed"], json!(true));
        assert_eq!(enabled["execution_blocked_reason"], json!(null));

        let source = store.runtime_source("demo-plugin").unwrap();
        assert_eq!(source["source"], json!(String::from_utf8(files[0].1.clone()).unwrap()));
        assert_eq!(source["entrypoint"], json!("main.js"));
        assert_eq!(source["package_sha256"], plugin["package_sha256"]);

        let info = store.runtime_info("demo-plugin").unwrap();
        assert_eq!(info["sandbox_profile"], json!("quickjs-legacy"));
        assert!(info["capabilities"].as_array().unwrap().iter().any(|item| item == &json!("ui.composition.v2")));
        // список не включает хеш; он доступен только через API выполнения
        assert_eq!(source["package_sha256"], info["package_sha256"]);

        let grants = store.set_permission_grants("demo-plugin", &json!(["storage:workspace"])).unwrap();
        assert_eq!(grants["granted_permissions"], json!(["storage:workspace"]));
        assert!(store.set_permission_grants("demo-plugin", &json!(["crypto:sign"])).is_err());
        assert!(store.set_permission_grants("demo-plugin", &json!(["links:open", "links:open"])).is_err());

        let summary = store.permission_summary("demo-plugin").unwrap();
        assert_eq!(summary["execution_allowed"], json!(true));
        assert_eq!(
            store.authorize_operation("demo-plugin", "links:open").unwrap_err(),
            "Пользователь не выдал плагину разрешение links:open"
        );
        assert!(store.authorize_operation("demo-plugin", "storage:workspace").is_ok());
    }

    #[test]
    fn plugin_api_permission_reduction_cannot_restore_revoked_grants() {
        let store = test_store("permission-reduction");
        let main_js = b"console.log('demo')".to_vec();
        let files = [("main.js", main_js.clone())];
        let manifest = unsigned_v2_manifest("demo-plugin", &["storage:workspace", "links:open"], &files);
        let archive = zip_bytes(&[("plugin.json", manifest), ("main.js", main_js)]);
        store.install_archive(&archive, false, false).unwrap();
        store
            .set_permission_grants("demo-plugin", &json!(["storage:workspace", "links:open"]))
            .unwrap();

        let plugin = store.reduce_permission_grants("demo-plugin", &json!(["storage:workspace"])).unwrap();
        assert_eq!(plugin["granted_permissions"], json!(["storage:workspace"]));
        assert!(store
            .reduce_permission_grants("demo-plugin", &json!(["storage:workspace", "links:open"]))
            .is_err());
        assert_eq!(store.permission_summary("demo-plugin").unwrap()["granted_permissions"], json!(["storage:workspace"]));
    }

    #[test]
    fn install_signed_v2_archive_requires_pinned_publisher() {
        let store = test_store("signed-v2");
        let seed: [u8; 32] = core::array::from_fn(|index| (index + 3) as u8);
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().to_bytes());

        let main_js = b"export default {}".to_vec();
        let mut package_files = BTreeMap::new();
        package_files.insert("main.js".to_owned(), main_js.clone());
        let unsigned = json!({
            "id": "signed-plugin",
            "name": "Signed",
            "version": "2.0",
            "api_version": "2",
            "entrypoint": "main.js",
            "permissions": ["links:open"],
            "signature": {"key_id": "author"},
        });
        let signed_manifest =
            super::super::plugin_security::sign_manifest(&unsigned, &package_files, seed_pem(seed).as_bytes()).unwrap();
        let archive = zip_bytes(&[
            ("plugin.json", signed_manifest.to_string().into_bytes()),
            ("main.js", main_js),
        ]);

        let plugin = store.install_archive(&archive, false, false).unwrap();
        assert_eq!(plugin["signed"], json!(true));
        assert_eq!(plugin["publisher_key_id"], json!("author"));
        assert_eq!(plugin["publisher_pinned"], json!(false));
        assert_eq!(plugin["execution_blocked_reason"], json!("publisher-not-trusted"));

        let status = store.pin_publisher("author", &public_key, false).unwrap();
        assert_eq!(status["publisher"]["pinned"], json!(true));

        let enabled = store.set_enabled("signed-plugin", true).unwrap();
        assert_eq!(enabled["execution_allowed"], json!(true));
        assert_eq!(enabled["trust"], json!("verified"));

        let tampered = zip_bytes(&[
            ("plugin.json", signed_manifest.to_string().into_bytes()),
            ("main.js", b"export default 'evil'".to_vec()),
        ]);
        let error = store.install_archive(&tampered, false, true).unwrap_err();
        assert!(error.contains("Plugin file hash mismatch"), "ошибка: {error}");
    }

    #[test]
    fn probe_reserved_path_error() {
        let store = test_store("probe-reserved");
        let main_js = b"console.log(1)".to_vec();
        let reserved_archive = zip_bytes(&[
            ("plugin.json", unsigned_v2_manifest("demo-plugin", &[], &[("main.js", main_js.clone())])),
            ("storage/keep.txt", b"x".to_vec()),
            ("main.js", main_js),
        ]);
        let error = store.install_archive(&reserved_archive, false, false).unwrap_err();
        eprintln!("PROBE reserved error: {error}");
        for entry in fs::read_dir(&store.root).unwrap() {
            eprintln!("PROBE root entry: {:?}", entry.unwrap().path());
        }
    }

    #[test]
    fn install_archive_rejects_broken_packages() {
        let store = test_store("broken");
        assert_eq!(
            store.install_archive(b"", false, false).unwrap_err(),
            "Архив плагина пустой или слишком большой"
        );
        let not_zip = b"this is not a zip archive at all".to_vec();
        assert!(store.install_archive(&not_zip, false, false).unwrap_err().starts_with("Не удалось установить плагин"));

        let main_js = b"console.log(1)".to_vec();
        let manifest_bytes = unsigned_v2_manifest("demo-plugin", &[], &[("main.js", main_js.clone())]);
        let both = zip_bytes(&[
            ("plugin.json", manifest_bytes.clone()),
            ("plugin.yml", b"id: demo-plugin\nname: D\nversion: 1\nentrypoint: main.js\n".to_vec()),
            ("main.js", main_js.clone()),
        ]);
        assert_eq!(
            store.install_archive(&both, false, false).unwrap_err(),
            "Архив должен содержать ровно один корневой plugin.yml или plugin.json"
        );

        let no_entrypoint = zip_bytes(&[("plugin.json", manifest_bytes)]);
        assert_eq!(
            store.install_archive(&no_entrypoint, false, false).unwrap_err(),
            "Plugin package is missing its entrypoint"
        );

        let reserved_archive = zip_bytes(&[
            ("plugin.json", unsigned_v2_manifest("demo-plugin", &[], &[("main.js", main_js.clone())])),
            ("storage/keep.txt", b"x".to_vec()),
            ("main.js", main_js),
        ]);
        assert_eq!(
            store.install_archive(&reserved_archive, false, false).unwrap_err(),
            "Plugin package uses a reserved runtime path: storage"
        );
    }

    #[test]
    fn replace_preserves_data_directories_and_narrows_grants() {
        let store = test_store("replace");
        let main_js = b"console.log(1)".to_vec();
        let archive = zip_bytes(&[
            ("plugin.json", unsigned_v2_manifest("demo-plugin", &["links:open", "storage:workspace"], &[("main.js", main_js.clone())])),
            ("main.js", main_js.clone()),
        ]);
        store.install_archive(&archive, false, false).unwrap();
        store.set_permission_grants("demo-plugin", &json!(["links:open", "storage:workspace"])).unwrap();
        store.write_storage("demo-plugin", "state", &json!({"keep": true})).unwrap();

        let next_js = b"console.log(2)".to_vec();
        let replacement = zip_bytes(&[
            ("plugin.json", unsigned_v2_manifest("demo-plugin", &["links:open"], &[("main.js", next_js.clone())])),
            ("main.js", next_js),
        ]);
        let replaced = store.install_archive(&replacement, false, true).unwrap();
        // при замене сохраняются только разрешения из нового манифеста
        assert_eq!(replaced["granted_permissions"], json!(["links:open"]));
        assert_eq!(store.read_storage("demo-plugin", "state").unwrap(), json!({"keep": true}));
        assert!(!store.root.join("demo-plugin").join(".install-").exists());

        assert_eq!(
            store.install_archive(&replacement, false, false).unwrap_err(),
            "Плагин уже установлен: demo-plugin"
        );
    }

    #[test]
    fn static_json_manifest_lists_without_status_fields() {
        let store = test_store("static");
        let directory = store.root.join("static-plugin");
        create_dir_private(&directory).unwrap();
        let manifest = json!({
            "id": "static-plugin",
            "name": "Static",
            "version": "1",
            "api_version": "1",
            "entrypoint": "main.js",
            "permissions": [],
        });
        fs::write(directory.join(PLUGIN_V2_MANIFEST), manifest.to_string()).unwrap();

        let plugins = store.list().unwrap();
        assert_eq!(plugins.len(), 1);
        // `api_version` 1 остаётся статическим контрактом без полей выполнения
        assert_eq!(plugins[0]["id"], json!("static-plugin"));
        assert!(plugins[0].get("installed").is_none());
        assert_eq!(store.manifest("static-plugin").unwrap()["name"], json!("Static"));
    }

    #[test]
    fn revocation_blocks_execution_and_enabled() {
        let store = test_store("revoked");
        let main_js = b"console.log(1)".to_vec();
        let archive = zip_bytes(&[
            ("plugin.json", unsigned_v2_manifest("demo-plugin", &[], &[("main.js", main_js.clone())])),
            ("main.js", main_js),
        ]);
        store.install_archive(&archive, false, false).unwrap();
        store.set_local_untrusted_execution("demo-plugin", true).unwrap();
        store.set_enabled("demo-plugin", true).unwrap();

        let revoked = store.revoke_plugin("demo-plugin", "вредоносный").unwrap();
        assert_eq!(revoked["revoked"], json!(true));
        assert_eq!(revoked["revocation_reason"], json!("вредоносный"));
        assert_eq!(revoked["execution_allowed"], json!(false));
        assert_eq!(
            store.set_enabled("demo-plugin", true).unwrap_err(),
            "Плагин отозван локально или списком отзыва"
        );
        assert_eq!(
            store.runtime_source("demo-plugin").unwrap_err(),
            "Плагин отозван локально или списком отзыва"
        );
        let restored = store.unrevoke_plugin("demo-plugin").unwrap();
        assert_eq!(restored["execution_allowed"], json!(true));
    }
}
