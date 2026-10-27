//! файловые операции сервера; пути остаются внутри экземпляра, служебные файлы недоступны
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::{json, Map, Value};

use super::config::ConfigStore;
use super::instances::{InstanceInfo, InstanceStore};
use super::portable_fs::{atomic_write, regular_file, remove_tree, safe_path};
use super::server::{
    normalize_server_config, save_protected_settings, server_config, server_instance, stores,
    write_server_config, InstanceLock, SERVER_CONFIG_FILE,
};
use super::snapshots::SnapshotStore;

const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;
const MAX_REMOVE_ENTRIES: usize = 100_000;
const MAX_REMOVE_DEPTH: usize = 128;
const LIST_ENTRY_LIMIT: usize = 1000;

fn reserved_name(name: &str) -> bool {
    name.starts_with(".trial-launcher") || name.starts_with(".nivora-launcher")
}

fn split_parts(raw: &str, allow_root: bool) -> Result<Vec<String>, String> {
    let value = raw.replace('\\', "/");
    if value.contains('\0') {
        return Err("Путь содержит недопустимый символ".to_owned());
    }
    if value.is_empty() && allow_root {
        return Ok(Vec::new());
    }
    let trimmed = value.trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    if value.starts_with('/')
        || parts.is_empty()
        || parts.contains(&"..")
        || parts.first().map(|part| part.len() == 2 && part.ends_with(':')).unwrap_or(false)
    {
        return Err("Путь выходит за пределы сервера".to_owned());
    }
    Ok(parts.into_iter().map(str::to_owned).collect())
}

fn as_parts(parts: &[String]) -> Vec<&str> {
    parts.iter().map(String::as_str).collect()
}

