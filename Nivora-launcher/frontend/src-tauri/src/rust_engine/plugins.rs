// Плагины: инвентаризация, установка ZIP-архивов, манифесты plugin.yml (legacy)
// и plugin.json (v2/v3 с подписями), grants разрешений, namespace-хранилища
// (storage/config/files). database() и sqlite-authorizer из Python не портированы:
// SQLite-доступ плагинов — территория QuickJS-рантайма, не хранилища.
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use super::config::ConfigStore;
use super::mods::random_hex;
use super::net::atomic_json_write;
use super::plugin_security::{canonical_json_bytes, parse_strict_json, PluginTrustStore};

pub(crate) const PLUGIN_MANIFEST: &str = "plugin.yml";
pub(crate) const PLUGIN_V2_MANIFEST: &str = "plugin.json";
const PLUGIN_MAX_MANIFEST_BYTES: usize = 64 * 1024;
const PLUGIN_MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const PLUGIN_MAX_UNPACKED_BYTES: usize = 128 * 1024 * 1024;
const PLUGIN_MAX_FILES: usize = 4096;
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

// \z вместо $: Python fullmatch не допускает перевод строки в конце.
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

// ---------------------- мелкие утилиты ----------------------

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// str() из Python для значений JSON: None → "None", bool → "True"/"False".
fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(number) => number.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Правда Python: пустая строка/список/объект/0/None — ложь.
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

/// len(...) из Python считает кодовые точки, а не байты.
fn text_fits(value: &str, limit: usize) -> bool {
    char_len(value) <= limit
}

/// Печатым считаем всё кроме управления и пробелов-разделителей (кроме ' '):
/// приближение str.isprintable из Python.
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

// ---------------------- хранилище плагинов ----------------------

pub(crate) struct PluginStore {
    pub(crate) root: PathBuf,
    pub(crate) data_home: PathBuf,
    pub(crate) trust_store: PluginTrustStore,
    permission_path: PathBuf,
    settings: Value,
    access_lock: Mutex<()>,
}

impl PluginStore {
    pub(crate) fn new(config: &ConfigStore) -> Result<Self, String> {
        let data_home = config.paths.data_home.clone();
        let root = data_home.join("plugins");
        if path_is_symlink(&root) {
            return Err("Каталог плагинов является символьной ссылкой".to_owned());
        }
        create_dir_private(&root).map_err(|_| "Каталог плагинов является символьной ссылкой".to_owned())?;
        let trust_store = PluginTrustStore::new(&data_home.join("plugin-trust"))?;
        let permission_path = data_home.join(PLUGIN_PERMISSION_FILE);
        read_permission_state(&permission_path)?;
        Ok(Self {
            root,
            data_home,
            trust_store,
            permission_path,
            settings: config.settings().clone(),
            access_lock: Mutex::new(()),
        })
    }

    pub(crate) fn list(&self) -> Result<Vec<Value>, String> {
        let state = {
            let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            read_permission_state(&self.permission_path)?
        };
        discover_plugins_inner(&self.root, &self.trust_store, &state)
    }

    pub(crate) fn install_archive(&self, content: &[u8], enable: bool, replace: bool) -> Result<Value, String> {
        if content.is_empty() || content.len() > PLUGIN_MAX_ARCHIVE_BYTES {
            return Err("Архив плагина пустой или слишком большой".to_owned());
        }
        let staging = self.root.join(format!(".install-{}", random_hex(8)));
        if create_dir_private(&staging).is_err() {
            return Err("Не удалось установить плагин: could not create staging directory".to_owned());
        }
        let outcome = self.install_staged(&staging, content, enable, replace);
        if outcome.is_err() {
            let _ = remove_tree(&staging);
        }
        match outcome {
            Ok(value) => Ok(value),
            Err(InstallFail::User(message)) => Err(message),
            Err(InstallFail::Os(message)) => Err(format!("Не удалось установить плагин: {message}")),
        }
    }

    fn install_staged(
        &self,
        staging: &Path,
        content: &[u8],
        enable: bool,
        replace: bool,
    ) -> Result<Value, InstallFail> {
        let mut archive = zip::ZipArchive::new(Cursor::new(content))
            .map_err(|error| InstallFail::Os(error.to_string()))?;
        let members_len = archive.len();
        if members_len > PLUGIN_MAX_FILES {
            return Err(InstallFail::User("Архив плагина содержит слишком много файлов".to_owned()));
        }
        struct Member {
            name: String,
            is_dir: bool,
            size: u64,
            is_symlink: bool,
        }
        let mut members: Vec<Member> = Vec::with_capacity(members_len);
        for index in 0..members_len {
            let file = archive.by_index(index).map_err(|error| InstallFail::Os(error.to_string()))?;
            let mode = file.unix_mode().unwrap_or(0);
            members.push(Member {
                name: file.name().to_owned(),
                is_dir: file.is_dir(),
                size: file.size(),
                is_symlink: (mode & 0o170000) == 0o120000,
            });
        }

        let mut total_size: u64 = 0;
        let mut seen_members: HashSet<String> = HashSet::new();
        for (index, member) in members.iter().enumerate() {
            let path = archive_member_path(staging, &member.name)?;
            let normalized = path
                .strip_prefix(staging)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| member.name.clone());
            if !seen_members.insert(normalized) {
                return Err(InstallFail::User("Архив плагина содержит повторяющийся путь".to_owned()));
            }
            if member.is_symlink {
                return Err(InstallFail::User("Архив плагина содержит символьную ссылку".to_owned()));
            }
            if member.is_dir {
                create_dir_private(&path).map_err(|error| InstallFail::Os(error.to_string()))?;
                continue;
            }
            total_size += member.size;
            if member.size > PLUGIN_MAX_UNPACKED_BYTES as u64 || total_size > PLUGIN_MAX_UNPACKED_BYTES as u64 {
                return Err(InstallFail::User("Распакованный плагин слишком большой".to_owned()));
            }
            if let Some(parent) = path.parent() {
                create_dir_private(parent).map_err(|error| InstallFail::Os(error.to_string()))?;
            }
            let mut source = archive.by_index(index).map_err(|error| InstallFail::Os(error.to_string()))?;
            let mut payload = Vec::with_capacity(member.size as usize);
            source
                .by_ref()
                .take(PLUGIN_MAX_UNPACKED_BYTES as u64 + 1)
                .read_to_end(&mut payload)
                .map_err(|error| InstallFail::Os(error.to_string()))?;
            if payload.len() > PLUGIN_MAX_UNPACKED_BYTES {
                return Err(InstallFail::User("Распакованный плагин слишком большой".to_owned()));
            }
            let mut target = File::create(&path).map_err(|error| InstallFail::Os(error.to_string()))?;
            target.write_all(&payload).map_err(|error| InstallFail::Os(error.to_string()))?;
            restrict_mode(&path, 0o600);
        }

        let legacy_path = staging.join(PLUGIN_MANIFEST);
        let v2_path = staging.join(PLUGIN_V2_MANIFEST);
        if path_exists(&legacy_path) == path_exists(&v2_path) {
            return Err(InstallFail::User(
                "Архив должен содержать ровно один корневой plugin.yml или plugin.json".to_owned(),
            ));
        }
        let is_v2 = path_exists(&v2_path);

        let mut package: Option<V2Package> = None;
        let manifest: Value;
        if is_v2 {
            match static_json_manifest(staging)? {
                Some(static_json) => {
                    manifest = static_json;
                    let entrypoint = manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let entrypoint_path = safe_child(staging, &entrypoint)?;
                    if !path_is_file(&entrypoint_path) {
                        return Err(InstallFail::User("entrypoint плагина отсутствует в архиве".to_owned()));
                    }
                }
                None => {
                    reject_v2_runtime_paths(staging)?;
                    let parsed = read_v2_package(staging, None)?;
                    manifest = parsed.manifest.clone();
                    package = Some(parsed);
                }
            }
        } else {
            if !path_is_file(&legacy_path)
                || path_is_symlink(&legacy_path)
                || path_size(&legacy_path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64
            {
                return Err(InstallFail::User("В архиве отсутствует корректный plugin.yml".to_owned()));
            }
            let text = fs::read_to_string(&legacy_path).map_err(|error| InstallFail::Os(error.to_string()))?;
            let parsed = Value::Object(parse_plugin_yml(&text).map_err(InstallFail::User)?);
            manifest = validate_manifest(&parsed).map_err(InstallFail::User)?;
            let entrypoint = manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
            let entrypoint_path = safe_child(staging, &entrypoint)?;
            if !path_is_file(&entrypoint_path) {
                return Err(InstallFail::User("entrypoint плагина отсутствует в архиве".to_owned()));
            }
        }
        self.finish_install(staging, manifest, package, is_v2, enable, replace)
    }

    fn finish_install(
        &self,
        staging: &Path,
        manifest: Value,
        package: Option<V2Package>,
        is_v2: bool,
        enable: bool,
        replace: bool,
    ) -> Result<Value, InstallFail> {
        let plugin_id = manifest.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let target = self.root.join(&plugin_id);
        if path_exists(&target) || path_is_symlink(&target) {
            if !replace {
                return Err(InstallFail::User(format!("Плагин уже установлен: {plugin_id}")));
            }
            if path_is_symlink(&target) {
                return Err(InstallFail::User(
                    "Каталог существующего плагина является символьной ссылкой".to_owned(),
                ));
            }
            self.reconcile_replacement_permissions(&plugin_id, &target, if is_v2 { package.as_ref() } else { None })
                .map_err(InstallFail::User)?;
            for name in ["storage", "files", "config", "database", ".enabled"] {
                let old = target.join(name);
                if path_exists(&old) || path_is_symlink(&old) {
                    move_path(&old, &staging.join(name)).map_err(|error| InstallFail::Os(error.to_string()))?;
                }
            }
            remove_tree(&target).map_err(|error| InstallFail::Os(error.to_string()))?;
        }
        fs::rename(staging, &target).map_err(|error| InstallFail::Os(error.to_string()))?;
        restrict_mode(&target, 0o700);
        let enabled_marker = target.join(".enabled");
        if enable {
            // В Python static-json plugin.json с enable=True падал TypeError на
            // package=None; здесь просто не включаем такой пакет.
            let executable = !is_v2 || self.can_execute(package.as_ref(), &plugin_id);
            if executable {
                write_enabled_marker(&enabled_marker).map_err(|error| InstallFail::Os(error.to_string()))?;
            } else if path_exists(&enabled_marker) || path_is_symlink(&enabled_marker) {
                fs::remove_file(&enabled_marker).map_err(|error| InstallFail::Os(error.to_string()))?;
            }
        } else if path_exists(&enabled_marker) {
            fs::remove_file(&enabled_marker).map_err(|error| InstallFail::Os(error.to_string()))?;
        }
        self.namespace(&plugin_id).map_err(InstallFail::User)?;
        self.plugin_status(&plugin_id).map_err(InstallFail::User)
    }