fn request_str(request: &Value, key: &str) -> String {
    request.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

struct ServerFiles {
    config: ConfigStore,
    instances: InstanceStore,
}

impl ServerFiles {
    fn open() -> Result<Self, String> {
        let (config, instances) = stores()?;
        Ok(Self { config, instances })
    }

    fn root(&self, name: &str) -> Result<InstanceInfo, String> {
        server_instance(&self.instances, name)
    }

    fn mutation_lock(&self, name: &str) -> Result<Option<InstanceLock>, String> {
        let info = self.root(name)?;
        InstanceLock::acquire(&self.config.paths.data_home, &info.name)
    }

    fn list(&self, name: &str, directory: &str) -> Result<Vec<Value>, String> {
        let info = self.root(name)?;
        let parts = split_parts(directory, true)?;
        let path = safe_path(&info.path, &as_parts(&parts)).map_err(|error| format!("Не удалось прочитать папку сервера: {error}"))?;
        if !path.is_dir() {
            return Err("Папка сервера не найдена".to_owned());
        }
        let entries = fs::read_dir(&path).map_err(|error| format!("Не удалось прочитать папку сервера: {error}"))?;
        let mut values: Vec<(bool, String, Value)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if reserved_name(&name) {
                continue;
            }
            let item = entry.path();
            let Ok(meta) = fs::symlink_metadata(&item) else { continue };
            let is_directory = meta.is_dir();
            let is_file = meta.is_file();
            if !is_directory && !is_file {
                continue;
            }
            let size = if is_file { meta.len() } else { super::instance_workspace::directory_size(&item) };
            let modified_at = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            let relative = if parts.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", parts.join("/"), name)
            };
            values.push((
                is_directory,
                name.to_lowercase(),
                json!({
                    "name": name,
                    "path": relative,
                    "kind": if is_directory { "directory" } else { "file" },
                    "size": size,
                    "modified_at": modified_at,
                    "editable": is_file && size <= MAX_TEXT_BYTES as u64,
                }),
            ));
        }
        values.sort_by(|left, right| (!left.0).cmp(&!right.0).then_with(|| left.1.cmp(&right.1)));
        Ok(values.into_iter().take(LIST_ENTRY_LIMIT).map(|(_, _, value)| value).collect())
    }

    fn read_text(&self, name: &str, path: &str) -> Result<String, String> {
        let info = self.root(name)?;
        let parts = split_parts(path, false)?;
        if parts.last().map(|value| reserved_name(value)).unwrap_or(false) {
            return Err("Служебные файлы лаунчера нельзя читать через редактор".to_owned());
        }
        let target = safe_path(&info.path, &as_parts(&parts))?;
        regular_file(&target, Some(MAX_TEXT_BYTES as u64))?;
        let content = fs::read(&target).map_err(|_| "Текстовый файл не найден или недоступен".to_owned())?;
        if content.len() > MAX_TEXT_BYTES {
            return Err("Текстовый файл не найден или слишком большой".to_owned());
        }
        String::from_utf8(content).map_err(|_| "Файл не является UTF-8 текстом".to_owned())
    }

    fn write_text(&self, name: &str, path: &str, content: &str) -> Result<(), String> {
        let _guard = self.mutation_lock(name)?;
        let info = self.root(name)?;
        let encoded = content.as_bytes();
        if encoded.len() > MAX_TEXT_BYTES {
            return Err("Текстовый файл слишком большой".to_owned());
        }
        let parts = split_parts(path, false)?;
        if parts.last().map(|value| reserved_name(value)).unwrap_or(false) {
            return Err("Служебные файлы лаунчера нельзя изменять".to_owned());
        }
        atomic_write(&info.path, &as_parts(&parts), encoded).map(|_| ()).map_err(|_| "Не удалось сохранить файл сервера".to_owned())
    }

    fn mkdir(&self, name: &str, path: &str) -> Result<(), String> {
        let _guard = self.mutation_lock(name)?;
        let info = self.root(name)?;
        let parts = split_parts(path, false)?;
        if parts.last().map(|value| reserved_name(value)).unwrap_or(false) {
            return Err("Служебные каталоги лаунчера нельзя создавать".to_owned());
        }
        let target = safe_path(&info.path, &as_parts(&parts))?;
        fs::create_dir(&target).map_err(|_| "Не удалось создать папку сервера".to_owned())
    }

    fn upload(&self, name: &str, path: &str, content: &[u8]) -> Result<(), String> {
        let _guard = self.mutation_lock(name)?;
        let info = self.root(name)?;
        if content.len() > MAX_UPLOAD_BYTES {
            return Err("Файл сервера слишком большой (максимум 256 MiB)".to_owned());
        }
        let parts = split_parts(path, false)?;
        if parts.last().map(|value| reserved_name(value)).unwrap_or(false) {
            return Err("Служебные файлы лаунчера нельзя заменять".to_owned());
        }
        atomic_write(&info.path, &as_parts(&parts), content).map(|_| ()).map_err(|_| "Не удалось загрузить файл сервера".to_owned())
    }

    fn delete(&self, name: &str, path: &str) -> Result<(), String> {
        let _guard = self.mutation_lock(name)?;
        let info = self.root(name)?;
        let parts = split_parts(path, false)?;
        if parts.last().map(|value| reserved_name(value)).unwrap_or(false) {
            return Err("Служебный путь нельзя удалить".to_owned());
        }
        remove_tree(&info.path, &as_parts(&parts), Some(&reserved_name), MAX_REMOVE_ENTRIES, MAX_REMOVE_DEPTH)
            .map_err(|_| "Не удалось удалить путь сервера".to_owned())
    }
}