    pub(crate) fn set_enabled(&self, plugin_id: &str, enabled: bool) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        let marker = directory.join(".enabled");
        if enabled {
            if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
                return Err("Плагин отозван локально или списком отзыва".to_owned());
            }
            if path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
                let package = read_v2_package(&directory, Some(plugin_id))?;
                self.require_executable(&package, plugin_id, false)?;
            }
            write_enabled_marker(&marker).map_err(|error| error.to_string())?;
        } else if path_exists(&marker) || path_is_symlink(&marker) {
            if path_is_symlink(&marker) {
                return Err("Маркер состояния плагина является символьной ссылкой".to_owned());
            }
            fs::remove_file(&marker).map_err(|error| error.to_string())?;
        }
        self.plugin_status(plugin_id)
    }

    pub(crate) fn is_enabled(&self, plugin_id: &str) -> Result<bool, String> {
        Ok(enabled_state(&self.existing_plugin_directory(plugin_id)?))
    }

    pub(crate) fn uninstall(&self, plugin_id: &str) -> Result<(), String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        remove_tree(&directory).map_err(|error| error.to_string())?;
        self.update_permission_record(plugin_id, None)
    }

    pub(crate) fn namespace(&self, plugin_id: &str) -> Result<PathBuf, String> {
        let directory = self.plugin_directory(plugin_id)?;
        for name in ["storage", "files", "config", "database"] {
            let path = directory.join(name);
            if path_is_symlink(&path) {
                return Err(format!("Каталог плагина является символьной ссылкой: {name}"));
            }
            create_dir_private(&path).map_err(|error| error.to_string())?;
        }
        Ok(directory)
    }

    pub(crate) fn manifest(&self, plugin_id: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        let v2_path = directory.join(PLUGIN_V2_MANIFEST);
        if path_exists(&v2_path) {
            if let Some(static_json) = static_json_manifest(&directory)? {
                if static_json.get("id").and_then(Value::as_str) != Some(plugin_id) {
                    return Err("id plugin.json не совпадает с каталогом".to_owned());
                }
                return Ok(static_json);
            }
            return Ok(read_v2_package(&directory, Some(plugin_id))?.manifest);
        }
        if path_is_symlink(&v2_path) {
            return Err("plugin.json является символьной ссылкой".to_owned());
        }
        let path = directory.join(PLUGIN_MANIFEST);
        if path_is_symlink(&path) || !path_is_file(&path) || path_size(&path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64 {
            return Err("plugin.yml не найден".to_owned());
        }
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let value = validate_manifest(&Value::Object(parse_plugin_yml(&text)?))?;
        if value.get("id").and_then(Value::as_str) != Some(plugin_id) {
            return Err("id plugin.yml не совпадает с каталогом".to_owned());
        }
        if path_exists(&v2_path) {
            // Недостижимо (plugin.json перехвачен выше), сохранено как в Python.
            return Err("Каталог содержит конфликтующие plugin.yml и plugin.json".to_owned());
        }
        let entrypoint = value.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
        let entrypoint_path = safe_child(&directory, &entrypoint)?;
        if !path_is_file(&entrypoint_path) {
            return Err("entrypoint плагина отсутствует".to_owned());
        }
        Ok(value)
    }

    pub(crate) fn authorize_operation(&self, plugin_id: &str, permission: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            let package = read_v2_package(&directory, Some(plugin_id))?;
            self.require_executable(&package, plugin_id, true)?;
            let declared: Vec<String> = package
                .manifest
                .get("permissions")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            if !declared.iter().any(|item| item.as_str() == permission) {
                return Err(format!("Плагину не запрошено разрешение {permission}"));
            }
            let granted = self.granted_permissions(plugin_id, &declared)?;
            if !granted.iter().any(|item| item.as_str() == permission) {
                return Err(format!("Пользователь не выдал плагину разрешение {permission}"));
            }
            return Ok(package.manifest);
        }

        let manifest = self.manifest(plugin_id)?;
        if !self.is_enabled(plugin_id)? {
            return Err("Плагин выключен".to_owned());
        }
        if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
            return Err("Плагин отозван локально или списком отзыва".to_owned());
        }
        let declared = manifest.get("permissions").and_then(Value::as_array).cloned().unwrap_or_default();
        if !declared.iter().any(|item| item.as_str() == Some(permission)) {
            return Err(format!("Плагину не выдано разрешение {permission}"));
        }
        Ok(manifest)
    }

    pub(crate) fn set_permission_grants(&self, plugin_id: &str, permissions: &Value) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Постоянные grants поддерживаются только для plugin.json".to_owned());
        }
        let manifest = read_v2_package(&directory, Some(plugin_id))?.manifest;
        let list = permissions
            .as_array()
            .ok_or_else(|| "Список grants должен содержать строки".to_owned())?;
        if list.iter().any(|item| !item.is_string()) {
            return Err("Список grants должен содержать строки".to_owned());
        }
        let requested: BTreeSet<String> = manifest
            .get("permissions")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let raw: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        let granted: BTreeSet<String> = raw.iter().cloned().collect();
        if granted.len() != raw.len() || !granted.iter().all(|item| requested.contains(item)) {
            return Err("Можно выдать только уникальные permissions из plugin.json".to_owned());
        }
        let record = json!({
            "grants": sorted_set(&raw),
            "untrusted_execution_hash": self.untrusted_execution_hash(plugin_id)?,
        });
        self.update_permission_record(plugin_id, Some(record.as_object().cloned().unwrap()))?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn set_local_untrusted_execution(&self, plugin_id: &str, allowed: bool) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Локальный запуск поддерживается только для plugin.json".to_owned());
        }
        let package = read_v2_package(&directory, Some(plugin_id))?;
        if package.signed {
            return Err("Локальный untrusted-запуск доступен только для unsigned plugin.json".to_owned());
        }
        let record = json!({
            "grants": self.stored_grants(plugin_id)?,
            "untrusted_execution_hash": if allowed { package.package_digest.clone() } else { String::new() },
        });
        self.update_permission_record(plugin_id, Some(record.as_object().cloned().unwrap()))?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn pin_publisher(&self, key_id: &str, public_key: &str, revocation_authority: bool) -> Result<Value, String> {
        self.trust_store
            .pin_publisher_key(key_id, &json!(public_key), revocation_authority)?;
        let publisher = self.trust_store.publisher_status(key_id, Some(&json!(public_key)))?;
        let mut publisher_fields = publisher.as_object().cloned().unwrap_or_default();
        publisher_fields.insert("public_key".to_owned(), json!(public_key));
        let mut result = self.security_status()?;
        result["publisher"] = Value::Object(publisher_fields);
        Ok(result)
    }

    pub(crate) fn unpin_publisher(&self, key_id: &str) -> Result<Value, String> {
        let removed = self.trust_store.unpin_publisher_key(key_id)?;
        let mut result = self.security_status()?;
        result["key_id"] = json!(key_id);
        result["unpinned"] = json!(removed);
        Ok(result)
    }

    pub(crate) fn set_publisher_revocation_authority(&self, key_id: &str, enabled: bool) -> Result<Value, String> {
        self.trust_store.set_revocation_authority(key_id, enabled)?;
        self.security_status()
    }

    pub(crate) fn import_revocations_json(&self, document_json: &str) -> Result<Value, String> {
        self.trust_store.import_revocations_json(document_json)
    }

    pub(crate) fn revoke_plugin(&self, plugin_id: &str, reason: &str) -> Result<Value, String> {
        self.trust_store.revoke_plugin(plugin_id, reason)?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn unrevoke_plugin(&self, plugin_id: &str) -> Result<Value, String> {
        self.trust_store.unrevoke_plugin(plugin_id)?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn runtime_source(&self, plugin_id: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Выполняемый runtime доступен только для plugin.json".to_owned());
        }
        let package = read_v2_package(&directory, Some(plugin_id))?;
        self.require_executable(&package, plugin_id, true)?;
        let entrypoint = package.manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
        let source_bytes = package.files.get(&entrypoint);
        let Some(source_bytes) = source_bytes else {
            return Err("entrypoint плагина отсутствует или превышает лимит runtime".to_owned());
        };
        if source_bytes.len() > PLUGIN_RUNTIME_MAX_SOURCE_BYTES {
            return Err("entrypoint плагина отсутствует или превышает лимит runtime".to_owned());
        }
        let source = std::str::from_utf8(source_bytes).map_err(|_| "entrypoint плагина должен быть UTF-8".to_owned())?;
        Ok(json!({
            "manifest": package.manifest,
            "entrypoint": entrypoint,
            "source": source,
            "package_sha256": package.package_digest,
        }))
    }

    pub(crate) fn runtime_info(&self, plugin_id: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Выполняемый runtime доступен только для plugin.json".to_owned());
        }
        let package = read_v2_package(&directory, Some(plugin_id))?;
        let manifest = &package.manifest;
        let api_version = manifest.get("api_version").and_then(Value::as_str).unwrap_or_default();
        let default_profile = if api_version == "3" { "strict" } else { "quickjs-legacy" };
        Ok(json!({
            "plugin_id": plugin_id,
            "api_version": api_version,
            "permissions": manifest.get("permissions").cloned().unwrap_or_else(|| json!([])),
            "capabilities": runtime_capabilities_for_manifest(manifest),
            "sandbox_profile": field_str(manifest.as_object().unwrap_or(&Map::new()), "sandbox_profile", default_profile),
            "package_sha256": package.package_digest,
        }))
    }

    pub(crate) fn permission_summary(&self, plugin_id: &str) -> Result<Value, String> {
        let plugin = self.plugin_status(plugin_id)?;
        Ok(json!({
            "plugin_id": plugin_id,
            "requested_permissions": plugin.get("requested_permissions").and_then(Value::as_array).cloned().unwrap_or_default(),
            "granted_permissions": plugin.get("granted_permissions").and_then(Value::as_array).cloned().unwrap_or_default(),
            "execution_allowed": plugin.get("execution_allowed").and_then(Value::as_bool).unwrap_or(false),
            "api_version": plugin.get("api_version").cloned().unwrap_or_else(|| json!("1")),
        }))
    }

    pub(crate) fn security_status(&self) -> Result<Value, String> {
        let plugins = self.list()?;
        let mut publishers: Vec<Value> = Vec::new();
        for plugin in &plugins {
            let Some(key_id) = plugin.get("publisher_key_id").and_then(Value::as_str) else { continue };
            let Some(public_key) = plugin.get("publisher_public_key").and_then(Value::as_str) else { continue };
            let Ok(status) = self.trust_store.publisher_status(key_id, Some(&json!(public_key))) else { continue };
            let mut fields = status.as_object().cloned().unwrap_or_default();
            fields.insert("public_key".to_owned(), json!(public_key));
            fields.insert(
                "fingerprint".to_owned(),
                plugin.get("publisher_fingerprint").cloned().unwrap_or(Value::Null),
            );
            publishers.push(Value::Object(fields));
        }
        Ok(json!({ "plugins": plugins, "publishers": publishers }))
    }

    pub(crate) fn quota_usage(&self, plugin_id: &str) -> Result<Value, String> {
        let namespace = self.namespace(plugin_id)?;
        let buckets: [(&str, PathBuf); 4] = [
            ("storage", namespace.join("storage")),
            ("config", namespace.join("config")),
            ("files", namespace.join("files")),
            ("database", namespace.join("database")),
        ];
        let mut directories = Map::new();
        for (name, path) in &buckets {
            directories.insert((*name).to_owned(), directory_usage(path)?);
        }
        let total_bytes: i64 = directories.values().filter_map(|item| item.get("bytes").and_then(Value::as_i64)).sum();
        let total_files: i64 = directories.values().filter_map(|item| item.get("files").and_then(Value::as_i64)).sum();
        Ok(json!({
            "plugin_id": plugin_id,
            "totals": { "bytes": total_bytes, "files": total_files },
            "directories": directories,
        }))
    }

    pub(crate) fn projected_launcher_config(&self, schema_id: &str) -> Result<Value, String> {
        const LAUNCHER_PUBLIC_CONFIG: &[&str] = &[
            "theme", "active_theme_id", "accent_color", "sidebar_position", "sidebar_mode", "sidebar_width_mode",
            "sidebar_width", "sidebar_floating", "sidebar_visibility", "animations", "density", "ui_scale",
            "graphics_mode", "launcher_version", "startup_view", "last_view", "remember_last_view",
            "default_version_filter", "default_loader", "max_downloads", "download_timeout_seconds", "download_retries",
            "show_activity_panel", "show_toasts", "font_family", "keep_logs_days", "developer_mode",
        ];
        const NETWORK_DIAGNOSTICS: &[&str] = &[
            "max_downloads", "download_timeout_seconds", "download_retries", "telemetry", "offline_debug_limit_seconds",
        ];
        let allowed: &[&str] = match schema_id {
            "launcher-public-config" => LAUNCHER_PUBLIC_CONFIG,
            "network-diagnostics" => NETWORK_DIAGNOSTICS,
            _ => return Err("Неизвестная схема projected config".to_owned()),
        };
        let removed: &[&str] = match schema_id {
            "launcher-public-config" => &[
                "client_id", "telemetry_endpoint", "http_proxy", "cloud_username", "cloud_url", "cloud_remote_path",
            ],
            _ => &["http_proxy", "client_id", "cloud_username", "cloud_url"],
        };
        let mut data = Map::new();
        if let Some(settings) = self.settings.as_object() {
            for key in allowed {
                if let Some(value) = settings.get(*key) {
                    data.insert(key.to_string(), value.clone());
                }
            }
        }
        Ok(json!({
            "schema_id": schema_id,
            "data": data,
            "removed_fields": removed,
        }))
    }

    // Python: Path(__file__).resolve().parent.parent. Здесь исполняемый файл —
    // единственный аналог «каталога лаунчера» в дистрибутиве Tauri.
    pub(crate) fn launcher_root(&self) -> Result<PathBuf, String> {
        let executable = std::env::current_exe().map_err(|_| "Не удалось определить корень лаунчера".to_owned())?;
        let resolved = fs::canonicalize(&executable).unwrap_or(executable);
        resolved
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Не удалось определить корень лаунчера".to_owned())
    }

    pub(crate) fn list_launcher_files(&self, directory: &str) -> Result<Vec<Value>, String> {
        let root = self.launcher_root()?;
        let target = if directory.is_empty() { root.clone() } else { safe_child(&root, directory)? };
        if path_is_symlink(&target) || !path_exists(&target) {
            return Ok(Vec::new());
        }
        if !path_is_dir(&target) {
            return Err("Каталог launcher-root недоступен".to_owned());
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_regular_files(&target, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<Value> = Vec::new();
        for (_, path) in files {
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
            let relative = path.strip_prefix(&root).unwrap_or(&path);
            out.push(json!({
                "path": relative.to_string_lossy().replace('\\', "/"),
                "size": path_size(&path).unwrap_or(0),
                "modified_at": file_mtime(&path),
            }));
        }
        Ok(out)
    }

    pub(crate) fn read_launcher_file(&self, relative_path: &str) -> Result<String, String> {
        let path = safe_child(&self.launcher_root()?, relative_path)?;
        if !path_is_file(&path) || path_size(&path).unwrap_or(0) > MAX_LAUNCHER_FILE_BYTES as u64 {
            return Err("Файл launcher-root не найден или слишком большой".to_owned());
        }
        fs::read_to_string(&path).map_err(|error| format!("Не удалось прочитать файл launcher-root: {error}"))
    }

    pub(crate) fn read_storage(&self, plugin_id: &str, key: &str) -> Result<Value, String> {
        let path = self.storage_path(plugin_id, key)?;
        if !path_exists(&path) {
            return Ok(Value::Null);
        }
        if path_is_symlink(&path) || path_size(&path).unwrap_or(0) > MAX_STORAGE_BYTES as u64 {
            return Err("Данные плагина повреждены или слишком большие".to_owned());
        }
        let raw = fs::read(&path).map_err(|error| format!("Не удалось прочитать данные плагина: {error}"))?;
        serde_json::from_slice(&raw).map_err(|error| format!("Не удалось прочитать данные плагина: {error}"))
    }

    pub(crate) fn write_storage(&self, plugin_id: &str, key: &str, value: &Value) -> Result<(), String> {
        let path = self.storage_path(plugin_id, key)?;
        let encoded = serde_json::to_vec(value).map_err(|error| format!("Данные плагина слишком большие: {error}"))?;
        if encoded.len() > MAX_STORAGE_BYTES {
            return Err("Данные плагина слишком большие".to_owned());
        }
        atomic_json_write(&path, value, true)
    }

    pub(crate) fn remove_storage(&self, plugin_id: &str, key: &str) -> Result<(), String> {
        let path = self.storage_path(plugin_id, key)?;
        if path_is_symlink(&path) {
            return Err("Данные плагина являются символьной ссылкой".to_owned());
        }
        if path_exists(&path) {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn read_config(&self, plugin_id: &str) -> Result<Map<String, Value>, String> {
        let path = self.config_path(plugin_id)?;
        if !path_exists(&path) {
            return Ok(Map::new());
        }
        if path_is_symlink(&path) || path_size(&path).unwrap_or(0) > MAX_CONFIG_BYTES as u64 {
            return Err("config.yml плагина повреждён или слишком большой".to_owned());
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return Err(format!("Не удалось прочитать config.yml плагина: {error}")),
        };
        // parse_plugin_yml всегда строит объект, так что isinstance-проверка Python недостижима.
        parse_plugin_yml(&text)
    }

    pub(crate) fn write_config(&self, plugin_id: &str, value: &Value) -> Result<(), String> {
        let Some(fields) = value.as_object() else {
            return Err("Конфигурация плагина должна быть объектом".to_owned());
        };
        let path = self.config_path(plugin_id)?;
        let encoded = dump_yaml(fields)?;
        if encoded.len() > MAX_CONFIG_BYTES {
            return Err("config.yml плагина слишком большой".to_owned());
        }
        if let Some(parent) = path.parent() {
            create_dir_private(parent).map_err(|error| error.to_string())?;
        }
        let temporary = path.with_file_name(format!(".config.{}", random_hex(8)));
        let outcome = (|| -> Result<(), std::io::Error> {
            let mut file = open_private_file(&temporary)?;
            file.write_all(encoded.as_bytes())?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome.map_err(|error| format!("Не удалось сохранить config.yml плагина: {error}"))
    }

    pub(crate) fn config_get(&self, plugin_id: &str, key: &str) -> Result<Value, String> {
        validate_config_key(key)?;
        let config = self.read_config(plugin_id)?;
        Ok(config.get(key).cloned().unwrap_or(Value::Null))
    }

    pub(crate) fn config_set(&self, plugin_id: &str, key: &str, value: &Value) -> Result<(), String> {
        validate_config_key(key)?;
        let mut config = self.read_config(plugin_id)?;
        config.insert(key.to_owned(), value.clone());
        self.write_config(plugin_id, &Value::Object(config))
    }

    pub(crate) fn config_remove(&self, plugin_id: &str, key: &str) -> Result<(), String> {
        validate_config_key(key)?;
        let mut config = self.read_config(plugin_id)?;
        config.remove(key);
        self.write_config(plugin_id, &Value::Object(config))
    }

    pub(crate) fn file_path(&self, plugin_id: &str, relative_path: &str) -> Result<PathBuf, String> {
        let directory = self.namespace(plugin_id)?.join("files");
        safe_child(&directory, relative_path)
    }

    pub(crate) fn list_files(&self, plugin_id: &str, directory: &str) -> Result<Vec<String>, String> {
        let root = self.namespace(plugin_id)?.join("files");
        let target = if directory.is_empty() { root.clone() } else { safe_child(&root, directory)? };
        if path_is_symlink(&target) || !path_exists(&target) {
            return Ok(Vec::new());
        }
        if !path_is_dir(&target) {
            return Err("Каталог файлов плагина недоступен".to_owned());
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_regular_files(&target, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<String> = Vec::new();
        for (_, path) in files {
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
            out.push(path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/"));
        }
        Ok(out)
    }

    pub(crate) fn list_file_info(&self, plugin_id: &str, directory: &str) -> Result<Vec<Value>, String> {
        let root = self.namespace(plugin_id)?.join("files");
        let target = if directory.is_empty() { root.clone() } else { safe_child(&root, directory)? };
        if path_is_symlink(&target) || !path_exists(&target) {
            return Ok(Vec::new());
        }
        if !path_is_dir(&target) {
            return Err("Каталог файлов плагина недоступен".to_owned());
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_regular_files(&target, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<Value> = Vec::new();
        for (_, path) in files {
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
            out.push(json!({
                "path": path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/"),
                "size": path_size(&path).unwrap_or(0),
                "modified_at": file_mtime(&path),
            }));
        }
        Ok(out)
    }

    pub(crate) fn read_file(&self, plugin_id: &str, relative_path: &str) -> Result<String, String> {
        let path = self.file_path(plugin_id, relative_path)?;
        if !path_is_file(&path) || path_size(&path).unwrap_or(0) > MAX_PLUGIN_FILE_BYTES as u64 {
            return Err("Файл плагина не найден или слишком большой".to_owned());
        }
        fs::read_to_string(&path).map_err(|error| format!("Не удалось прочитать файл плагина: {error}"))
    }

    pub(crate) fn write_file(&self, plugin_id: &str, relative_path: &str, content: &str) -> Result<(), String> {
        let path = self.file_path(plugin_id, relative_path)?;
        let encoded = content.as_bytes();
        if encoded.len() > MAX_PLUGIN_FILE_BYTES {
            return Err("Файл плагина слишком большой".to_owned());
        }
        let files_root = self.namespace(plugin_id)?.join("files");
        if let Some(parent) = path.parent() {
            create_dir_private(parent).map_err(|error| error.to_string())?;
        }
        assert_no_symlink_chain(path.parent().unwrap_or(&path), &files_root)?;
        if path_exists(&path) || path_is_symlink(&path) {
            if path_is_symlink(&path) {
                return Err("Символьная ссылка в файлах плагина запрещена".to_owned());
            }
        }
        fs::write(&path, encoded).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) fn remove_file(&self, plugin_id: &str, relative_path: &str) -> Result<(), String> {
        let path = self.file_path(plugin_id, relative_path)?;
        if path_is_symlink(&path) {
            return Err("Символьная ссылка в файлах плагина запрещена".to_owned());
        }
        if path_exists(&path) {
            if !path_is_file(&path) {
                return Err("Можно удалить только файл плагина".to_owned());
            }
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn require_executable(&self, package: &V2Package, plugin_id: &str, require_enabled: bool) -> Result<(), String> {
        if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
            return Err("Плагин отозван локально или списком отзыва".to_owned());
        }
        if package.signed {
            let key_id = package.key_id.clone().unwrap_or_default();
            let public_key = package.public_key.clone().unwrap_or_default();
            if !self
                .trust_store
                .is_trusted_publisher(&key_id, Some(&json!(public_key)))
                .unwrap_or(false)
            {
                return Err("Издатель не закреплён в локальном trust store или отозван".to_owned());
            }
        } else if self.untrusted_execution_hash(plugin_id)? != package.package_digest {
            return Err("Для unsigned-плагина требуется явное согласие на локальный untrusted-запуск".to_owned());
        }
        if require_enabled && !enabled_state(&self.existing_plugin_directory(plugin_id)?) {
            return Err("Плагин выключен".to_owned());
        }
        Ok(())
    }

    fn can_execute(&self, package: Option<&V2Package>, plugin_id: &str) -> bool {
        let Some(package) = package else { return false };
        if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
            return false;
        }
        if package.signed {
            let key_id = package.key_id.clone().unwrap_or_default();
            let public_key = package.public_key.clone().unwrap_or_default();
            return self
                .trust_store
                .is_trusted_publisher(&key_id, Some(&json!(public_key)))
                .unwrap_or(false);
        }
        self.untrusted_execution_hash(plugin_id).unwrap_or_default() == package.package_digest
    }

    fn plugin_status(&self, plugin_id: &str) -> Result<Value, String> {
        for plugin in self.list()? {
            if plugin.get("id").and_then(Value::as_str) == Some(plugin_id) {
                return Ok(plugin);
            }
        }
        Err("Плагин не установлен".to_owned())
    }

    fn permission_record(&self, plugin_id: &str) -> Result<Value, String> {
        let state = {
            let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            read_permission_state(&self.permission_path)?
        };
        Ok(state
            .pointer("/plugins")
            .and_then(Value::as_object)
            .and_then(|fields| fields.get(plugin_id))
            .cloned()
            .unwrap_or_else(default_permission_record))
    }

    fn stored_grants(&self, plugin_id: &str) -> Result<Vec<String>, String> {
        Ok(self
            .permission_record(plugin_id)?
            .get("grants")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default())
    }

    fn untrusted_execution_hash(&self, plugin_id: &str) -> Result<String, String> {
        Ok(py_str(&self.permission_record(plugin_id)?.get("untrusted_execution_hash").cloned().unwrap_or(Value::Null)))
    }

    fn granted_permissions(&self, plugin_id: &str, declared: &[String]) -> Result<Vec<String>, String> {
        let stored = self.stored_grants(plugin_id)?;
        Ok(sorted_set(&declared.iter().filter(|item| stored.contains(item)).cloned().collect::<Vec<_>>()))
    }

    fn update_permission_record(&self, plugin_id: &str, value: Option<Map<String, Value>>) -> Result<(), String> {
        let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = read_permission_state(&self.permission_path)?;
        if let Some(plugins) = state.get_mut("plugins").and_then(Value::as_object_mut) {
            match value {
                Some(record) => {
                    plugins.insert(plugin_id.to_owned(), Value::Object(record));
                }
                None => {
                    plugins.remove(plugin_id);
                }
            }
        }
        write_permission_state(&self.permission_path, &state)
    }

    fn reconcile_replacement_permissions(
        &self,
        plugin_id: &str,
        directory: &Path,
        replacement: Option<&V2Package>,
    ) -> Result<(), String> {
        let previous = if path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            read_v2_package(directory, Some(plugin_id)).ok()
        } else {
            None
        };

        let manifest_permissions = |package: Option<&V2Package>| -> BTreeSet<String> {
            package
                .map(|package| {
                    package
                        .manifest
                        .get("permissions")
                        .and_then(Value::as_array)
                        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default()
                })
                .unwrap_or_default()
        };
        let previous_permissions = manifest_permissions(previous.as_ref());
        let next_permissions = manifest_permissions(replacement);
        let record = self.permission_record(plugin_id)?;
        let stored_grants: Vec<String> = record
            .get("grants")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let grants: Vec<String> = stored_grants
            .iter()
            .filter(|item| previous_permissions.contains(*item) && next_permissions.contains(*item))
            .cloned()
            .collect();
        let grants = sorted_set(&grants);
        let mut execution_hash = String::new();
        if let (Some(previous), Some(replacement)) = (previous.as_ref(), replacement) {
            if previous.package_digest == replacement.package_digest {
                execution_hash = py_str(&record.get("untrusted_execution_hash").cloned().unwrap_or(Value::Null));
            }
        }

        if grants.is_empty() && execution_hash.is_empty() {
            self.update_permission_record(plugin_id, None)
        } else {
            let record = json!({"grants": grants, "untrusted_execution_hash": execution_hash});
            self.update_permission_record(plugin_id, Some(record.as_object().cloned().unwrap()))
        }
    }

    fn plugin_directory(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !PLUGIN_ID_RE.is_match(plugin_id) {
            return Err("Небезопасный идентификатор плагина".to_owned());
        }
        let directory = self.root.join(plugin_id);
        if path_is_symlink(&directory) {
            return Err("Каталог плагина является символьной ссылкой".to_owned());
        }
        create_dir_private(&directory).map_err(|error| error.to_string())?;
        Ok(directory)
    }

    fn existing_plugin_directory(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !PLUGIN_ID_RE.is_match(plugin_id) {
            return Err("Небезопасный идентификатор плагина".to_owned());
        }
        let directory = self.root.join(plugin_id);
        if path_is_symlink(&directory) {
            return Err("Каталог плагина является символьной ссылкой".to_owned());
        }
        let legacy = directory.join(PLUGIN_MANIFEST);
        let v2 = directory.join(PLUGIN_V2_MANIFEST);
        let has_legacy = path_exists(&legacy) || path_is_symlink(&legacy);
        let has_v2 = path_exists(&v2) || path_is_symlink(&v2);
        if !path_is_dir(&directory) || !(has_legacy || has_v2) {
            return Err("Плагин не установлен".to_owned());
        }
        Ok(directory)
    }

    fn storage_path(&self, plugin_id: &str, key: &str) -> Result<PathBuf, String> {
        let normalized = key.trim();
        if !STORAGE_KEY_RE.is_match(normalized) {
            return Err("Небезопасный ключ storage".to_owned());
        }
        Ok(self.namespace(plugin_id)?.join("storage").join(format!("{normalized}.json")))
    }

    fn config_path(&self, plugin_id: &str) -> Result<PathBuf, String> {
        let path = self.namespace(plugin_id)?.join("config").join("config.yml");
        if path_is_symlink(&path) {
            return Err("config.yml плагина является символьной ссылкой".to_owned());
        }
        Ok(path)
    }
}

// ---------------------- инвентаризация ----------------------

pub(crate) fn discover_plugins(root: &Path) -> Result<Vec<Value>, String> {
    let trust_store = PluginTrustStore::new(&root.parent().unwrap_or(Path::new(".")).join("plugin-trust"))?;
    let permission_state = read_permission_state(&root.parent().unwrap_or(Path::new(".")).join(PLUGIN_PERMISSION_FILE))?;
    discover_plugins_inner(root, &trust_store, &permission_state)
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
        let static_json = if has_v2 { static_json_manifest(directory)? } else { None };
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
        // Битые legacy-манифесты молча пропускаем — как в Python.
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

// ---------------------- plugin.yml ----------------------

pub(crate) fn parse_plugin_yml(text: &str) -> Result<Map<String, Value>, String> {
    if text.len() > PLUGIN_MAX_MANIFEST_BYTES {
        return Err("plugin.yml слишком большой".to_owned());
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut stack: Vec<Frame> = vec![Frame::root()];
    for (index, raw_line) in lines.iter().enumerate() {
        let number = index + 1;
        if raw_line.contains('\t') {
            return Err(format!("plugin.yml содержит tab на строке {number}"));
        }
        let line = strip_yaml_comment(raw_line).trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with("---") {
            continue;
        }
        let indent = (line.len() - line.trim_start_matches(' ').len()) as i64;
        let content = line.trim();
        while matches!(stack.last(), Some(frame) if indent <= frame.indent) {
            attach_frame(&mut stack);
        }
        if stack.is_empty() {
            return Err(format!("Некорректный отступ в plugin.yml на строке {number}"));
        }
        if let Some(item) = content.strip_prefix("- ") {
            match &mut stack.last_mut().expect("стек не пуст").node {
                YamlNode::List(items) => items.push(yaml_scalar(item.trim())?),
                YamlNode::Map(_) => {
                    return Err(format!("Список plugin.yml находится не под ключом на строке {number}"))
                }
            }
            continue;
        }
        let Some((raw_key, raw_value)) = content.split_once(':') else {
            return Err(format!("Некорректная строка plugin.yml: {number}"));
        };
        let key = raw_key.trim();
        if !YAML_KEY_RE.is_match(key) {
            return Err(format!("Небезопасный ключ plugin.yml: {key}"));
        }
        let value = raw_value.trim();
        let parent = &mut stack.last_mut().expect("стек не пуст").node;
        match parent {
            YamlNode::Map(fields) => {
                if !value.is_empty() {
                    fields.insert(key.to_owned(), yaml_scalar(value)?);
                } else {
                    // Вставка дочернего узла отложена до pop: пока фрейм на вершине
                    // стека, пишут только в него, а порядок ключей как в Python.
                    let child = if next_is_list(&lines, number) {
                        YamlNode::List(Vec::new())
                    } else {
                        YamlNode::Map(Map::new())
                    };
                    stack.push(Frame { indent, node: child, attach: Some(key.to_owned()) });
                }
            }
            YamlNode::List(_) => return Err(format!("Объект plugin.yml ожидается на строке {number}")),
        }
    }
    while stack.len() > 1 {
        attach_frame(&mut stack);
    }
    match stack.pop() {
        Some(Frame { node: YamlNode::Map(fields), .. }) => Ok(fields),
        _ => Ok(Map::new()),
    }
}

enum YamlNode {
    Map(Map<String, Value>),
    List(Vec<Value>),
}

struct Frame {
    indent: i64,
    node: YamlNode,
    attach: Option<String>,
}

impl Frame {
    fn root() -> Self {
        // -1 как в Python: корневой фрейм нельзя вытолкнуть никаким отступом.
        Self { indent: -1, node: YamlNode::Map(Map::new()), attach: None }
    }
}

fn attach_frame(stack: &mut Vec<Frame>) {
    let Some(frame) = stack.pop() else { return };
    let Some(key) = frame.attach else { return };
    let Some(parent) = stack.last_mut() else { return };
    if let YamlNode::Map(fields) = &mut parent.node {
        fields.insert(key, match frame.node {
            YamlNode::Map(fields) => Value::Object(fields),
            YamlNode::List(items) => Value::Array(items),
        });
    }
}

fn next_is_list(lines: &[&str], index: usize) -> bool {
    for raw in lines.iter().skip(index) {
        let value = strip_yaml_comment(raw).trim();
        if value.is_empty() {
            continue;
        }
        return value.starts_with('-');
    }
    false
}

fn strip_yaml_comment(value: &str) -> &str {
    let mut quote = '\0';
    for (index, character) in value.char_indices() {
        if character == '\'' || character == '"' {
            if quote == character {
                quote = '\0';
            } else if quote == '\0' {
                quote = character;
            }
        } else if character == '#' && quote == '\0' && (index == 0 || value[..index].ends_with(char::is_whitespace)) {
            return &value[..index];
        }
    }
    value
}

fn yaml_scalar(value: &str) -> Result<Value, String> {
    if value.starts_with('!') || value.starts_with('&') || value.starts_with('*') || value == "|" || value == ">" {
        return Err("plugin.yml не поддерживает YAML tags, anchors и blocks".to_owned());
    }
    if value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value).map_err(|_| "Некорректная строка plugin.yml".to_owned());
    }
    if value.starts_with('\'') && value.ends_with('\'') {
        let inner = if value.len() >= 2 { &value[1..value.len() - 1] } else { "" };
        return Ok(json!(inner.replace("''", "'")));
    }
    match value.to_lowercase().as_str() {
        "true" | "yes" => return Ok(json!(true)),
        "false" | "no" => return Ok(json!(false)),
        "null" | "~" => return Ok(Value::Null),
        _ => {}
    }
    if YAML_INT_RE.is_match(value) {
        // Целые шире i64 оставляем строкой — JSON-числа в manifest-контрактах не шире i64.
        if let Ok(parsed) = value.parse::<i64>() {
            return Ok(json!(parsed));
        }
    }
    if YAML_FLOAT_RE.is_match(value) {
        if let Ok(parsed) = value.parse::<f64>() {
            return Ok(json!(parsed));
        }
    }
    if value.starts_with('[') && value.ends_with(']') {
        let parsed: Value =
            serde_json::from_str(value).map_err(|_| "Списки plugin.yml должны быть JSON-подобными".to_owned())?;
        if !parsed.is_array() {
            return Err("Некорректный список plugin.yml".to_owned());
        }
        return Ok(parsed);
    }
    Ok(json!(value))
}

// ---------------------- манифесты ----------------------

fn static_json_manifest(root: &Path) -> Result<Option<Value>, String> {
    let path = root.join(PLUGIN_V2_MANIFEST);
    if path_is_symlink(&path) || !path_is_file(&path) || path_size(&path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64 {
        return Ok(None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(fields) = parsed.as_object() else { return Ok(None) };
    let api_version = field_str(fields, "api_version", "1");
    if api_version.trim() != "1" {
        return Ok(None);
    }
    // Ошибки валидации пробивают наружу (как LauncherError в Python) и ломают list().
    Ok(Some(validate_manifest(&parsed)?))
}

pub(crate) fn validate_manifest(value: &Value) -> Result<Value, String> {
    let Some(fields) = value.as_object() else {
        return Err("plugin.yml должен содержать объект".to_owned());
    };
    let plugin_id = field_str(fields, "id", "").trim().to_lowercase();
    let name = field_str(fields, "name", "").trim().to_owned();
    let version = field_str(fields, "version", "").trim().to_owned();
    let entrypoint = field_str(fields, "entrypoint", "").trim().replace('\\', "/");
    if !PLUGIN_ID_RE.is_match(&plugin_id) {
        return Err("plugin.yml содержит небезопасный id".to_owned());
    }
    if name.is_empty() || char_len(&name) > 120 || version.is_empty() {
        return Err("plugin.yml требует name и version".to_owned());
    }
    let entrypoint_parts: Vec<&str> = entrypoint.split('/').filter(|part| !part.is_empty()).collect();
    if entrypoint.is_empty()
        || entrypoint.starts_with('/')
        || entrypoint_parts.contains(&"..")
        || !entrypoint.ends_with(".js")
    {
        return Err("entrypoint должен быть относительным .js-файлом".to_owned());
    }
    let permissions = fields.get("permissions").cloned().unwrap_or_else(|| json!([]));
    let Some(permissions) = permissions.as_array() else {
        return Err("permissions в plugin.yml должен быть списком".to_owned());
    };
    let normalized_permissions: Vec<String> = permissions.iter().map(py_str).map(|item| item.trim().to_lowercase()).collect();
    if normalized_permissions.iter().any(|item| !PLUGIN_PERMISSION_NAMES.contains(item.as_str())) {
        return Err("plugin.yml содержит неизвестное permission".to_owned());
    }
    let network_hosts = fields.get("network_hosts").cloned().unwrap_or_else(|| json!([]));
    let Some(network_hosts) = network_hosts.as_array() else {
        return Err("network_hosts в plugin.yml должен быть списком".to_owned());
    };
    let hosts: Vec<String> = network_hosts
        .iter()
        .map(py_str)
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.trim().to_lowercase().trim_end_matches('.').to_owned())
        .collect();
    for host in &hosts {
        if !HOSTNAME_RE.is_match(host) {
            return Err("network_hosts должен содержать только имена хостов".to_owned());
        }
        if host.parse::<std::net::IpAddr>().is_ok() {
            return Err("network_hosts не может содержать IP-адреса".to_owned());
        }
    }
    let mut sidebar_tab: Option<String> = None;
    if let Some(raw_tab) = fields.get("sidebar_tab") {
        let tab = raw_tab
            .as_object()
            .filter(|tab| tab.len() == 1 && tab.contains_key("label"))
            .ok_or_else(|| "sidebar_tab должен содержать только поле label".to_owned())?;
        let raw_label = tab.get("label").cloned().unwrap_or(Value::Null);
        let Some(label) = raw_label.as_str() else {
            return Err("sidebar_tab.label должен быть строкой".to_owned());
        };
        let label = label.trim();
        if label.is_empty() || char_len(label) > 40 || !py_printable(label) {
            return Err("sidebar_tab.label должен содержать от 1 до 40 печатных символов".to_owned());
        }
        if !normalized_permissions.iter().any(|item| item == "ui:compose") {
            return Err("sidebar_tab требует permission ui:compose".to_owned());
        }
        sidebar_tab = Some(label.to_owned());
    }

    let mut manifest = Map::new();
    manifest.insert("id".to_owned(), json!(plugin_id));
    manifest.insert("name".to_owned(), json!(name));
    manifest.insert("version".to_owned(), json!(truncate_chars(&version, 64)));
    manifest.insert("author".to_owned(), json!(truncate_chars(&field_str(fields, "author", "Local developer"), 120)));
    manifest.insert("description".to_owned(), json!(truncate_chars(&field_str(fields, "description", ""), 500)));
    manifest.insert("icon".to_owned(), json!(truncate_chars(&field_str(fields, "icon", "plug"), 64)));
    manifest.insert("category".to_owned(), json!(truncate_chars(&field_str(fields, "category", "Local"), 64)));
    manifest.insert("api_version".to_owned(), json!(truncate_chars(&field_str(fields, "api_version", "1"), 16)));
    manifest.insert("entrypoint".to_owned(), json!(entrypoint));
    manifest.insert("permissions".to_owned(), json!(dedup_strings(&normalized_permissions)));
    manifest.insert("network_hosts".to_owned(), json!(dedup_strings(&hosts)));
    manifest.insert(
        "featured".to_owned(),
        json!(py_truthy(fields.get("featured").unwrap_or(&Value::Null))),
    );
    if let Some(label) = sidebar_tab {
        manifest.insert("sidebar_tab".to_owned(), json!({ "label": label }));
    }
    Ok(Value::Object(manifest))
}

pub(crate) fn validate_plugin_json_manifest(value: &Value) -> Result<Value, String> {
    if !value.is_object() {
        return Err("plugin.json must contain a JSON object".to_owned());
    }
    match value.get("api_version").and_then(Value::as_str) {
        Some("2") => validate_v2_manifest(value),
        Some("3") => validate_v3_manifest(value),
        _ => Err("plugin.json api_version must be \"2\" or \"3\"".to_owned()),
    }
}

pub(crate) fn validate_v2_manifest(value: &Value) -> Result<Value, String> {
    validate_v2_or_v3(value, "2")
}

pub(crate) fn validate_v3_manifest(value: &Value) -> Result<Value, String> {
    validate_v2_or_v3(value, "3")
}

fn validate_v2_or_v3(value: &Value, api_version: &str) -> Result<Value, String> {
    let version_label = api_version;
    let Some(fields) = value.as_object() else {
        return Err("plugin.json must contain a JSON object".to_owned());
    };
    let known_fields: BTreeSet<&str> = if api_version == "3" {
        V2_FIELDS.iter().chain(V3_EXTRA_FIELDS.iter()).copied().collect()
    } else {
        V2_FIELDS.iter().copied().collect()
    };
    let unknown: Vec<&str> = fields.keys().map(String::as_str).filter(|key| !known_fields.contains(key)).collect();
    if !unknown.is_empty() {
        let mut sorted = unknown;
        sorted.sort_unstable();
        return Err(format!("plugin.json contains unsupported field(s): {}", sorted.join(", ")));
    }
    for required in ["id", "name", "version", "api_version", "entrypoint", "permissions", "integrity"] {
        if !fields.contains_key(required) {
            return Err(format!("plugin.json is missing a required v{version_label} field"));
        }
    }
    for (field, limit) in [("id", 64), ("name", 120), ("version", 64), ("api_version", 16), ("entrypoint", 256)] {
        schema_text(
            fields.get(field).unwrap_or(&Value::Null),
            &format!("plugin.json.{field}"),
            limit,
            false,
        )?;
    }
    let entrypoint = field_str(fields, "entrypoint", "");
    v2_package_path(&entrypoint)?;
    if fields.get("api_version").and_then(Value::as_str) != Some(api_version) {
        return Err(format!("plugin.json api_version must be \"{api_version}\""));
    }
    let permission_names = if api_version == "3" { &*PLUGIN_V3_PERMISSION_NAMES } else { &*PLUGIN_V2_PERMISSION_NAMES };
    let Some(permissions) = fields.get("permissions").and_then(Value::as_array) else {
        return Err("plugin.json permissions must be an array of strings".to_owned());
    };
    if permissions.iter().any(|item| !item.is_string()) {
        return Err("plugin.json permissions must be an array of strings".to_owned());
    }
    if permissions.iter().any(|item| !item.as_str().map(|name| permission_names.contains(name)).unwrap_or(false)) {
        return Err("plugin.json contains an unknown permission".to_owned());
    }
    if let Some(network_hosts) = fields.get("network_hosts") {
        let hosts = network_hosts.as_array().cloned().unwrap_or_default();
        if !network_hosts.is_array() || hosts.iter().any(|item| !item.is_string()) {
            return Err("plugin.json network_hosts must be an array of strings".to_owned());
        }
    }
    for (field, limit) in [
        ("author", 120),
        ("description", 500),
        ("icon", 64),
        ("category", 64),
        ("update_channel", 64),
        ("sandbox_profile", 64),
    ] {
        if let Some(item) = fields.get(field) {
            schema_text(item, &format!("plugin.json.{field}"), limit, field == "description")?;
        }
    }
    schema_object(
        fields.get("integrity").unwrap_or(&Value::Null),
        "plugin.json.integrity",
        &["files"],
        &["files"],
    )?;
    if !fields
        .get("integrity")
        .and_then(|integrity| integrity.get("files"))
        .map(Value::is_object)
        .unwrap_or(false)
    {
        return Err("plugin.json integrity.files must be an object".to_owned());
    }
    if let Some(signature) = fields.get("signature") {
        if !signature.is_object() {
            return Err("plugin.json signature must be an object".to_owned());
        }
    }

    let validated = validate_manifest(value)?;
    // Сравнение как в Python — с нормализованной строкой: нестроговой id не совпадёт.
    if value.get("id").and_then(Value::as_str) != validated.get("id").and_then(Value::as_str) {
        return Err("plugin.json id and entrypoint must use their normalized forms".to_owned());
    }
    if value.get("entrypoint").and_then(Value::as_str) != validated.get("entrypoint").and_then(Value::as_str) {
        return Err("plugin.json id and entrypoint must use their normalized forms".to_owned());
    }
    if let Some(config) = fields.get("config") {
        validate_v2_config(config)?;
    }
    if let Some(ui) = fields.get("ui") {
        validate_v2_ui(ui)?;
    }
    if let Some(i18n) = fields.get("i18n") {
        validate_v2_i18n(i18n)?;
    }
    if let Some(theme) = fields.get("theme") {
        validate_v2_theme(theme)?;
    }
    if let Some(actions) = fields.get("actions") {
        let actions = schema_array(actions, "plugin.json.actions", 256)?;
        validate_v2_named_records(
            actions,
            "plugin.json.actions[]",
            &["id", "label", "description", "icon", "confirmation"],
            &["id", "label"],
        )?;
    }
    if let Some(events) = fields.get("events") {
        let limit = if api_version == "3" { 128 } else { 64 };
        let events = schema_array(events, "plugin.json.events", limit)?;
        let known_events = if api_version == "3" { PLUGIN_V3_EVENTS } else { PLUGIN_V2_EVENTS };
        if events.iter().any(|item| !item.as_str().map(|name| known_events.contains(&name)).unwrap_or(false)) {
            return Err("plugin.json.events contains an unsupported launcher event".to_owned());
        }
    }
    if let Some(commands) = fields.get("commands") {
        validate_v2_commands(commands)?;
    }
    if let Some(shortcuts) = fields.get("shortcuts") {
        let shortcuts = schema_array(shortcuts, "plugin.json.shortcuts", 256)?;
        validate_v2_named_records(
            shortcuts,
            "plugin.json.shortcuts[]",
            &["id", "label", "default_key", "description"],
            &["id", "label", "default_key"],
        )?;
    }
    if api_version == "3" {
        for (field, maximum) in [
            ("capabilities", 64 * 1024usize),
            ("consent_scopes", 16 * 1024),
            ("extension_points", 16 * 1024),
            ("dependency_graph", 64 * 1024),
            ("compatibility_matrix", 64 * 1024),
            ("host_requirements", 32 * 1024),
        ] {
            if let Some(item) = fields.get(field) {
                validate_bounded_json(item, &format!("plugin.json.{field}"), maximum, 12)?;
            }
        }
    }

    let mut result = fields.clone();
    for (key, normalized) in validated.as_object().cloned().unwrap_or_default() {
        if matches!(key.as_str(), "id" | "name" | "version" | "author" | "description" | "icon" | "category" | "entrypoint" | "permissions" | "network_hosts") {
            result.insert(key, normalized);
        }
    }
    result.insert("api_version".to_owned(), json!(api_version));
    Ok(Value::Object(result))
}

// ---------------------- примитивы схемы plugin.json ----------------------

fn schema_text<'a>(value: &'a Value, label: &str, maximum: usize, allow_empty: bool) -> Result<&'a str, String> {
    let invalid = || format!("{label} must be text of at most {maximum} characters");
    let Some(text) = value.as_str() else { return Err(invalid()) };
    // len() из Python — в кодовых точках.
    if !text_fits(text, maximum) || (!allow_empty && text.trim().is_empty()) || text.contains('\0') {
        return Err(invalid());
    }
    Ok(text)
}

fn schema_object<'a>(value: &'a Value, label: &str, allowed: &[&str], required: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let Some(fields) = value.as_object() else {
        return Err(format!("{label} has an invalid object shape"));
    };
    if !required.iter().all(|key| fields.contains_key(*key)) || fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{label} has an invalid object shape"));
    }
    Ok(fields)
}

fn schema_array<'a>(value: &'a Value, label: &str, maximum: usize) -> Result<&'a Vec<Value>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{label} must be an array of at most {maximum} items"));
    };
    if items.len() > maximum {
        return Err(format!("{label} must be an array of at most {maximum} items"));
    }
    Ok(items)
}

fn schema_identifier<'a>(value: &'a Value, label: &str) -> Result<&'a str, String> {
    let text = schema_text(value, label, 80, false)?;
    if !SCHEMA_IDENTIFIER_RE.is_match(text) {
        return Err(format!("{label} is not a safe identifier"));
    }
    Ok(text)
}

fn schema_optional_text(record: &Map<String, Value>, key: &str, label: &str, maximum: usize) -> Result<(), String> {
    if let Some(value) = record.get(key) {
        schema_text(value, &format!("{label}.{key}"), maximum, true)?;
    }
    Ok(())
}

fn schema_bool(record: &Map<String, Value>, key: &str, label: &str) -> Result<(), String> {
    if let Some(value) = record.get(key) {
        if !value.is_boolean() {
            return Err(format!("{label}.{key} must be a boolean"));
        }
    }
    Ok(())
}

fn schema_number(record: &Map<String, Value>, key: &str, label: &str) -> Result<(), String> {
    if let Some(value) = record.get(key) {
        if !finite_number(value) {
            return Err(format!("{label}.{key} must be a finite number"));
        }
    }
    Ok(())
}

fn finite_number(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64().map(|float| float.is_finite()).unwrap_or(true),
        _ => false,
    }
}

fn validate_bounded_json(value: &Value, label: &str, max_bytes: usize, max_depth: usize) -> Result<(), String> {
    fn walk(node: &Value, depth: usize, label: &str, max_depth: usize) -> Result<(), String> {
        if depth > max_depth {
            return Err(format!("{label} exceeds the supported nesting depth"));
        }
        match node {
            Value::Null | Value::String(_) | Value::Bool(_) => Ok(()),
            Value::Number(_) => {
                if !finite_number(node) {
                    return Err(format!("{label} contains a non-finite number"));
                }
                Ok(())
            }
            Value::Array(items) => {
                if items.len() > 1024 {
                    return Err(format!("{label} contains too many array items"));
                }
                for item in items {
                    walk(item, depth + 1, label, max_depth)?;
                }
                Ok(())
            }
            Value::Object(fields) => {
                if fields.len() > 1024 {
                    return Err(format!("{label} contains too many object fields"));
                }
                for (key, item) in fields {
                    if !text_fits(key, 256) {
                        return Err(format!("{label} contains an unsafe object key"));
                    }
                    walk(item, depth + 1, label, max_depth)?;
                }
                Ok(())
            }
        }
    }
    walk(value, 0, label, max_depth)?;
    let encoded = canonical_json_bytes(value, label)
        .map_err(|_| format!("{label} must be serializable as bounded JSON"))?;
    if encoded.len() > max_bytes {
        return Err(format!("{label} exceeds its bounded JSON size limit"));
    }
    Ok(())
}

fn validate_v2_option(value: &Value, label: &str) -> Result<(), String> {
    let option = schema_object(value, label, &["value", "label", "description", "disabled"], &["value", "label"])?;
    schema_text(option.get("value").unwrap_or(&Value::Null), &format!("{label}.value"), 256, true)?;
    schema_text(option.get("label").unwrap_or(&Value::Null), &format!("{label}.label"), 256, false)?;
    schema_optional_text(option, "description", label, 1000)?;
    schema_bool(option, "disabled", label)
}

fn validate_v2_config(value: &Value) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Err("plugin.json.config must be a bounded object".to_owned());
    };
    if fields.len() > 256 {
        return Err("plugin.json.config must be a bounded object".to_owned());
    }
    for (key, raw_field) in fields {
        if !CONFIG_KEY_RE.is_match(key) {
            return Err("plugin.json.config contains an unsafe key".to_owned());
        }
        let label = format!("plugin.json.config.{key}");
        let field = schema_object(
            raw_field,
            &label,
            &["type", "label", "description", "default", "minimum", "maximum", "options"],
            &["type", "label"],
        )?;
        let field_type = field.get("type").and_then(Value::as_str).unwrap_or_default();
        if !matches!(field_type, "string" | "number" | "boolean" | "select") {
            return Err(format!("{label}.type is unsupported"));
        }
        schema_text(field.get("label").unwrap_or(&Value::Null), &format!("{label}.label"), 256, false)?;
        schema_optional_text(field, "description", &label, 1000)?;
        if let Some(default) = field.get("default") {
            match field_type {
                "string" | "select" => {
                    if !default.is_string() {
                        return Err(format!("{label}.default must be text"));
                    }
                }
                "number" => {
                    if !finite_number(default) {
                        return Err(format!("{label}.default must be a finite number"));
                    }
                }
                "boolean" => {
                    if !default.is_boolean() {
                        return Err(format!("{label}.default must be a boolean"));
                    }
                }
                _ => {}
            }
        }
        for bound in ["minimum", "maximum"] {
            schema_number(field, bound, &label)?;
        }
        if let (Some(minimum), Some(maximum)) = (field.get("minimum"), field.get("maximum")) {
            if minimum.as_f64() > maximum.as_f64() {
                return Err(format!("{label}.minimum must not exceed maximum"));
            }
        }
        if let Some(options) = field.get("options") {
            let options = schema_array(options, &format!("{label}.options"), 128)?;
            let mut seen: HashSet<&str> = HashSet::new();
            for option in options {
                validate_v2_option(option, &format!("{label}.options[]"))?;
                if let Some(value) = option.get("value").and_then(Value::as_str) {
                    if !seen.insert(value) {
                        return Err(format!("{label}.options contains duplicate values"));
                    }
                }
            }
        }
    }
    Ok(())
}