pub(crate) fn server_public(info: &InstanceInfo, config: &Value) -> Value {
    let value = || config.as_object().cloned().unwrap_or_default();
    let fields = value();
    let field = |key: &str, fallback: &str| -> String {
        fields
            .get(key)
            .map(|item| item.as_str().map(str::to_owned).unwrap_or_else(|| item.to_string()))
            .unwrap_or_else(|| fallback.to_owned())
    };
    json!({
        "name": info.name,
        "display_name": if info.display_name.is_empty() { info.name.clone() } else { info.display_name.clone() },
        "description": info.description,
        "group": info.group,
        "icon": info.icon,
        "banner": info.banner,
        "played_seconds": info.played_seconds,
        "version_id": info.version_id,
        "minecraft_version": info.minecraft_version,
        "loader": info.loader,
        "path": info.path.to_string_lossy(),
        "kind": "server",
        "server_core": field("core", &info.server_core),
        "server_core_version": field("version", &info.server_core_version),
        "server_port": fields.get("port").and_then(Value::as_i64).unwrap_or(info.server_port),
        "server_eula": fields.get("eula_accepted").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub(crate) fn write_eula_file(root: &Path, accepted: bool) -> Result<(), String> {
    let content = format!("# Generated by Nivora Launcher\neula={}\n", if accepted { "true" } else { "false" });
    let target = root.join("eula.txt");
    if fs::symlink_metadata(&target).is_ok() {
        regular_file(&target, None).map_err(|_| "Файл EULA является символьной ссылкой или имеет неверный тип".to_owned())?;
    }
    atomic_write(root, &["eula.txt"], content.as_bytes()).map(|_| ()).map_err(|error| format!("Не удалось безопасно сохранить EULA: {error}"))
}

pub(crate) fn server_details(request: &Value) -> Result<Value, String> {
    let name = request_str(request, "name");
    let files = ServerFiles::open()?;
    let (config, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    let mut payload = Map::new();
    let server_config_value = server_config(&config, &instances, &name)?;
    payload.insert("server".to_owned(), server_public(&info, &server_config_value));
    payload.insert("config".to_owned(), server_config_value);
    let memory_total_kib = super::config::system_memory_kib();
    payload.insert("memory_total_kib".to_owned(), json!(memory_total_kib));
    payload.insert("memory_limit_kib".to_owned(), json!(super::server::server_memory_limit_kib(Some(memory_total_kib))));
    payload.insert("status".to_owned(), super::server::status_server_with(&config, &instances, &name)?);
    payload.insert("files".to_owned(), json!(files.list(&name, &request_str(request, "directory"))?));
    payload.insert("plugin_files".to_owned(), json!(files.list(&name, "plugins")?));
    payload.insert("logs".to_owned(), super::server::server_log_for(&instances, &info, 0)?);
    let snapshots = SnapshotStore::new(&info.path)?;
    payload.insert(
        "snapshots".to_owned(),
        json!({
            "mods": snapshots.list("mods")?,
            "plugins": snapshots.list("plugins")?,
        }),
    );
    Ok(Value::Object(payload))
}

pub(crate) fn server_save_config(request: &Value) -> Result<Value, String> {
    let name = request_str(request, "name");
    let values = request.get("values").cloned().unwrap_or(Value::Null);
    let Some(fields) = values.as_object() else {
        return Err("Настройки сервера имеют неожиданный формат".to_owned());
    };
    let (config, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    let current = server_config(&config, &instances, &name)?;
    let mut merged = current.as_object().cloned().unwrap_or_default();
    for (key, value) in fields {
        if merged.contains_key(key) {
            merged.insert(key.clone(), value.clone());
        }
    }
    let normalized = normalize_server_config(&Value::Object(merged), false)?;
    write_eula_file(&info.path, normalized.get("eula_accepted").and_then(Value::as_bool).unwrap_or(false))?;
    write_server_config(&info.path.join(SERVER_CONFIG_FILE), &normalized)?;
    let security_defaults = ["java_path", "security_profile", "security_permissions"];
    let mut subset = Map::new();
    for key in security_defaults {
        if let Some(value) = normalized.get(key) {
            subset.insert(key.to_owned(), value.clone());
        }
    }
    save_protected_settings(&config, &info.name, &Value::Object(subset))?;
    let mut profile = super::instances::profile_metadata(&info.path);
    profile.insert("kind".to_owned(), json!("server"));
    profile.insert("server_core".to_owned(), normalized.get("core").cloned().unwrap_or(json!("custom")));
    profile.insert("server_core_version".to_owned(), normalized.get("version").cloned().unwrap_or(json!("")));
    profile.insert("server_port".to_owned(), normalized.get("port").cloned().unwrap_or(json!(25565)));
    profile.insert("server_eula".to_owned(), normalized.get("eula_accepted").cloned().unwrap_or(json!(false)));
    super::net::atomic_json_write(&info.path.join(super::instances::INSTANCE_METADATA_FILE), &Value::Object(profile), true)?;

    super::events::emit_status_event("server-config-changed", "Настройки сервера сохранены.");
    let server = server_public(&info, &normalized);
    Ok(json!({ "config": normalized, "server": server }))
}

pub(crate) fn server_file_list(request: &Value) -> Result<Value, String> {
    let files = ServerFiles::open()?;
    Ok(json!({ "files": files.list(&request_str(request, "name"), &request_str(request, "directory"))? }))
}

pub(crate) fn server_file_read(request: &Value) -> Result<Value, String> {
    let files = ServerFiles::open()?;
    Ok(json!({ "content": files.read_text(&request_str(request, "name"), &request_str(request, "path"))? }))
}

pub(crate) fn server_file_write(request: &Value) -> Result<Value, String> {
    let files = ServerFiles::open()?;
    files.write_text(&request_str(request, "name"), &request_str(request, "path"), &request_str(request, "content"))?;
    Ok(json!({ "saved": true }))
}

pub(crate) fn server_file_mkdir(request: &Value) -> Result<Value, String> {
    let files = ServerFiles::open()?;
    files.mkdir(&request_str(request, "name"), &request_str(request, "path"))?;
    Ok(json!({ "created": true }))
}

pub(crate) fn server_file_upload(request: &Value) -> Result<Value, String> {
    let files = ServerFiles::open()?;
    let name = request_str(request, "name");
    let path = request_str(request, "path");
    let raw = request.get("data").and_then(Value::as_str).unwrap_or_default();
    let content = decode_upload(raw)?;
    files.upload(&name, &path, &content)?;
    Ok(json!({ "uploaded": true }))
}

pub(crate) fn server_file_delete(request: &Value) -> Result<Value, String> {
    let files = ServerFiles::open()?;
    files.delete(&request_str(request, "name"), &request_str(request, "path"))?;
    Ok(json!({ "removed": true }))
}

/// возвращает категорию снапшота
fn snapshot_category(request: &Value) -> String {
    request_str(request, "category")
}

pub(crate) fn server_snapshot_list(request: &Value) -> Result<Value, String> {
    let name = request_str(request, "name");
    let category = snapshot_category(request);
    let (_, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    let snapshots = SnapshotStore::new(&info.path)?;
    Ok(json!({ "snapshots": snapshots.list(&category)?, "category": category }))
}

pub(crate) fn server_snapshot_create(request: &Value) -> Result<Value, String> {
    let name = request_str(request, "name");
    let category = snapshot_category(request);
    let (config, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    let settings = server_config(&config, &instances, &name)?;
    let retention_hours = settings.get("snapshot_retention_hours").and_then(Value::as_i64).unwrap_or(5);
    let max_count = settings.get("snapshot_max_count").and_then(Value::as_i64).unwrap_or(1);
    let store = SnapshotStore::new(&info.path)?;
    let snapshot = store.create(&category, retention_hours, max_count)?;
    Ok(json!({ "snapshot": snapshot, "snapshots": store.list(&category)? }))
}

pub(crate) fn server_snapshot_delete(request: &Value) -> Result<Value, String> {
    let name = request_str(request, "name");
    let category = snapshot_category(request);
    let snapshot_id = request_str(request, "snapshot_id");
    let (_, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    let store = SnapshotStore::new(&info.path)?;
    let removed = store.delete(&category, &snapshot_id);
    Ok(json!({ "removed": removed, "snapshots": store.list(&category)? }))
}

pub(crate) fn server_snapshot_restore(request: &Value) -> Result<Value, String> {
    let name = request_str(request, "name");
    let category = snapshot_category(request);
    let snapshot_id = request_str(request, "snapshot_id");
    let (config, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    if super::server::server_is_running(&info.path) {
        return Err("Остановите сервер перед восстановлением снапшота".to_owned());
    }
    let store = SnapshotStore::new(&info.path)?;
    let restored = store.restore(&category, &snapshot_id)?;
    let files = ServerFiles::open()?;
    Ok(json!({
        "restored": restored,
        "snapshots": store.list(&category)?,
        "files": files.list(&name, &category)?,
    }))
}

fn decode_upload(raw: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let compressed = raw.trim();
    let decoded = if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(compressed) {
        decoded
    } else {
        return Err("Файл сервера должен быть корректным base64".to_owned());
    };
    if decoded.len() > MAX_UPLOAD_BYTES {
        return Err("Файл сервера слишком большой (максимум 256 MiB)".to_owned());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "nivora-server-files-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn split_parts_rules() {
        assert!(split_parts("", true).unwrap().is_empty());
        assert!(split_parts("", false).is_err());
        assert_eq!(split_parts("plugins/ess", false).unwrap(), vec!["plugins", "ess"]);
        assert_eq!(split_parts(r"plugins\ess", false).unwrap(), vec!["plugins", "ess"]);
        assert_eq!(split_parts("a/./b", false).unwrap(), vec!["a", "b"]);
        assert!(split_parts("../escape", false).is_err());
        assert!(split_parts("/abs", false).is_err());
        assert!(split_parts("C:/abs", false).is_err());
        assert!(split_parts("a\0b", false).is_err());
    }

    #[test]
    fn reserved_names_cover_both_prefixes() {
        assert!(reserved_name(".trial-launcher-instance.json"));
        assert!(reserved_name(".trial-launcher-server.json"));
        assert!(reserved_name(".nivora-launcher-server.json"));
        assert!(!reserved_name("server.properties"));
    }

    #[test]
    fn eula_file_written_atomically() {
        let root = scratch("eula");
        write_eula_file(&root, true).unwrap();
        assert_eq!(fs::read_to_string(root.join("eula.txt")).unwrap(), "# Generated by Nivora Launcher\neula=true\n");
        write_eula_file(&root, false).unwrap();
        assert_eq!(fs::read_to_string(root.join("eula.txt")).unwrap(), "# Generated by Nivora Launcher\neula=false\n");
        #[cfg(unix)]
        {
            fs::remove_file(root.join("eula.txt")).unwrap();
            std::os::unix::fs::symlink(root.join("elsewhere"), root.join("eula.txt")).unwrap();
            assert!(write_eula_file(&root, true).is_err());
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn decode_upload_accepts_base64_only() {
        assert_eq!(decode_upload("aGVsbG8=").unwrap(), b"hello");
        assert!(decode_upload("!!!").is_err());
    }

    #[test]
    fn files_roundtrip_over_server_instance() {
        let base = scratch("roundtrip");
        let config_dir = base.join("config");
        let data_dir = base.join("data");
        let cache_dir = base.join("cache");
        for directory in [&config_dir, &data_dir, &cache_dir] {
            fs::create_dir_all(directory).unwrap();
        }
        let roots = super::super::state::LauncherRoots {
            config_home: config_dir,
            data_home: data_dir.clone(),
            cache_home: cache_dir,
        };
        let config = ConfigStore::from_paths(super::super::config::Paths::from_roots(&roots)).expect("конфиг");
        let instances = InstanceStore::new(&config).expect("экземпляры");
        let instance_dir = data_dir.join("game/instances/demo");
        fs::create_dir_all(instance_dir.join("world")).unwrap();
        fs::write(
            instance_dir.join(super::super::instances::INSTANCE_METADATA_FILE),
            serde_json::json!({ "kind": "server", "name": "demo" }).to_string(),
        )
        .unwrap();
        fs::write(instance_dir.join(".nivora-launcher-server.json"), "{}").unwrap();
        fs::write(instance_dir.join("world/level.dat"), vec![7u8; 4096]).unwrap();

        let files = ServerFiles { config, instances };

        let entries = files.list("demo", "").expect("list");
        let names: Vec<&str> = entries.iter().filter_map(|item| item["name"].as_str()).collect();
        assert_eq!(names.first().copied(), Some("world"));
        assert!(!names.contains(&".nivora-launcher-server.json"));
        assert!(!names.iter().any(|name| name.starts_with(".nivora-")));
        let world = entries.first().unwrap();
        assert_eq!(world["kind"].as_str(), Some("directory"));
        assert_eq!(world["size"].as_i64(), Some(4096));

        files.write_text("demo", "server.properties", "motd=привет").expect("write");
        assert_eq!(files.read_text("demo", "server.properties").unwrap(), "motd=привет");
        assert_eq!(
            files.write_text("demo", ".nivora-launcher-server.json", "{}").unwrap_err(),
            "Служебные файлы лаунчера нельзя изменять"
        );

        files.mkdir("demo", "plugins").expect("mkdir");
        assert!(files.delete("demo", "server.properties").is_ok());
        assert!(files.delete("demo", "world").is_ok());
        assert!(files.read_text("demo", "../escape").is_err());
        assert!(files.delete("demo", "").is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn public_shape_matches_python() {
        let info = InstanceInfo {
            name: "demo".into(),
            path: "/srv/demo".into(),
            version_id: "vid".into(),
            minecraft_version: "1.20.1".into(),
            loader: "paper".into(),
            loader_version: String::new(),
            display_name: "Demo Server".into(),
            description: "описание".into(),
            group: String::new(),
            icon: String::new(),
            banner: String::new(),
            played_seconds: 42,
            kind: "server".into(),
            game_edition: "java".into(),
            bedrock_profile: String::new(),
            server_core: "custom".into(),
            server_core_version: String::new(),
            server_port: 25565,
            server_eula: false,
            running: false,
            notes: String::new(),
            tags: Vec::new(),
        };
        let config = json!({ "core": "paper", "version": "1.20.1", "port": 25577, "eula_accepted": true });
        let public = server_public(&info, &config);
        assert_eq!(public["kind"].as_str(), Some("server"));
        assert_eq!(public["server_core"].as_str(), Some("paper"));
        assert_eq!(public["server_port"].as_i64(), Some(25577));
        assert_eq!(public["server_eula"].as_bool(), Some(true));
        assert_eq!(public["display_name"].as_str(), Some("Demo Server"));
    }
}