struct UiBudget {
    nodes: usize,
    items: usize,
}

impl UiBudget {
    fn collect<'a>(&mut self, value: &'a Value, label: &str, maximum: usize) -> Result<&'a Vec<Value>, String> {
        let items = schema_array(value, label, maximum)?;
        self.items += items.len();
        if self.items > 512 {
            return Err("UI collection budget exceeded".to_owned());
        }
        Ok(items)
    }
}

fn validate_v2_ui(value: &Value) -> Result<(), String> {
    let ui = schema_object(value, "plugin.json.ui", &["views"], &["views"])?;
    let views = schema_array(ui.get("views").unwrap_or(&Value::Null), "plugin.json.ui.views", 128)?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut budget = UiBudget { nodes: 0, items: 0 };
    for raw_view in views {
        let view = schema_object(
            raw_view,
            "plugin.json.ui.views[]",
            &["id", "slot", "title", "description", "icon", "order", "root"],
            &["id", "slot", "title", "root"],
        )?;
        let view_id = schema_identifier(view.get("id").unwrap_or(&Value::Null), "plugin.json.ui.views[].id")?.to_owned();
        if !seen.insert(view_id.clone()) {
            return Err("plugin.json.ui.views contains duplicate ids".to_owned());
        }
        let slot = view.get("slot").and_then(Value::as_str).unwrap_or_default();
        if !view.get("slot").map(Value::is_string).unwrap_or(false) || !V2_UI_SLOTS.contains(&slot) {
            return Err("plugin.json.ui.views[].slot is unsupported".to_owned());
        }
        schema_text(view.get("title").unwrap_or(&Value::Null), "plugin.json.ui.views[].title", 160, false)?;
        schema_optional_text(view, "description", "plugin.json.ui.views[]", 2000)?;
        schema_optional_text(view, "icon", "plugin.json.ui.views[]", 64)?;
        schema_number(view, "order", "plugin.json.ui.views[]")?;
        validate_v2_ui_node(view.get("root").unwrap_or(&Value::Null), "plugin.json.ui.views[].root", 0, &mut budget)?;
    }
    Ok(())
}

fn validate_v2_ui_node(value: &Value, label: &str, depth: usize, budget: &mut UiBudget) -> Result<(), String> {
    budget.nodes += 1;
    if depth > 10 || budget.nodes > 512 {
        return Err("plugin.json UI tree exceeds its depth or node limit".to_owned());
    }
    if !value.is_object() || !value.get("type").map(Value::is_string).unwrap_or(false) {
        return Err(format!("{label} must be a plugin UI node"));
    }
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    let (allowed, required): (&[&str], &[&str]) = match kind {
        "form" => (
            &["type", "id", "fields", "submitAction"],
            &["type", "id", "fields", "submitAction"],
        ),
        "tabs" => (&["type", "id", "tabs"], &["type", "id", "tabs"]),
        "accordion" => (&["type", "id", "sections"], &["type", "id", "sections"]),
        "table" => (&["type", "id", "columns", "rows"], &["type", "id", "columns", "rows"]),
        "chart" => (&["type", "id", "label", "values"], &["type", "id", "values"]),
        "include" => (&["type", "id", "fragment"], &["type", "id", "fragment"]),
        "text" => (&["type", "id", "text", "tone"], &["type", "text"]),
        "badge" => (&["type", "id", "label", "tone"], &["type", "label"]),
        "button" => (&["type", "id", "label", "action", "disabled", "tone"], &["type", "id", "label", "action"]),
        "input" => (
            &["type", "id", "label", "value", "placeholder", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "textarea" => (
            &["type", "id", "label", "value", "placeholder", "rows", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "number" => (
            &["type", "id", "label", "value", "min", "max", "step", "unit", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "slider" => (
            &["type", "id", "label", "value", "min", "max", "step", "unit", "config_key", "action", "onChange"],
            &["type", "id", "label", "min", "max"],
        ),
        "color" => (&["type", "id", "label", "value", "config_key", "onChange"], &["type", "id", "label"]),
        "datetime" => (
            &["type", "id", "label", "value", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "select" => (
            &["type", "id", "label", "options", "value", "config_key", "action", "onChange"],
            &["type", "id", "label", "options"],
        ),
        "checkbox" => (&["type", "id", "label", "checked", "config_key", "action", "onChange"], &["type", "id", "label"]),
        "switch" => (
            &["type", "id", "label", "checked", "hint", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "progress" => (&["type", "id", "value", "max", "label"], &["type", "value"]),
        "metric" => (&["type", "id", "label", "value", "unit", "tone"], &["type", "label", "value"]),
        "alert" => (&["type", "id", "title", "text", "tone"], &["type", "text"]),
        "code" => (&["type", "id", "text"], &["type", "text"]),
        "image" => (&["type", "id", "url", "alt", "height"], &["type", "url"]),
        "list" => (&["type", "id", "items"], &["type", "items"]),
        "link" => (&["type", "id", "label", "url", "action"], &["type", "label", "url"]),
        "divider" => (&["type", "id"], &["type"]),
        "group" => (&["type", "id", "label", "direction", "children"], &["type", "children"]),
        "card" => (&["type", "id", "label", "tone", "children"], &["type", "children"]),
        "grid" => (&["type", "id", "columns", "children"], &["type", "children"]),
        _ => return Err(format!("{label}.type is unsupported")),
    };
    let node = schema_object(value, label, allowed, required)?;
    for field in ["id", "action", "config_key", "onChange"] {
        if let Some(item) = node.get(field) {
            schema_identifier(item, &format!("{label}.{field}"))?;
        }
    }
    for field in ["label", "text", "placeholder", "url", "title", "hint", "unit", "alt"] {
        if let Some(item) = node.get(field) {
            schema_text(item, &format!("{label}.{field}"), 4096, matches!(field, "value" | "placeholder"))?;
        }
    }
    if matches!(kind, "input" | "select" | "textarea" | "color" | "datetime") {
        if let Some(item) = node.get("value") {
            schema_text(item, &format!("{label}.value"), 4096, true)?;
        }
    }
    if kind == "color" {
        if let Some(item) = node.get("value") {
            if !COLOR_RE.is_match(&py_str(item)) {
                return Err(format!("{label}.value must be #rrggbb"));
            }
        }
    }
    if kind == "image" {
        if let Some(item) = node.get("url") {
            schema_text(item, &format!("{label}.url"), 4096, false)?;
        }
    }
    if let Some(tone) = node.get("tone") {
        let value = tone.as_str().unwrap_or_default();
        if !tone.is_string() || !V2_UI_TONES.contains(&value) {
            return Err(format!("{label}.tone is unsupported"));
        }
    }
    for field in ["disabled", "checked"] {
        schema_bool(node, field, label)?;
    }
    if kind == "progress" {
        for field in ["value", "max"] {
            schema_number(node, field, label)?;
        }
    }
    if matches!(kind, "number" | "slider") {
        for field in ["value", "min", "max", "step"] {
            schema_number(node, field, label)?;
        }
        if let (Some(minimum), Some(maximum)) = (node.get("min"), node.get("max")) {
            if minimum.as_f64().unwrap_or(f64::NAN) > maximum.as_f64().unwrap_or(f64::NAN) {
                return Err(format!("{label}.min exceeds {label}.max"));
            }
        }
    }
    if kind == "textarea" {
        if let Some(rows) = node.get("rows") {
            schema_number(node, "rows", label)?;
            let value = rows.as_f64().unwrap_or(f64::NAN);
            if !(1.0..=24.0).contains(&value) {
                return Err(format!("{label}.rows is out of range"));
            }
        }
    }
    if kind == "image" {
        if let Some(height) = node.get("height") {
            schema_number(node, "height", label)?;
            let value = height.as_f64().unwrap_or(f64::NAN);
            if !(16.0..=720.0).contains(&value) {
                return Err(format!("{label}.height is out of range"));
            }
        }
    }
    if kind == "grid" {
        if let Some(columns) = node.get("columns") {
            schema_number(node, "columns", label)?;
            let value = columns.as_f64().unwrap_or(f64::NAN);
            if value.fract() != 0.0 || !(1..=4).contains(&(value as i64)) {
                return Err(format!("{label}.columns is out of range"));
            }
        }
    }
    if kind == "include" {
        schema_identifier(node.get("fragment").unwrap_or(&Value::Null), &format!("{label}.fragment"))?;
    }
    if kind == "form" {
        schema_identifier(node.get("submitAction").unwrap_or(&Value::Null), &format!("{label}.submitAction"))?;
        for child in budget.collect(node.get("fields").unwrap_or(&Value::Null), &format!("{label}.fields"), 512)?.clone() {
            validate_v2_ui_node(&child, &format!("{label}.fields[]"), depth + 1, budget)?;
        }
    }
    if matches!(kind, "tabs" | "accordion") {
        let key = if kind == "tabs" { "tabs" } else { "sections" };
        let mut ids: HashSet<String> = HashSet::new();
        for entry in budget
            .collect(node.get(key).unwrap_or(&Value::Null), &format!("{label}.{key}"), 64)?
            .clone()
        {
            let fields = schema_object(&entry, label, &["id", "label", "content"], &["id", "label", "content"])?;
            let identifier = schema_identifier(fields.get("id").unwrap_or(&Value::Null), label)?.to_owned();
            if !ids.insert(identifier) {
                return Err("Duplicate UI tab/section id".to_owned());
            }
            schema_text(fields.get("label").unwrap_or(&Value::Null), label, 256, false)?;
            validate_v2_ui_node(fields.get("content").unwrap_or(&Value::Null), label, depth + 1, budget)?;
        }
    }
    if kind == "table" {
        for column in budget.collect(node.get("columns").unwrap_or(&Value::Null), label, 32)?.clone() {
            let fields = schema_object(&column, label, &["key", "label"], &["key", "label"])?;
            schema_identifier(fields.get("key").unwrap_or(&Value::Null), label)?;
            schema_text(fields.get("label").unwrap_or(&Value::Null), label, 256, false)?;
        }
        for row in budget.collect(node.get("rows").unwrap_or(&Value::Null), label, 512)?.clone() {
            let Some(cells) = row.as_object() else {
                return Err("Invalid table row".to_owned());
            };
            if cells.len() > 32 {
                return Err("Invalid table row".to_owned());
            }
            for cell in cells.values() {
                if cell.is_string() {
                    schema_text(cell, label, 4096, true)?;
                } else if !finite_number(cell) {
                    return Err("Invalid table cell".to_owned());
                }
            }
        }
    }
    if kind == "chart" {
        for item in budget.collect(node.get("values").unwrap_or(&Value::Null), label, 512)?.clone() {
            if !finite_number(&item) {
                return Err("Invalid chart value".to_owned());
            }
        }
    }
    if kind == "select" {
        let options = budget.collect(node.get("options").unwrap_or(&Value::Null), &format!("{label}.options"), 128)?;
        if options.is_empty() {
            return Err(format!("{label}.options must not be empty"));
        }
        for option in options.clone() {
            validate_v2_option(&option, &format!("{label}.options[]"))?;
        }
    }
    if kind == "list" {
        for item in budget.collect(node.get("items").unwrap_or(&Value::Null), &format!("{label}.items"), 512)?.clone() {
            if item.is_string() {
                schema_text(&item, &format!("{label}.items[]"), 4096, true)?;
                continue;
            }
            let record =
                schema_object(&item, &format!("{label}.items[]"), &["id", "label", "detail", "badge", "action"], &["id", "label"])?;
            schema_identifier(record.get("id").unwrap_or(&Value::Null), &format!("{label}.items[].id"))?;
            schema_text(record.get("label").unwrap_or(&Value::Null), &format!("{label}.items[].label"), 256, false)?;
            for field in ["detail", "badge", "action"] {
                schema_optional_text(record, field, &format!("{label}.items[]"), 512)?;
            }
        }
    }
    if kind == "group" {
        if let Some(item) = node.get("label") {
            schema_text(item, &format!("{label}.label"), 256, true)?;
        }
        if let Some(direction) = node.get("direction") {
            let value = direction.as_str().unwrap_or_default();
            if !direction.is_string() || !V2_UI_DIRECTIONS.contains(&value) {
                return Err(format!("{label}.direction is unsupported"));
            }
        }
        for child in budget
            .collect(node.get("children").unwrap_or(&Value::Null), &format!("{label}.children"), 512)?
            .clone()
        {
            validate_v2_ui_node(&child, &format!("{label}.children[]"), depth + 1, budget)?;
        }
    }
    if matches!(kind, "card" | "grid") {
        if kind == "card" {
            if let Some(item) = node.get("label") {
                schema_text(item, &format!("{label}.label"), 256, true)?;
            }
        }
        for child in budget
            .collect(node.get("children").unwrap_or(&Value::Null), &format!("{label}.children"), 512)?
            .clone()
        {
            validate_v2_ui_node(&child, &format!("{label}.children[]"), depth + 1, budget)?;
        }
    }
    Ok(())
}

fn validate_v2_i18n(value: &Value) -> Result<(), String> {
    let i18n = schema_object(
        value,
        "plugin.json.i18n",
        &["default_locale", "messages"],
        &["default_locale", "messages"],
    )?;
    schema_text(
        i18n.get("default_locale").unwrap_or(&Value::Null),
        "plugin.json.i18n.default_locale",
        35,
        false,
    )?;
    let Some(messages) = i18n.get("messages").and_then(Value::as_object) else {
        return Err("plugin.json.i18n.messages must be a bounded object".to_owned());
    };
    if messages.len() > 32 {
        return Err("plugin.json.i18n.messages must be a bounded object".to_owned());
    }
    for (locale, raw_table) in messages {
        schema_text(&json!(locale), "plugin.json.i18n.locale", 35, false)?;
        let Some(table) = raw_table.as_object() else {
            return Err("plugin.json.i18n locale table must be a bounded object".to_owned());
        };
        if table.len() > 2048 {
            return Err("plugin.json.i18n locale table must be a bounded object".to_owned());
        }
        for (key, text) in table {
            schema_text(&json!(key), "plugin.json.i18n message key", 256, false)?;
            schema_text(text, "plugin.json.i18n message", 4096, true)?;
        }
    }
    Ok(())
}

fn validate_v2_theme(value: &Value) -> Result<(), String> {
    let theme = schema_object(value, "plugin.json.theme", &["name", "colors", "tokens"], &[])?;
    if let Some(name) = theme.get("name") {
        schema_text(name, "plugin.json.theme.name", 120, false)?;
    }
    for (field, allow_number) in [("colors", false), ("tokens", true)] {
        let Some(mapping) = theme.get(field).and_then(Value::as_object) else {
            continue;
        };
        if mapping.len() > 256 {
            return Err(format!("plugin.json.theme.{field} must be a bounded object"));
        }
        for (key, item) in mapping {
            schema_identifier(&json!(key), &format!("plugin.json.theme.{field} key"))?;
            if item.is_string() {
                schema_text(item, &format!("plugin.json.theme.{field}.{key}"), 256, true)?;
            } else if !(allow_number && finite_number(item)) {
                return Err(format!("plugin.json.theme.{field}.{key} has an unsupported value"));
            }
        }
    }
    Ok(())
}

fn validate_v2_named_records(
    value: &[Value],
    label: &str,
    allowed: &[&str],
    required: &[&str],
) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    for item in value {
        let record = schema_object(item, label, allowed, required)?;
        let identifier = schema_identifier(record.get("id").unwrap_or(&Value::Null), &format!("{label}.id"))?.to_owned();
        if !seen.insert(identifier) {
            return Err(format!("{label} contains duplicate ids"));
        }
        schema_text(record.get("label").unwrap_or(&Value::Null), &format!("{label}.label"), 256, false)?;
        for field in required.iter().filter(|field| **field != "id" && **field != "label") {
            schema_text(
                record.get(*field).unwrap_or(&Value::Null),
                &format!("{label}.{field}"),
                256,
                false,
            )?;
        }
        for field in allowed.iter().filter(|field| !required.contains(field) && **field != "id") {
            schema_optional_text(record, field, label, 1000)?;
        }
    }
    Ok(())
}

fn validate_v2_commands(value: &Value) -> Result<(), String> {
    let commands = schema_array(value, "plugin.json.commands", 256)?;
    let mut seen: HashSet<String> = HashSet::new();
    for raw_command in commands {
        let command = schema_object(
            raw_command,
            "plugin.json.commands[]",
            &["id", "label", "description", "arguments"],
            &["id", "label"],
        )?;
        let identifier =
            schema_identifier(command.get("id").unwrap_or(&Value::Null), "plugin.json.commands[].id")?.to_owned();
        if !seen.insert(identifier) {
            return Err("plugin.json.commands contains duplicate ids".to_owned());
        }
        schema_text(command.get("label").unwrap_or(&Value::Null), "plugin.json.commands[].label", 256, false)?;
        schema_optional_text(command, "description", "plugin.json.commands[]", 1000)?;
        if let Some(arguments) = command.get("arguments") {
            let args = schema_array(arguments, "plugin.json.commands[].arguments", 64)?;
            let mut names: HashSet<String> = HashSet::new();
            for raw_arg in args {
                let arg = schema_object(
                    raw_arg,
                    "plugin.json.commands[].arguments[]",
                    &["name", "description", "required", "choices"],
                    &["name"],
                )?;
                let name =
                    schema_identifier(arg.get("name").unwrap_or(&Value::Null), "plugin.json.commands[].arguments[].name")?
                        .to_owned();
                if !names.insert(name) {
                    return Err("plugin.json command arguments contain duplicate names".to_owned());
                }
                schema_optional_text(arg, "description", "plugin.json.commands[].arguments[]", 1000)?;
                schema_bool(arg, "required", "plugin.json.commands[].arguments[]")?;
                if let Some(choices) = arg.get("choices") {
                    for choice in schema_array(choices, "plugin.json.commands[].arguments[].choices", 256)? {
                        schema_text(choice, "plugin.json command choice", 256, false)?;
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------- разрешения плагинов ----------------------

fn read_permission_state(path: &Path) -> Result<Value, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"schema_version": PLUGIN_PERMISSION_SCHEMA, "plugins": {}}));
        }
        Err(_) => return Err("Could not inspect plugin permission grants".to_owned()),
    };
    if !metadata.is_file() || metadata.len() > MAX_PERMISSION_STATE_BYTES as u64 {
        return Err("Plugin permission grants must be a bounded regular file".to_owned());
    }
    let raw = read_private_file(path, MAX_PERMISSION_STATE_BYTES)
        .map_err(|_| "Could not securely read plugin permission grants".to_owned())?;
    let Some(raw) = raw else {
        return Ok(json!({"schema_version": PLUGIN_PERMISSION_SCHEMA, "plugins": {}}));
    };
    if raw.len() > MAX_PERMISSION_STATE_BYTES {
        return Err("Plugin permission grants file is too large".to_owned());
    }
    let state = parse_strict_json(&raw, "Plugin permission grants")
        .map_err(|_| "Plugin permission grants contain invalid JSON".to_owned())?;
    validate_permission_state(&state)?;
    Ok(state)
}

fn validate_permission_state(value: &Value) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Err("Plugin permission grants have an unsupported format".to_owned());
    };
    if fields.len() != 2 || !fields.contains_key("schema_version") || !fields.contains_key("plugins") {
        return Err("Plugin permission grants have an unsupported format".to_owned());
    }
    if fields.get("schema_version").and_then(Value::as_i64) != Some(PLUGIN_PERMISSION_SCHEMA) {
        return Err("Plugin permission grants have an unsupported schema version".to_owned());
    }
    let Some(plugins) = fields.get("plugins").and_then(Value::as_object) else {
        return Err("Plugin permission grants plugins must be an object".to_owned());
    };
    for (plugin_id, record) in plugins {
        if !PLUGIN_ID_RE.is_match(plugin_id) {
            return Err("Plugin permission grants contain an unsafe plugin id".to_owned());
        }
        let Some(record) = record.as_object() else {
            return Err("Plugin permission grants contain an invalid record".to_owned());
        };
        if record.len() != 2 || !record.contains_key("grants") || !record.contains_key("untrusted_execution_hash") {
            return Err("Plugin permission grants contain an invalid record".to_owned());
        }
        let grants = record.get("grants").cloned().unwrap_or(Value::Null);
        let Some(grants) = grants.as_array() else {
            return Err("Plugin permission grants contain an invalid permission".to_owned());
        };
        for grant in grants {
            let Some(name) = grant.as_str() else {
                return Err("Plugin permission grants contain an invalid permission".to_owned());
            };
            if !PLUGIN_PERMISSION_NAMES.contains(name) {
                return Err("Plugin permission grants contain an invalid permission".to_owned());
            }
        }
        let unique: HashSet<&str> = grants.iter().filter_map(Value::as_str).collect();
        if unique.len() != grants.len() {
            return Err("Plugin permission grants contain duplicate permissions".to_owned());
        }
        let package_hash = record.get("untrusted_execution_hash").cloned().unwrap_or(Value::Null);
        let Some(package_hash) = package_hash.as_str() else {
            return Err("Plugin permission grants contain an invalid local execution hash".to_owned());
        };
        if !package_hash.is_empty() && !HEX64_RE.is_match(package_hash) {
            return Err("Plugin permission grants contain an invalid local execution hash".to_owned());
        }
    }
    Ok(())
}

fn write_permission_state(path: &Path, value: &Value) -> Result<(), String> {
    validate_permission_state(value)?;
    if path_is_symlink(path) {
        return Err("Plugin permission grants must not be a symbolic link".to_owned());
    }
    atomic_json_write(path, value, true).map_err(|_| "Could not save plugin permission grants".to_owned())
}

/// Чтение с O_NOFOLLOW и fchmod 0600 — замена set_private_fd из platform_support.
fn read_private_file(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Could not securely read plugin permission grants".to_owned())?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.kind() {
                std::io::ErrorKind::NotFound => Ok(None),
                _ => Err("Could not securely read plugin permission grants".to_owned()),
            };
        }
        if unsafe { libc::fchmod(fd, 0o600) } != 0 {
            unsafe { libc::close(fd) };
            return Err("Could not securely read plugin permission grants".to_owned());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let mut buffer = Vec::new();
        let outcome = file.take(max_bytes as u64 + 1).read_to_end(&mut buffer);
        match outcome {
            Ok(_) if buffer.len() <= max_bytes => Ok(Some(buffer)),
            Ok(_) => Err("Plugin permission grants file is too large".to_owned()),
            Err(_) => Err("Could not securely read plugin permission grants".to_owned()),
        }
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(true) {
            return Err("Could not securely read plugin permission grants".to_owned());
        }
        match fs::read(path) {
            Ok(buffer) if buffer.len() <= max_bytes => Ok(Some(buffer)),
            Ok(_) => Err("Plugin permission grants file is too large".to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Could not securely read plugin permission grants".to_owned()),
        }
    }
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

// ---------------------- пакеты plugin.json ----------------------

fn parse_v2_manifest_bytes(content: &[u8]) -> Result<Value, String> {
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

struct V2Package {
    manifest: Value,
    files: BTreeMap<String, Vec<u8>>,
    signed: bool,
    key_id: Option<String>,
    public_key: Option<String>,
    fingerprint: Option<String>,
    package_digest: String,
}

fn read_v2_package(root: &Path, expected_id: Option<&str>) -> Result<V2Package, String> {
    if path_is_symlink(root) || !path_is_dir(root) {
        return Err("Plugin package root must be a real directory".to_owned());
    }
    if path_exists(&root.join(PLUGIN_MANIFEST)) || path_is_symlink(&root.join(PLUGIN_MANIFEST)) {
        return Err("Plugin package cannot mix plugin.yml and plugin.json".to_owned());
    }
    let (file_bytes, manifest_bytes) = collect_v2_files(root)?;
    let raw_manifest = parse_v2_manifest_bytes(&manifest_bytes)?;
    let manifest = validate_plugin_json_manifest(&raw_manifest)?;
    if let Some(expected) = expected_id {
        if manifest.get("id").and_then(Value::as_str) != Some(expected) {
            return Err("plugin.json id does not match its plugin directory".to_owned());
        }
    }
    let entrypoint = manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
    if !file_bytes.contains_key(&entrypoint) {
        return Err("Plugin package is missing its entrypoint".to_owned());
    }

    let signed = raw_manifest.as_object().map(|fields| fields.contains_key("signature")).unwrap_or(false);
    let (key_id, public_key, fingerprint): (Option<String>, Option<String>, Option<String>);
    if signed {
        let verification = super::plugin_security::verify_manifest(&raw_manifest, &file_bytes);
        let verified = verification.get("signed") == Some(&json!(true))
            && verification.get("valid") == Some(&json!(true));
        if !verified {
            let reason = verification
                .get("error")
                .and_then(Value::as_str)
                .filter(|error| !error.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| "invalid package".to_owned());
            return Err(format!("Plugin signature or integrity verification failed: {reason}"));
        }
        key_id = verification.get("key_id").and_then(Value::as_str).map(str::to_owned);
        public_key = verification.get("public_key").and_then(Value::as_str).map(str::to_owned);
        fingerprint = verification.get("fingerprint").and_then(Value::as_str).map(str::to_owned);
    } else {
        verify_unsigned_v2_integrity(&raw_manifest, &file_bytes)?;
        key_id = None;
        public_key = None;
        fingerprint = None;
    }

    let canonical = canonical_json_bytes(&raw_manifest, "plugin.json")
        .map_err(|_| "plugin.json cannot be encoded as canonical JSON".to_owned())?;
    Ok(V2Package {
        manifest,
        files: file_bytes,
        signed,
        key_id,
        public_key,
        fingerprint,
        package_digest: sha256_hex(&canonical),
    })
}

fn collect_v2_files(root: &Path) -> Result<(BTreeMap<String, Vec<u8>>, Vec<u8>), String> {
    let manifest_path = root.join(PLUGIN_V2_MANIFEST);
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|_| "Plugin package must contain a root plugin.json".to_owned())?;
    if !metadata.is_file() || metadata.len() > PLUGIN_MAX_MANIFEST_BYTES as u64 {
        return Err("plugin.json must be a bounded regular file".to_owned());
    }
    let manifest_bytes = read_regular_package_file(&manifest_path, PLUGIN_MAX_MANIFEST_BYTES)?;

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total_size = manifest_bytes.len() as u64;
    let mut file_count: usize = 1;
    walk_v2_package(root, root, &mut files, &mut total_size, &mut file_count)?;
    Ok((files, manifest_bytes))
}

fn relative_posix(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

fn walk_v2_package(
    directory: &Path,
    root: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    total_size: &mut u64,
    file_count: &mut usize,
) -> Result<(), String> {
    let inspect = |error: std::io::Error| format!("Could not inspect plugin package files: {error}");
    let mut entries: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
    for entry in fs::read_dir(directory).map_err(inspect)? {
        let entry = entry.map_err(inspect)?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // os.walk делит записи по stat-following; symlink-каталоги попадают в каталоги
    // и ловятся lstat-проверкой ниже.
    let mut kept_directories: Vec<PathBuf> = Vec::new();
    for (name, path) in &entries {
        let follows_dir = fs::metadata(path).map(|metadata| metadata.is_dir()).unwrap_or(false);
        if !follows_dir {
            continue;
        }
        let lstat = fs::symlink_metadata(path).map_err(inspect)?;
        if lstat.is_symlink() {
            return Err(format!("Plugin package contains a symbolic link: {}", relative_posix(path, root)));
        }
        if !lstat.is_dir() {
            return Err(format!("Plugin package contains a non-directory path: {}", relative_posix(path, root)));
        }
        let relative = v2_package_path(&relative_posix(path, root))?;
        let name_text = name.to_string_lossy();
        if relative.split('/').count() == 1 && V2_DATA_DIRECTORIES.contains(&name_text.as_ref()) {
            continue;
        }
        if relative == ".enabled" {
            return Err("plugin.json package uses a reserved runtime path".to_owned());
        }
        kept_directories.push(path.clone());
    }

    for (name, path) in &entries {
        let _ = name;
        let follows_dir = fs::metadata(path).map(|metadata| metadata.is_dir()).unwrap_or(false);
        if follows_dir {
            continue;
        }
        let lstat = fs::symlink_metadata(path).map_err(inspect)?;
        let relative = v2_package_path(&relative_posix(path, root))?;
        if lstat.is_symlink() {
            return Err(format!("Plugin package contains a symbolic link: {relative}"));
        }
        if !lstat.is_file() {
            return Err(format!("Plugin package contains a non-regular file: {relative}"));
        }
        if relative == PLUGIN_V2_MANIFEST {
            continue;
        }
        if relative == ".enabled" {
            if lstat.len() != 0 {
                return Err("Plugin enabled marker is malformed".to_owned());
            }
            continue;
        }
        if relative == PLUGIN_MANIFEST {
            return Err("Plugin package cannot mix plugin.yml and plugin.json".to_owned());
        }
        let remaining = (PLUGIN_MAX_UNPACKED_BYTES as u64).saturating_sub(*total_size);
        if lstat.len() > remaining {
            return Err("Plugin package exceeds the unpacked size limit".to_owned());
        }
        let content = read_regular_package_file(path, remaining as usize)?;
        *total_size += content.len() as u64;
        *file_count += 1;
        if *file_count > PLUGIN_MAX_FILES {
            return Err("Plugin package contains too many files".to_owned());
        }
        files.insert(relative, content);
    }

    for path in kept_directories {
        walk_v2_package(&path, root, files, total_size, file_count)?;
    }
    Ok(())
}

fn read_regular_package_file(path: &Path, size_limit: usize) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Could not securely read plugin package file".to_owned())?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err("Could not securely read plugin package file".to_owned());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        // fstat до чтения — как os.fstat в Python.
        let metadata = file
            .metadata()
            .map_err(|_| "Could not securely read plugin package file".to_owned())?;
        if !metadata.is_file() || metadata.len() > size_limit as u64 {
            return Err("Plugin package file is not a bounded regular file".to_owned());
        }
        let mut buffer = Vec::new();
        if file
            .take(size_limit as u64 + 1)
            .read_to_end(&mut buffer)
            .is_err()
        {
            return Err("Could not securely read plugin package file".to_owned());
        }
        if buffer.len() > size_limit || buffer.len() as u64 != metadata.len() {
            return Err("Plugin package file changed while it was being read".to_owned());
        }
        Ok(buffer)
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(path).map_err(|_| "Could not securely read plugin package file".to_owned())?;
        if metadata.is_symlink() || !metadata.is_file() || metadata.len() > size_limit as u64 {
            return Err("Plugin package file is not a bounded regular file".to_owned());
        }
        fs::read(path).map_err(|_| "Could not securely read plugin package file".to_owned())
    }
}

fn v2_package_path(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.contains('\\')
        || value.contains('\0')
        || value.starts_with('/')
        || WINDOWS_DRIVE_RE.is_match(value)
    {
        return Err("Plugin package contains an unsafe path".to_owned());
    }
    if value.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("Plugin package contains an unsafe path".to_owned());
    }
    Ok(value.to_owned())
}

fn reject_v2_runtime_paths(root: &Path) -> Result<(), String> {
    for name in ["storage", "files", "config", "database", ".enabled"] {
        let child = root.join(name);
        if path_exists(&child) || path_is_symlink(&child) {
            return Err(format!("Plugin package uses a reserved runtime path: {name}"));
        }
    }
    Ok(())
}

fn verify_unsigned_v2_integrity(manifest: &Value, file_bytes: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let integrity = manifest.get("integrity");
    let valid_shape = integrity
        .and_then(Value::as_object)
        .map(|fields| fields.len() == 1 && fields.contains_key("files"))
        .unwrap_or(false)
        && integrity
            .and_then(|value| value.get("files"))
            .map(Value::is_object)
            .unwrap_or(false);
    if !valid_shape {
        return Err("Unsigned plugin.json must include integrity.files".to_owned());
    }
    let Some(expected) = integrity.and_then(|value| value.get("files")).and_then(Value::as_object) else {
        return Err("Unsigned plugin.json must include integrity.files".to_owned());
    };
    if expected.len() != file_bytes.len() || file_bytes.keys().any(|path| !expected.contains_key(path)) {
        return Err("Plugin integrity hashes must exactly cover every package file except plugin.json".to_owned());
    }
    for (path, content) in file_bytes {
        let Some(digest) = expected.get(path).and_then(Value::as_str) else {
            return Err(format!("Plugin integrity hash is invalid for {path}"));
        };
        if !HEX64_RE.is_match(digest) {
            return Err(format!("Plugin integrity hash is invalid for {path}"));
        }
        if digest != sha256_hex(content) {
            return Err(format!("Plugin file hash mismatch: {path}"));
        }
    }
    Ok(())
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

// ---------------------- пути и файловые операции ----------------------

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

/// shutil.move: если назначение — существующий каталог, источник уходит внутрь.
fn move_path(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    if fs::metadata(destination).map(|metadata| metadata.is_dir()).unwrap_or(false) {
        if let Some(name) = source.file_name() {
            return fs::rename(source, destination.join(name));
        }
    }
    fs::rename(source, destination)
}

/// rglob("*") из Python: symlink не разворачиваем и не отдаём.
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
    // Компоненты уже проверены lstat-ом, поэтому лексический путь равен canonical.
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
    // Path.resolve() в Python не строгий: несуществующие компоненты достраиваются
    // лексически (staging/storage ещё не создан на момент проверки).
    let root_resolved = resolve_non_strict(root);
    let parent_resolved = resolve_non_strict(candidate.parent().unwrap_or(root));
    if !within(&parent_resolved, &root_resolved) {
        return Err(InstallFail::User(
            "Архив плагина выходит за пределы staging-каталога".to_owned(),
        ));
    }
    Ok(candidate)
}

// Аналог Path.resolve(strict=False): существующие компоненты разворачиваются
// через canonicalize (включая symlinks), остальные достраиваются лексически.
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

// ---------------------- config.yml ----------------------

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
                // str(float) из Python даёт «2.0», а не «2».
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

// ---------------------- ошибки установки ----------------------

enum InstallFail {
    /// LauncherError из Python — текст передаётся пользователю как есть.
    User(String),
    /// OSError/BadZipFile — обёртка «Не удалось установить плагин: …».
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
    use base64::Engine as _;
    use ed25519_dalek::{SigningKey, Signer};

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
            data_home: data_home.clone(),
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

    // ---------------------- plugin.yml ----------------------

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
        // Комментарий за значением отбрасывается.
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

    // ---------------------- validate_manifest ----------------------

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
            // entrypoint проверяется до permissions/network_hosts/sidebar_tab — как в Python.
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

    // ---------------------- схема v2/v3 ----------------------

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
        // network:listen вырезан из v2, но остался в union-наборе legacy.
        assert!(!PLUGIN_V2_PERMISSION_NAMES.contains("network:listen"));
        assert!(PLUGIN_PERMISSION_NAMES.contains("network:listen"));
        assert!(PLUGIN_V3_PERMISSION_NAMES.contains("crypto:sign"));
        assert!(PLUGIN_V3_EVENTS.len() > PLUGIN_V2_EVENTS.len());
    }

    #[test]
    #[ignore = "сверить ожидания с plugins.py после релиза"]
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

        // Глубина дерева ограничена десятью уровнями.
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

    // ---------------------- store ----------------------

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
    #[ignore = "сверить ожидания с plugins.py после релиза"]
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
    #[ignore = "сверить ожидания с plugins.py после релиза"]
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

        // Без согласия даже включить нельзя.
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
        // В списке плагинов digest не отдаётся — только в runtime_source/runtime_info.
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

        // Подменённое содержимое файла при исходной подписи не пройдёт integrity:
        // манифест не переподписываем — hash файла в integrity перестаёт совпадать.
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
        // storage:workspace исчез из манифеста — grant срезается.
        assert_eq!(replaced["granted_permissions"], json!(["links:open"]));
        assert_eq!(store.read_storage("demo-plugin", "state").unwrap(), json!({"keep": true}));
        assert!(!store.root.join("demo-plugin").join(".install-").exists());

        // Повторная установка без replace отклоняется.
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
        // api_version 1 — статический контракт plugin.yml без полей исполнения.
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
