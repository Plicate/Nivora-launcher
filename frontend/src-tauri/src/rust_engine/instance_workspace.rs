// порт python-модуля instance_workspace.py: рабочее пространство Java-
// экземпляра — настройки, ресурсы (моды/ресурспаки/шейдеры), миры (NBT) и
// список серверов. Имена и сообщения — дословно как в воркере.
use base64::Engine as _;
use flate2::read::GzDecoder;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::config::{self, ConfigStore};
use super::events::emit_status_event;
use super::instances::{InstanceInfo, InstanceStore};
use super::net::atomic_json_write;
use super::snapshots::SnapshotStore;

const INSTANCE_SETTINGS_FILE: &str = ".trial-launcher.json";
const SECURITY_POLICY_VERSION: i64 = 4;
const PROJECT_METADATA_SUFFIX: &str = ".trial-project.json";
const MODPACK_MARKER: &str = ".trial-launcher-modpacks.json";
const MAX_ASSET_METADATA_BYTES: u64 = 64 * 1024;
const MAX_ASSET_ICON_BYTES: u64 = 64 * 1024;
const MAX_INSTANCE_SETTINGS_BYTES: u64 = 256 * 1024;
const MAX_ASSET_ENTRIES: usize = 5000;
const INSTANCE_SNAPSHOT_CATEGORIES: [&str; 4] = ["mods", "resourcepacks", "shaders", "saves"];
const MAX_NBT_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_NBT_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;
const MAX_NBT_COLLECTION_ITEMS: i64 = 100_000;
const MAX_NBT_DEPTH: usize = 64;
const MAX_NBT_NODES: usize = 500_000;
const MAX_NBT_STRING_BYTES: usize = 1_048_576;
const MAX_IMPORT_ASSET_BYTES: usize = 256 * 1024 * 1024;

const SECURITY_PROFILES: [&str; 4] = ["compat", "off", "manual", "strict"];
const SECURITY_PERMISSIONS: [&str; 10] = [
    "network:game",
    "files:instance",
    "desktop:display",
    "desktop:audio",
    "files:host-read",
    "files:host-write",
    "process:spawn",
    "chat:read",
    "input:capture",
    "native:load",
];
const DEFAULT_PERMISSIONS: [&str; 4] = [
    "network:game",
    "files:instance",
    "desktop:display",
    "desktop:audio",
];
const INSTANCE_SECURITY_KEYS: [&str; 4] = [
    "java_path",
    "security_profile",
    "security_permissions",
    "security_policy_version",
];
/// обычные (незащищённые) ключи настроек — порядок как в DEFAULT_INSTANCE_SETTINGS.
const INSTANCE_REGULAR_KEYS: [&str; 12] = [
    "memory",
    "graphics_mode",
    "window_width",
    "window_height",
    "fullscreen",
    "quick_play_world",
    "quick_play_server",
    "jvm_arguments",
    "game_arguments",
    "jvm_preset",
    "console_colors",
    "library_overrides",
];
const DEFAULT_CONSOLE_COLORS: [(&str, &str); 3] = [
    ("error", "#ff657a"),
    ("info", "#e6edf7"),
    ("warning", "#f5c451"),
];
const LIBRARY_OVERRIDE_ALLOWLIST: [&str; 8] = [
    "org.lwjgl:lwjgl",
    "org.lwjgl:lwjgl-freetype",
    "org.lwjgl:lwjgl-glfw",
    "org.lwjgl:lwjgl-jemalloc",
    "org.lwjgl:lwjgl-openal",
    "org.lwjgl:lwjgl-opengl",
    "org.lwjgl:lwjgl-stb",
    "org.lwjgl:lwjgl-tinyfd",
];

fn asset_directory_name(kind: &str) -> Option<&'static str> {
    match kind {
        "mods" => Some("mods"),
        "resourcepacks" => Some("resourcepacks"),
        "shaders" => Some("shaderpacks"),
        _ => None,
    }
}

fn project_type_kind(project_type: &str) -> Option<&'static str> {
    match project_type {
        "mod" => Some("mods"),
        "resourcepack" => Some("resourcepacks"),
        "shader" => Some("shaders"),
        "datapack" => Some("datapacks"),
        _ => None,
    }
}

fn instance_security_defaults() -> Value {
    json!({
        "java_path": "",
        "security_profile": "strict",
        "security_permissions": DEFAULT_PERMISSIONS,
        "security_policy_version": SECURITY_POLICY_VERSION,
    })
}

fn default_instance_settings_map() -> Map<String, Value> {
    let mut values = Map::new();
    values.insert("memory".into(), json!("auto"));
    values.insert("java_path".into(), json!(""));
    values.insert("graphics_mode".into(), json!("auto"));
    values.insert("window_width".into(), json!(1280));
    values.insert("window_height".into(), json!(720));
    values.insert("fullscreen".into(), json!(false));
    values.insert("quick_play_world".into(), json!(""));
    values.insert("quick_play_server".into(), json!(""));
    values.insert("jvm_arguments".into(), json!([]));
    values.insert("game_arguments".into(), json!([]));
    values.insert("jvm_preset".into(), json!("none"));
    let mut colors = Map::new();
    for (key, value) in DEFAULT_CONSOLE_COLORS {
        colors.insert(key.into(), json!(value));
    }
    values.insert("console_colors".into(), Value::Object(colors));
    values.insert("security_profile".into(), json!("strict"));
    values.insert("security_permissions".into(), json!(DEFAULT_PERMISSIONS));
    values.insert(
        "security_policy_version".into(),
        json!(SECURITY_POLICY_VERSION),
    );
    values.insert("library_overrides".into(), json!({}));
    values
}

pub(crate) struct InstanceWorkspace {
    instances: InstanceStore,
}

impl InstanceWorkspace {
    pub(crate) fn new() -> Result<Self, String> {
        let config = ConfigStore::load()?;
        let instances = InstanceStore::new(&config)?;
        Ok(Self { instances })
    }

    fn java_instance(&self, name: &str) -> Result<InstanceInfo, String> {
        let info = self.instances.get(name)?;
        if info.kind != "client" || info.game_edition != "java" {
            return Err(
                "Эта рабочая область предназначена только для Minecraft Java Edition".to_owned(),
            );
        }
        Ok(info)
    }

    pub(crate) fn settings(&self, name: &str) -> Result<Value, String> {
        let info = self.java_instance(name)?;
        let mut values = default_instance_settings_map();
        let protected = self
            .instances
            .load_protected_settings(&info.name, &instance_security_defaults())?;
        for (key, value) in protected.as_object().into_iter().flatten() {
            values.insert(key.clone(), value.clone());
        }
        let path = info.path.join(INSTANCE_SETTINGS_FILE);
        if is_symlink(&path) {
            return Err("Настройки экземпляра являются символьной ссылкой".to_owned());
        }
        let mut rewrite = !path.exists();
        if path.exists() {
            let meta = fs::symlink_metadata(&path)
                .map_err(|error| format!("Не удалось прочитать настройки экземпляра: {error}"))?;
            if meta.len() > MAX_INSTANCE_SETTINGS_BYTES {
                return Err("Настройки экземпляра слишком большие".to_owned());
            }
            let raw_text = fs::read_to_string(&path)
                .map_err(|error| format!("Не удалось прочитать настройки экземпляра: {error}"))?;
            let raw: Value = serde_json::from_str(&raw_text)
                .map_err(|error| format!("Не удалось прочитать настройки экземпляра: {error}"))?;
            if let Some(fields) = raw.as_object() {
                for key in INSTANCE_REGULAR_KEYS {
                    if let Some(stored) = fields.get(key) {
                        values.insert(key.to_owned(), stored.clone());
                    }
                }
                rewrite = INSTANCE_SECURITY_KEYS
                    .iter()
                    .any(|key| fields.contains_key(*key));
            }
        }
        let normalized = normalize_instance_settings(&values)?;
        let mut payload = Map::new();
        for key in INSTANCE_SECURITY_KEYS {
            payload.insert(
                key.to_owned(),
                normalized.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        self.instances
            .save_protected_settings(&info.name, &Value::Object(payload))?;
        if rewrite {
            write_instance_settings(&path, &normalized)?;
        }
        Ok(normalized)
    }

    pub(crate) fn save_settings(&self, name: &str, values: &Value) -> Result<Value, String> {
        let info = self.java_instance(name)?;
        let mut current = self.settings(name)?;
        for key in default_instance_settings_map().keys() {
            let Some(incoming) = values.get(key) else {
                continue;
            };
            if key == "console_colors" {
                if let (Some(target), Some(update)) =
                    (current.get_mut("console_colors"), incoming.as_object())
                {
                    if let Some(target_map) = target.as_object_mut() {
                        for (color_key, color_value) in update {
                            target_map.insert(color_key.clone(), color_value.clone());
                        }
                        continue;
                    }
                }
            }
            current
                .as_object_mut()
                .expect("нормализованные настройки — объект")
                .insert(key.clone(), incoming.clone());
        }
        let normalized =
            normalize_instance_settings(current.as_object().expect("настройки — объект"))?;
        write_instance_settings(&info.path.join(INSTANCE_SETTINGS_FILE), &normalized)?;
        let mut payload = Map::new();
        for key in INSTANCE_SECURITY_KEYS {
            payload.insert(
                key.to_owned(),
                normalized.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        self.instances
            .save_protected_settings(&info.name, &Value::Object(payload))?;
        Ok(normalized)
    }

    pub(crate) fn snapshot(&self, name: &str) -> Result<Value, String> {
        let info = self.java_instance(name)?;
        let mut assets = Map::new();
        for kind in ["mods", "resourcepacks", "shaders"] {
            assets.insert(kind.to_owned(), Value::Array(self.list_assets(name, kind)?));
        }
        let mut snapshots = Map::new();
        let store = SnapshotStore::new(&info.path)?;
        for category in INSTANCE_SNAPSHOT_CATEGORIES {
            snapshots.insert(category.to_owned(), Value::Array(store.list(category)?));
        }
        Ok(json!({
            "instance": {
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
                "kind": info.kind,
                "game_edition": info.game_edition,
                "server_core": info.server_core,
                "server_core_version": info.server_core_version,
                "server_port": info.server_port,
                "server_eula": info.server_eula,
                "running": info.running,
                "notes": info.notes,
                "tags": info.tags,
            },
            "settings": self.settings(name)?,
            "assets": assets,
            "worlds": self.list_worlds(name)?,
            "servers": self.list_servers(name)?,
            "snapshots": snapshots,
        }))
    }

    pub(crate) fn list_assets(&self, name: &str, kind: &str) -> Result<Vec<Value>, String> {
        let directory = self.asset_directory(name, kind)?;
        self.list_assets_from_directory(&directory, kind)
    }

    pub(crate) fn list_world_assets(
        &self,
        name: &str,
        world_id: &str,
        kind: &str,
    ) -> Result<Vec<Value>, String> {
        if kind != "datapacks" {
            return Err(format!("Неизвестный тип ресурса мира: {kind}"));
        }
        let directory = self.world_directory(name, world_id)?.join("datapacks");
        self.list_assets_from_directory(&directory, kind)
    }

    /// станет горячим после порта modpacks.py (installed-проекты каталогов).
    #[allow(dead_code)]
    pub(crate) fn installed_projects(
        &self,
        name: &str,
        project_type: &str,
        catalog_id: Option<&str>,
    ) -> Result<Vec<String>, String> {
        self.java_instance(name)?;
        if project_type == "modpack" {
            let marker = self.instances.get(name)?.path.join(MODPACK_MARKER);
            if is_symlink(&marker) || !marker.is_file() {
                return Ok(Vec::new());
            }
            let meta = match fs::symlink_metadata(&marker) {
                Ok(meta) => meta,
                Err(_) => return Ok(Vec::new()),
            };
            if meta.len() > 512 * 1024 {
                return Ok(Vec::new());
            }
            let Ok(text) = fs::read_to_string(&marker) else {
                return Ok(Vec::new());
            };
            let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
                return Ok(Vec::new());
            };
            let mut result = Vec::new();
            for item in items {
                let Some(entry) = item.as_object() else {
                    continue;
                };
                let project_id = entry
                    .get("project_id")
                    .map(py_display)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if project_id.is_empty() {
                    continue;
                }
                if let Some(expected) = catalog_id {
                    let stored = entry
                        .get("catalog_id")
                        .or_else(|| entry.get("source"))
                        .map(py_display)
                        .unwrap_or_default()
                        .trim()
                        .to_lowercase();
                    if stored != expected.trim().to_lowercase() {
                        continue;
                    }
                }
                result.push(project_id);
            }
            return Ok(result);
        }
        let mut result = Vec::new();
        for metadata_path in self.catalog_metadata_paths(name, project_type)? {
            if let Some(project_id) = metadata_project_id(&metadata_path, project_type, catalog_id)
            {
                if !project_id.is_empty() {
                    result.push(project_id);
                }
            }
        }
        Ok(result)
    }

    /// станет горячим после порта modpacks.py (installed-проекты каталогов).
    #[allow(dead_code)]
    pub(crate) fn remove_installed_project(
        &self,
        name: &str,
        project_type: &str,
        project_id: &str,
        catalog_id: Option<&str>,
    ) -> Result<(Value, usize), String> {
        self.java_instance(name)?;
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err("Не указан project ID ресурса".to_owned());
        }
        let mut removed = 0usize;
        for metadata_path in self.catalog_metadata_paths(name, project_type)? {
            if metadata_project_id(&metadata_path, project_type, catalog_id).as_deref()
                != Some(project_id)
            {
                continue;
            }
            let file_name = metadata_path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            // имя метаданных — ".{имя ресурса}{suffix}"
            let target_name = file_name
                .strip_prefix('.')
                .unwrap_or(&file_name)
                .strip_suffix(PROJECT_METADATA_SUFFIX)
                .unwrap_or(&file_name)
                .to_owned();
            let target = metadata_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&target_name);
            if is_symlink(&target) {
                return Err("Установленный ресурс является символьной ссылкой".to_owned());
            }
            if target.is_file() {
                fs::remove_file(&target).map_err(|error| error.to_string())?;
                removed += 1;
            }
            fs::remove_file(&metadata_path).map_err(|error| error.to_string())?;
        }
        if removed == 0 {
            return Err("Установленный ресурс не найден".to_owned());
        }
        Ok((self.snapshot(name)?, removed))
    }

    fn catalog_metadata_paths(
        &self,
        name: &str,
        project_type: &str,
    ) -> Result<Vec<PathBuf>, String> {
        self.java_instance(name)?;
        let Some(kind) = project_type_kind(project_type) else {
            return Err(format!("Неизвестный тип ресурса: {project_type}"));
        };
        let directories: Vec<PathBuf> = if kind == "datapacks" {
            let saves = self.instances.get(name)?.path.join("saves");
            if is_symlink(&saves) || !saves.exists() {
                return Ok(Vec::new());
            }
            let Ok(entries) = fs::read_dir(&saves) else {
                return Ok(Vec::new());
            };
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir() && !is_symlink(path))
                .map(|world| world.join("datapacks"))
                .collect()
        } else {
            vec![self.asset_directory(name, kind)?]
        };
        let mut paths = Vec::new();
        for directory in directories {
            if is_symlink(&directory) || !directory.exists() {
                continue;
            }
            let mut tree = Vec::new();
            walk_paths(&directory, &mut tree);
            tree.sort_by_key(|path| path.to_string_lossy().to_lowercase());
            for path in tree {
                let file_name = path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !file_name.ends_with(PROJECT_METADATA_SUFFIX)
                    || is_symlink(&path)
                    || !path.is_file()
                {
                    continue;
                }
                paths.push(path);
            }
        }
        Ok(paths)
    }

    fn list_assets_from_directory(
        &self,
        directory: &Path,
        kind: &str,
    ) -> Result<Vec<Value>, String> {
        if is_symlink(directory) {
            return Err("Каталог ресурса является символьной ссылкой".to_owned());
        }
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut tree = Vec::new();
        walk_paths(directory, &mut tree);
        tree.sort_by_key(|path| path.to_string_lossy().to_lowercase());
        let mut values = Vec::new();
        for path in tree {
            let file_name = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            if is_symlink(&path)
                || !path.is_file()
                || file_name.ends_with(".trial-meta")
                || file_name.ends_with(PROJECT_METADATA_SUFFIX)
                || file_name.ends_with(".part")
            {
                continue;
            }
            let stat = fs::metadata(&path).map_err(|error| error.to_string())?;
            let metadata = asset_metadata(&path);
            values.push(json!({
                "name": file_name,
                "display_name": metadata.get("display_name").cloned().unwrap_or_default(),
                "path": strip_prefix_lossy(&path, directory),
                "size": stat.len(),
                "modified_at": mtime_seconds(&stat),
                "kind": kind,
                "enabled": !(kind == "mods" && file_name.ends_with(".disabled")),
                "version": metadata.get("version").cloned().unwrap_or_default(),
                "author": metadata.get("author").cloned().unwrap_or_default(),
                "source": metadata.get("source").cloned().unwrap_or_default(),
                "project_id": metadata.get("project_id").cloned().unwrap_or_default(),
                "icon_url": metadata.get("icon_url").cloned().unwrap_or_default(),
                "icon_data": metadata.get("icon_data").cloned().unwrap_or_default(),
            }));
            if values.len() >= MAX_ASSET_ENTRIES {
                break;
            }
        }
        Ok(values)
    }

    pub(crate) fn remove_asset(
        &self,
        name: &str,
        kind: &str,
        raw_relative: &str,
    ) -> Result<(), String> {
        let (directory, relative_path): (PathBuf, String);
        if kind == "datapacks" {
            let parts: Vec<String> = Path::new(raw_relative)
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            if parts.len() < 2
                || parts
                    .iter()
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err("Небезопасный путь датапака".to_owned());
            }
            let world_id = parts[0].clone();
            directory = self.world_directory(name, &world_id)?.join("datapacks");
            relative_path = parts[1..].join("/");
        } else {
            directory = self.asset_directory(name, kind)?;
            relative_path = raw_relative.to_owned();
        }
        let Ok(directory) = directory.canonicalize() else {
            return Err("Файл ресурса не найден".to_owned());
        };
        let raw_target = directory.join(&relative_path);
        if is_symlink(&raw_target) {
            return Err("Символьные ссылки в ресурсах запрещены".to_owned());
        }
        let Ok(target) = raw_target.canonicalize() else {
            return Err("Файл ресурса не найден".to_owned());
        };
        if !target.starts_with(&directory) || !target.is_file() {
            return Err("Файл ресурса не найден".to_owned());
        }
        fs::remove_file(&target).map_err(|error| error.to_string())?;
        let sidecar = sidecar_path(&target);
        if sidecar.exists() || is_symlink(&sidecar) {
            fs::remove_file(&sidecar).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn toggle_mod(
        &self,
        name: &str,
        relative_path: &str,
        enabled: bool,
    ) -> Result<Value, String> {
        let directory = self.asset_directory(name, "mods")?;
        let Ok(directory) = directory.canonicalize() else {
            return Err("Файл мода не найден".to_owned());
        };
        let raw_target = directory.join(relative_path);
        if is_symlink(&raw_target) {
            return Err("Файл мода не найден".to_owned());
        }
        let Ok(target) = raw_target.canonicalize() else {
            return Err("Файл мода не найден".to_owned());
        };
        if !target.starts_with(&directory) || !target.is_file() {
            return Err("Файл мода не найден".to_owned());
        }
        let target_name = target
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let destination_name = if enabled && target_name.ends_with(".disabled") {
            target_name
                .strip_suffix(".disabled")
                .unwrap_or(&target_name)
                .to_owned()
        } else if !enabled && !target_name.ends_with(".disabled") {
            format!("{target_name}.disabled")
        } else {
            return self.snapshot(name);
        };
        let destination = target.with_file_name(&destination_name);
        if destination.exists() || is_symlink(&destination) {
            return Err(format!("Файл уже существует: {destination_name}"));
        }
        fs::rename(&target, &destination).map_err(|error| error.to_string())?;
        let sidecar = sidecar_path(&target);
        let next_sidecar = sidecar_path(&destination);
        if is_symlink(&sidecar) || is_symlink(&next_sidecar) {
            return Err("Метаданные мода являются символьной ссылкой".to_owned());
        }
        if sidecar.exists() {
            fs::rename(&sidecar, &next_sidecar).map_err(|error| error.to_string())?;
        }
        self.snapshot(name)
    }

    pub(crate) fn import_asset(
        &self,
        name: &str,
        kind: &str,
        file_name: &str,
        content: &[u8],
    ) -> Result<Value, String> {
        if content.len() > MAX_IMPORT_ASSET_BYTES {
            return Err("Файл ресурса слишком большой (максимум 256 MiB)".to_owned());
        }
        let normalized = file_name.replace('\\', "/");
        let safe_name = Path::new(&normalized)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        if safe_name.is_empty() || safe_name == "." || safe_name == ".." || safe_name != normalized
        {
            return Err("Небезопасное имя импортируемого файла".to_owned());
        }
        let directory = self.asset_directory(name, kind)?;
        create_private_dir_all(&directory).map_err(|error| error.to_string())?;
        if !directory.is_dir() {
            return Err("Каталог ресурса недоступен".to_owned());
        }
        let destination = unique_file(&directory, &safe_name);
        write_private_file(&destination, content)?;
        self.snapshot(name)
    }

    pub(crate) fn list_servers(&self, name: &str) -> Result<Vec<Value>, String> {
        let path = self.java_instance(name)?.path.join("servers.dat");
        if is_symlink(&path) || !path.exists() {
            return Ok(Vec::new());
        }
        let root = read_nbt(&path)?;
        let Some(values) = root.get("servers").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for (index, server) in values.iter().enumerate() {
            if !server.is_object() {
                continue;
            }
            let address = server
                .get("ip")
                .map(py_display)
                .unwrap_or_default()
                .trim()
                .chars()
                .take(256)
                .collect::<String>();
            if address.is_empty() {
                continue;
            }
            let display = match server.get("name") {
                Some(value) if py_truthy(value) => py_display(value),
                _ => address.clone(),
            };
            result.push(json!({
                "id": format!("server-{index}"),
                "name": display.chars().take(256).collect::<String>(),
                "address": address,
                "accept_textures": server.get("acceptTextures").map(py_truthy).unwrap_or(false),
            }));
        }
        Ok(result)
    }

    pub(crate) fn list_worlds(&self, name: &str) -> Result<Vec<Value>, String> {
        let saves = self.java_instance(name)?.path.join("saves");
        if is_symlink(&saves) || !saves.exists() {
            return Ok(Vec::new());
        }
        let Ok(entries) = fs::read_dir(&saves) else {
            return Ok(Vec::new());
        };
        let mut worlds: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && !is_symlink(path))
            .collect();
        worlds.sort_by_key(|path| {
            path.file_name()
                .map(|value| value.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        let mut result = Vec::new();
        for path in worlds {
            let world_id = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            let level = path.join("level.dat");
            let mut data = Map::new();
            if level.exists() {
                if let Ok(root) = read_nbt(&level) {
                    let payload = root.get("Data").cloned().unwrap_or(root);
                    if let Some(fields) = payload.as_object() {
                        data = fields.clone();
                    }
                }
            }
            let icon = path.join("icon.png");
            let level_name = match data.get("LevelName") {
                Some(value) if py_truthy(value) => py_display(value),
                _ => world_id.clone(),
            };
            let last_played = data.get("LastPlayed").and_then(as_positive_int);
            result.push(json!({
                "id": world_id,
                "name": level_name,
                "path": path.to_string_lossy(),
                "size": directory_size(&path),
                "last_played": last_played,
                "icon": image_data_url(&icon),
                "datapacks": self.list_world_assets(name, &world_id, "datapacks")?,
            }));
        }
        Ok(result)
    }

    fn asset_directory(&self, name: &str, kind: &str) -> Result<PathBuf, String> {
        let Some(relative) = asset_directory_name(kind) else {
            return Err(format!("Неизвестный тип ресурса: {kind}"));
        };
        let directory = self.java_instance(name)?.path.join(relative);
        if is_symlink(&directory) {
            return Err("Каталог ресурса является символьной ссылкой".to_owned());
        }
        Ok(directory)
    }

    fn world_directory(&self, name: &str, world_id: &str) -> Result<PathBuf, String> {
        let saves = self.java_instance(name)?.path.join("saves");
        if is_symlink(&saves) {
            return Err("Каталог миров является символьной ссылкой".to_owned());
        }
        let safe_id = Path::new(world_id)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        if safe_id.is_empty() || safe_id != world_id || world_id == "." || world_id == ".." {
            return Err("Небезопасное имя мира".to_owned());
        }
        let raw_world = saves.join(&safe_id);
        if is_symlink(&raw_world) {
            return Err("Мир является символьной ссылкой".to_owned());
        }
        let Ok(resolved_saves) = saves.canonicalize() else {
            return Err("Мир не найден".to_owned());
        };
        let Ok(world) = raw_world.canonicalize() else {
            return Err("Мир не найден".to_owned());
        };
        if !world.starts_with(&resolved_saves) || !world.is_dir() {
            return Err("Мир не найден".to_owned());
        }
        Ok(world)
    }
}

// ------------------------------------------------------------- операции ----

pub(crate) fn instance_details(request: &Value) -> Result<Value, String> {
    let workspace = InstanceWorkspace::new()?;
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    workspace.snapshot(name)
}

pub(crate) fn save_instance_settings(request: &Value) -> Result<Value, String> {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let values = request.get("values").cloned().unwrap_or_else(|| json!({}));
    if !values.is_object() {
        return Err("Настройки экземпляра имеют неожиданный формат".to_owned());
    }
    let workspace = InstanceWorkspace::new()?;
    let settings = workspace.save_settings(name, &values)?;
    emit_status_event("workspace-changed", "Настройки экземпляра обновлены.");
    Ok(json!({ "settings": settings }))
}

pub(crate) fn remove_instance_asset(request: &Value) -> Result<Value, String> {
    let workspace = InstanceWorkspace::new()?;
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    workspace.remove_asset(
        name,
        request
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let snapshot = workspace.snapshot(name)?;
    emit_status_event("workspace-changed", "Файл ресурса удалён.");
    Ok(snapshot)
}

pub(crate) fn toggle_mod(request: &Value) -> Result<Value, String> {
    let workspace = InstanceWorkspace::new()?;
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = workspace.toggle_mod(
        name,
        request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        request.get("enabled").map(py_truthy).unwrap_or(false),
    )?;
    emit_status_event("workspace-changed", "Состояние мода обновлено.");
    Ok(result)
}

pub(crate) fn import_asset(request: &Value) -> Result<Value, String> {
    let encoded = request
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if encoded.len() > 360 * 1024 * 1024 {
        return Err("Файл ресурса слишком большой".to_owned());
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Импортируемый файл повреждён".to_owned())?;
    let workspace = InstanceWorkspace::new()?;
    let result = workspace.import_asset(
        request
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        request
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        request
            .get("file_name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &content,
    )?;
    emit_status_event("workspace-changed", "Ресурс импортирован.");
    Ok(result)
}

pub(crate) fn mods_remove_files(request: &Value) -> Result<Value, String> {
    let workspace = InstanceWorkspace::new()?;
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let info = workspace.java_instance(&name)?;
    if info.running {
        return Err("Остановите игру перед удалением модов".to_owned());
    }
    let Some(paths) = request.get("paths").and_then(Value::as_array) else {
        return Err("Не выбрано ни одного мода".to_owned());
    };
    if paths.is_empty() {
        return Err("Не выбрано ни одного мода".to_owned());
    }
    let mut removed: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    for value in paths.iter().take(2000) {
        let relative = value.as_str().unwrap_or_default();
        match workspace.remove_asset(&name, "mods", relative) {
            Ok(()) => removed.push(relative.to_owned()),
            Err(error) => failed.push(json!({ "path": relative, "error": error })),
        }
    }
    emit_status_event(
        "workspace-changed",
        &format!("Удалено модов: {}.", removed.len()),
    );
    Ok(json!({
        "removed": removed,
        "failed": failed,
        "details": workspace.snapshot(&name)?,
    }))
}

// ------------------------------------------------------- нормализация ----

fn normalize_instance_settings(values: &Map<String, Value>) -> Result<Value, String> {
    let mut result = default_instance_settings_map();
    for (key, value) in values {
        result.insert(key.clone(), value.clone());
    }
    let memory = result
        .get("memory")
        .map(py_display)
        .unwrap_or_else(|| "auto".to_owned());
    let memory = memory.trim();
    let memory = if memory.is_empty() { "auto" } else { memory };
    if memory.eq_ignore_ascii_case("auto") || memory.eq_ignore_ascii_case("automatic") {
        result.insert("memory".into(), json!("auto"));
    } else {
        let memory_kib = config::parse_memory_kib(memory)?;
        let total_kib = config::system_memory_kib();
        if total_kib > 0 && memory_kib > total_kib {
            return Err(format!(
                "Память экземпляра не может превышать доступные {}",
                config::format_memory_kib(total_kib)
            ));
        }
        result.insert(
            "memory".into(),
            json!(config::format_memory_kib(memory_kib)),
        );
    }
    result.insert(
        "java_path".into(),
        json!(result
            .get("java_path")
            .map(py_display)
            .unwrap_or_default()
            .trim()),
    );
    let graphics = result
        .get("graphics_mode")
        .map(py_display)
        .unwrap_or_else(|| "auto".to_owned())
        .trim()
        .to_lowercase();
    if !["auto", "nvidia", "mesa", "software"].contains(&graphics.as_str()) {
        return Err(format!("Неизвестный graphics mode: {graphics}"));
    }
    result.insert("graphics_mode".into(), json!(graphics));
    for (key, default) in [("window_width", 1280i64), ("window_height", 720i64)] {
        let value = result.get(key).cloned().unwrap_or_else(|| json!(default));
        let Some(number) = coerce_int(&value) else {
            return Err(format!("{key} должен быть числом"));
        };
        result.insert(key.to_owned(), json!(number.clamp(320, 7680)));
    }
    result.insert(
        "fullscreen".into(),
        json!(result.get("fullscreen").map(py_truthy).unwrap_or(false)),
    );
    for key in ["quick_play_world", "quick_play_server"] {
        result.insert(
            key.to_owned(),
            json!(result
                .get(key)
                .map(py_display)
                .unwrap_or_default()
                .trim()
                .chars()
                .take(256)
                .collect::<String>()),
        );
    }
    result.insert(
        "library_overrides".into(),
        normalize_library_overrides(result.get("library_overrides"))?,
    );
    let mut console_colors = Map::new();
    let raw_colors = result.get("console_colors").and_then(Value::as_object);
    for (key, default) in DEFAULT_CONSOLE_COLORS {
        let color = raw_colors
            .and_then(|colors| colors.get(key))
            .map(py_display)
            .unwrap_or_else(|| default.to_owned())
            .trim()
            .to_owned();
        let hex = hex_color_regex();
        let value = if hex.is_match(&color) {
            color.to_lowercase()
        } else {
            default.to_owned()
        };
        console_colors.insert(key.to_owned(), json!(value));
    }
    result.insert("console_colors".into(), Value::Object(console_colors));
    let profile = normalize_security_profile(result.get("security_profile"))?;
    result.insert("security_profile".into(), json!(profile));
    // аргументы JVM задаются на экземпляр и перекрывают профиль запуска —
    // нормализация та же, что у глобальных профилей.
    result.insert(
        "jvm_arguments".into(),
        config::profile_arguments(result.get("jvm_arguments")),
    );
    result.insert(
        "game_arguments".into(),
        config::profile_arguments(result.get("game_arguments")),
    );
    result.insert(
        "jvm_preset".into(),
        json!(safe_preset_id(
            &result
                .get("jvm_preset")
                .map(py_display)
                .unwrap_or_else(|| "none".to_owned())
        )),
    );
    let mut permissions = normalize_security_permissions(result.get("security_permissions"));
    let previous_policy_version = result
        .get("security_policy_version")
        .and_then(coerce_int)
        .unwrap_or(0);
    if previous_policy_version < SECURITY_POLICY_VERSION {
        // миграция v4: ≤3 молча выключали network:game, и Minecraft не видел
        // серверов (ручной выбор пользователя после миграции не трогаем)
        for permission in ["network:game", "desktop:display", "desktop:audio"] {
            if !permissions.contains(&permission.to_owned()) {
                permissions.push(permission.to_owned());
            }
        }
    }
    result.insert("security_permissions".into(), json!(permissions));
    result.insert(
        "security_policy_version".into(),
        json!(SECURITY_POLICY_VERSION),
    );
    Ok(Value::Object(result))
}

fn normalize_security_profile(value: Option<&Value>) -> Result<String, String> {
    let raw = match value {
        Some(value) if py_truthy(value) => py_display(value),
        _ => "strict".to_owned(),
    };
    let profile = raw.trim().to_lowercase();
    if !SECURITY_PROFILES.contains(&profile.as_str()) {
        return Err(format!("Неизвестный security profile: {profile}"));
    }
    Ok(profile)
}

fn normalize_security_permissions(value: Option<&Value>) -> Vec<String> {
    let defaults: Vec<Value> = DEFAULT_PERMISSIONS.iter().map(|item| json!(item)).collect();
    let raw = match value {
        Some(Value::Array(items)) => items.as_slice(),
        _ => defaults.as_slice(),
    };
    let mut result: Vec<String> = Vec::new();
    for item in raw {
        let permission = py_display(item).trim().to_lowercase();
        if !SECURITY_PERMISSIONS.contains(&permission.as_str()) || result.contains(&permission) {
            continue;
        }
        result.push(permission);
    }
    result
}

fn normalize_library_overrides(value: Option<&Value>) -> Result<Value, String> {
    let Some(fields) = value.and_then(Value::as_object) else {
        return Ok(json!({}));
    };
    let regex = library_version_regex();
    let mut cleaned = Map::new();
    for (name, version) in fields {
        let key = name.trim();
        let raw = py_display(version).trim().to_owned();
        if !LIBRARY_OVERRIDE_ALLOWLIST.contains(&key) || !regex.is_match(&raw) {
            continue;
        }
        cleaned.insert(key.to_owned(), json!(raw));
    }
    Ok(Value::Object(cleaned))
}

fn safe_preset_id(value: &str) -> String {
    let allowed = [
        "none",
        "fps",
        "balanced",
        "heavy_modpack",
        "compatibility",
        "streaming",
    ];
    let cleaned = value.trim().to_lowercase();
    if allowed.contains(&cleaned.as_str()) {
        cleaned
    } else {
        "none".to_owned()
    }
}

fn pretty_asset_name(file_name: &str) -> String {
    let mut value = file_name.to_owned();
    if let Some(stripped) = value.strip_suffix(".disabled") {
        value = stripped.to_owned();
    }
    for suffix in [".jar", ".zip"] {
        if value.to_lowercase().ends_with(suffix) {
            value.truncate(value.len() - suffix.len());
            break;
        }
    }
    let version = asset_version_regex();
    let value = version.replace(&value, "").into_owned();
    // re.sub(r"(?<!^)(?=[A-Z])", " ", …): пробел перед каждой заглавной
    // кроме первой — regex-крейт без lookbehind, поэтому вручную.
    let mut spaced = String::with_capacity(value.len() * 2);
    for (index, symbol) in value.chars().enumerate() {
        if index > 0 && symbol.is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(symbol);
    }
    let dashed = spaced.replace(['-', '_'], " ");
    let joined = dashed.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        "Неизвестный ресурс".to_owned()
    } else {
        joined
    }
}

fn safe_asset_icon_url(value: &str) -> String {
    let trimmed = value.trim();
    match url::Url::parse(trimmed) {
        Ok(parsed)
            if parsed.scheme().eq_ignore_ascii_case("https")
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none() =>
        {
            trimmed.chars().take(2048).collect()
        }
        _ => String::new(),
    }
}

fn asset_image_data_url(value: &[u8]) -> String {
    let mime = if value.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if value.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if value.len() >= 12 && &value[0..4] == b"RIFF" && &value[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return String::new();
    };
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(value)
    )
}

// --------------------------------------------------- метаданные ресурсов ----

fn asset_metadata(path: &Path) -> BTreeMap<String, String> {
    let metadata = read_project_metadata(path);
    let archive = read_archive_metadata(path);
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let sidecar = |key: &str| -> String {
        metadata
            .get(key)
            .filter(|value| py_truthy(value))
            .map(py_display)
            .unwrap_or_default()
    };
    let archive_str = |key: &str| -> String {
        archive
            .get(key)
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_default()
    };
    let display_name = {
        let direct = sidecar("display_name");
        let via_name = if direct.is_empty() {
            sidecar("name")
        } else {
            direct
        };
        let from_archive = archive_str("display_name");
        if !via_name.is_empty() {
            via_name
        } else if !from_archive.is_empty() {
            from_archive
        } else {
            pretty_asset_name(&file_name)
        }
    };
    let source = {
        let direct = sidecar("source");
        if direct.is_empty() {
            "Локальный файл".to_owned()
        } else {
            direct
        }
    };
    let mut payload = BTreeMap::new();
    payload.insert("display_name".into(), truncate_chars(&display_name, 160));
    payload.insert(
        "version".into(),
        truncate_chars(&short_or(&sidecar("version"), &archive_str("version")), 80),
    );
    payload.insert(
        "author".into(),
        truncate_chars(&short_or(&sidecar("author"), &archive_str("author")), 120),
    );
    payload.insert("source".into(), truncate_chars(&source, 80));
    payload.insert(
        "project_id".into(),
        truncate_chars(&sidecar("project_id"), 160),
    );
    payload.insert("icon_url".into(), safe_asset_icon_url(&sidecar("icon_url")));
    payload.insert("icon_data".into(), archive_str("icon_data"));
    payload
}

fn short_or(first: &str, second: &str) -> String {
    if first.is_empty() {
        second.to_owned()
    } else {
        first.to_owned()
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn read_project_metadata(path: &Path) -> Map<String, Value> {
    let sidecar = sidecar_path(path);
    if is_symlink(&sidecar) || !sidecar.is_file() {
        return Map::new();
    }
    let Ok(meta) = fs::symlink_metadata(&sidecar) else {
        return Map::new();
    };
    if meta.len() > MAX_ASSET_METADATA_BYTES {
        return Map::new();
    }
    let Ok(text) = fs::read_to_string(&sidecar) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

fn metadata_project_id(
    path: &Path,
    project_type: &str,
    catalog_id: Option<&str>,
) -> Option<String> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Some(String::new());
    };
    if meta.len() > 32 * 1024 {
        return Some(String::new());
    }
    let Ok(text) = fs::read_to_string(path) else {
        return Some(String::new());
    };
    let Ok(Value::Object(value)) = serde_json::from_str::<Value>(&text) else {
        return Some(String::new());
    };
    let stored_type = value
        .get("project_type")
        .map(|item| py_display_or(item, project_type))
        .unwrap_or_else(|| project_type.to_owned())
        .trim()
        .to_lowercase();
    let source = value
        .get("source")
        .map(py_display)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let stored_catalog = value
        .get("catalog_id")
        .or_else(|| value.get("catalog"))
        .map(|item| {
            let text = py_display(item);
            if text.trim().is_empty() && source == "modrinth" {
                "modrinth".to_owned()
            } else {
                text
            }
        })
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if stored_type != project_type {
        return Some(String::new());
    }
    if let Some(expected) = catalog_id {
        if stored_catalog != expected.trim().to_lowercase() {
            return Some(String::new());
        }
    }
    Some(
        value
            .get("project_id")
            .map(py_display)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    )
}

fn read_archive_metadata(path: &Path) -> BTreeMap<String, String> {
    let suffix = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if ![".jar", ".zip", ".mcpack", ".disabled"].contains(&suffix.as_str()) {
        return BTreeMap::new();
    }
    let Ok(file) = fs::File::open(path) else {
        return BTreeMap::new();
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return BTreeMap::new();
    };
    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    for entry in [
        "fabric.mod.json",
        "quilt.mod.json",
        "mcmod.info",
        "pack.mcmeta",
    ] {
        let raw = read_zip_entry(&mut archive, entry, MAX_ASSET_METADATA_BYTES);
        if raw.is_empty() {
            continue;
        }
        for (key, value) in parse_archive_metadata(entry, &raw) {
            if !value.is_empty() && !metadata.contains_key(&key) {
                metadata.insert(key, value);
            }
        }
    }
    let icon_entry = metadata.remove("icon_entry").unwrap_or_default();
    let icon_entry = if icon_entry.is_empty() {
        find_archive_icon(&mut archive)
    } else {
        icon_entry
    };
    let icon = if icon_entry.is_empty() {
        Vec::new()
    } else {
        read_zip_entry(&mut archive, &icon_entry, MAX_ASSET_ICON_BYTES)
    };
    metadata.insert("icon_data".into(), asset_image_data_url(&icon));
    metadata
}

fn read_zip_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str, limit: u64) -> Vec<u8> {
    if name.is_empty()
        || name.starts_with('/')
        || Path::new(name)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Vec::new();
    }
    let Ok(mut entry) = archive.by_name(name) else {
        return Vec::new();
    };
    if entry.is_dir() || entry.size() > limit {
        return Vec::new();
    }
    let mut buffer = Vec::with_capacity(entry.size() as usize);
    if entry.read_to_end(&mut buffer).is_err() {
        return Vec::new();
    }
    buffer
}

fn find_archive_icon(archive: &mut zip::ZipArchive<std::fs::File>) -> String {
    let preferred = ["icon.png", "logo.png", "pack.png"];
    let names: Vec<String> = archive.file_names().map(|name| name.to_owned()).collect();
    for name in preferred {
        if names.contains(&name.to_owned())
            && !read_zip_entry(archive, name, MAX_ASSET_ICON_BYTES).is_empty()
        {
            return name.to_owned();
        }
    }
    let mut candidates: Vec<String> = Vec::new();
    for name in &names {
        let base = Path::new(name)
            .file_name()
            .map(|value| value.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if preferred.contains(&base.as_str())
            && !read_zip_entry(archive, name, MAX_ASSET_ICON_BYTES).is_empty()
        {
            candidates.push(name.clone());
        }
    }
    candidates.sort();
    candidates.into_iter().next().unwrap_or_default()
}

fn parse_archive_metadata(entry: &str, raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw).into_owned();
    let mut parsed: Option<Value> = None;
    if entry.ends_with(".json") || entry.ends_with(".mcmeta") {
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => parsed = Some(value),
            Err(_) => return Vec::new(),
        }
    }
    let get = object_of;
    if entry == "fabric.mod.json" {
        if let Some(fields) = get(parsed.as_ref()) {
            let name = fields
                .get("name")
                .filter(|value| py_truthy(value))
                .or_else(|| fields.get("id"))
                .map(text_value);
            let version = fields.get("version").map(text_value);
            let author = fields
                .get("authors")
                .or_else(|| fields.get("contributors"))
                .map(author_value);
            let icon = fields.get("icon").map(text_value);
            return metadata_fields(name, version, author, icon);
        }
        return Vec::new();
    }
    if entry == "quilt.mod.json" {
        if let Some(fields) = get(parsed.as_ref()) {
            let loader = fields.get("quilt_loader").and_then(Value::as_object);
            let metadata = loader
                .and_then(|loader| loader.get("metadata"))
                .and_then(Value::as_object);
            let loader_id = loader.map(|loader| loader.get("id")).unwrap_or(None);
            let name = metadata
                .and_then(|meta| meta.get("name"))
                .filter(|value| py_truthy(value))
                .or(loader_id)
                .map(text_value);
            let version = loader
                .and_then(|loader| loader.get("version"))
                .map(text_value);
            let author = metadata
                .and_then(|meta| meta.get("contributors"))
                .map(author_value);
            let icon = metadata
                .and_then(|meta| meta.get("icon"))
                .or_else(|| fields.get("icon"))
                .map(text_value);
            return metadata_fields(name, version, author, icon);
        }
        return Vec::new();
    }
    if entry == "mcmod.info" {
        if let Some(value) = parsed.as_ref() {
            let item = match value {
                Value::Array(items) => items.first(),
                other => Some(other),
            };
            if let Some(fields) = get(item) {
                let name = fields
                    .get("displayName")
                    .filter(|value| py_truthy(value))
                    .or_else(|| fields.get("name"))
                    .map(text_value);
                let version = fields.get("version").map(text_value);
                let author = fields
                    .get("authorList")
                    .or_else(|| fields.get("authors"))
                    .map(author_value);
                let icon = fields.get("logoFile").map(text_value);
                return metadata_fields(name, version, author, icon);
            }
        }
        return Vec::new();
    }
    if entry == "pack.mcmeta" {
        if let Some(fields) = get(parsed.as_ref()) {
            let pack = fields.get("pack").and_then(Value::as_object);
            let name = pack
                .and_then(|pack| pack.get("description"))
                .map(text_value);
            let version = pack
                .and_then(|pack| pack.get("pack_format"))
                .map(text_value);
            return metadata_fields(name, version, None, None);
        }
    }
    Vec::new()
}

/// Option<&Value> → Option<&Map>: ас-объект манифеста или ничего.
fn object_of(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn metadata_fields(
    name: Option<String>,
    version: Option<String>,
    author: Option<String>,
    icon: Option<String>,
) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    if let Some(value) = name {
        fields.push(("display_name".to_owned(), value));
    }
    if let Some(value) = version {
        fields.push(("version".to_owned(), value));
    }
    if let Some(value) = author {
        fields.push(("author".to_owned(), value));
    }
    if let Some(value) = icon {
        fields.push(("icon_entry".to_owned(), value));
    }
    fields
}

/// str(value или text/translate) — как _text_value в Python.
fn text_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Object(fields) => {
            let direct = fields.get("text").or_else(|| fields.get("translate"));
            direct.map(text_value).unwrap_or_default()
        }
        Value::Array(items) => items
            .iter()
            .map(text_value)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn author_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Object(fields) => fields
            .get("name")
            .map(|item| py_display(item).trim().to_owned())
            .unwrap_or_default(),
        Value::Array(items) => items
            .iter()
            .map(author_value)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

fn image_data_url(path: &Path) -> String {
    if is_symlink(path) || !path.is_file() {
        return String::new();
    }
    let Ok(meta) = fs::symlink_metadata(path) else {
        return String::new();
    };
    if meta.len() > 2 * 1024 * 1024 {
        return String::new();
    }
    let Ok(raw) = fs::read(path) else {
        return String::new();
    };
    if !raw.starts_with(b"\x89PNG\r\n\x1a\n") {
        return String::new();
    }
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

// ------------------------------------------------------------- файловое ----

fn write_instance_settings(path: &Path, values: &Value) -> Result<(), String> {
    let mut payload = Map::new();
    for key in INSTANCE_REGULAR_KEYS {
        payload.insert(
            key.to_owned(),
            values.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    atomic_json_write(path, &Value::Object(payload), true)
}

fn sidecar_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}{PROJECT_METADATA_SUFFIX}"))
}

fn walk_paths(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    children.sort_by_key(|path| path.to_string_lossy().to_lowercase());
    for child in children {
        let symlink = is_symlink(&child);
        if !symlink && child.is_dir() {
            walk_paths(&child, out);
        }
        out.push(child);
    }
}

fn directory_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let item = entry.path();
        if is_symlink(&item) {
            continue;
        }
        if item.is_dir() {
            total += directory_size(&item);
        } else if item.is_file() {
            if let Ok(meta) = fs::metadata(&item) {
                total += meta.len();
            }
        }
    }
    total
}

fn unique_file(directory: &Path, file_name: &str) -> PathBuf {
    let candidate = directory.join(file_name);
    if fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_name.to_owned());
    let suffix = Path::new(file_name)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    let mut index = 2u32;
    loop {
        let candidate = directory.join(format!("{stem}-{index}{suffix}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        index += 1;
    }
}

fn create_private_dir_all(directory: &Path) -> std::io::Result<()> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent).map_err(|error| error.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Некорректный путь файла ресурса".to_owned())?;
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
        let descriptor = unsafe { libc::open(c_path.as_ptr(), flags, 0o600) };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err("Файл ресурса уже существует".to_owned())
            } else {
                Err(error.to_string())
            };
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(descriptor) };
        if let Err(error) = file.write_all(content) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(error.to_string());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(content) {
                    let _ = fs::remove_file(path);
                    return Err(error.to_string());
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err("Файл ресурса уже существует".to_owned())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

// ------------------------------------------------------------------ NBT ----

fn read_nbt(path: &Path) -> Result<Value, String> {
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = open_nbt_file(path)?;
    let meta = file
        .metadata()
        .map_err(|_| format!("Не удалось прочитать NBT: {}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_NBT_FILE_BYTES as u64 {
        return Err(format!(
            "NBT-файл слишком большой или имеет неверный тип: {file_name}"
        ));
    }
    let mut raw = Vec::new();
    file.take((MAX_NBT_FILE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| format!("Не удалось прочитать NBT: {}", path.display()))?;
    if raw.len() > MAX_NBT_FILE_BYTES {
        return Err(format!("NBT-файл слишком большой: {file_name}"));
    }
    if raw.starts_with(b"\x1f\x8b") {
        raw = gunzip_bounded(&raw, MAX_NBT_DECOMPRESSED_BYTES, &file_name)?;
    }
    let value = NbtReader::new(&raw)
        .read_root()
        .map_err(|_| format!("Некорректный NBT-файл: {file_name}"))?;
    Ok(if value.is_object() { value } else { json!({}) })
}

#[cfg(unix)]
fn open_nbt_file(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| format!("Не удалось прочитать NBT: {}", path.display()))
}

#[cfg(not(unix))]
fn open_nbt_file(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::open(path).map_err(|_| format!("Не удалось прочитать NBT: {}", path.display()))
}

/// счётчик прочитанных байт — GzDecoder не отдаёт total_in, а Python-
/// воркер отклоняет NBT с мусором после gzip-потока (unused_data).
struct CountingReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl Read for CountingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = &self.data[self.position..];
        let count = remaining.len().min(buf.len());
        buf[..count].copy_from_slice(&remaining[..count]);
        self.position += count;
        Ok(count)
    }
}

fn gunzip_bounded(raw: &[u8], limit: usize, file_name: &str) -> Result<Vec<u8>, String> {
    let mut counter = CountingReader {
        data: raw,
        position: 0,
    };
    let decoder = GzDecoder::new(&mut counter);
    let mut out = Vec::new();
    decoder
        .take((limit + 1) as u64)
        .read_to_end(&mut out)
        .map_err(|_| format!("Некорректный сжатый NBT-файл: {file_name}"))?;
    let consumed = counter.position;
    if out.len() > limit || consumed < raw.len() {
        return Err(format!(
            "Распакованный NBT-файл слишком большой или повреждён: {file_name}"
        ));
    }
    Ok(out)
}

struct NbtReader<'a> {
    raw: &'a [u8],
    offset: usize,
    nodes: usize,
}

impl<'a> NbtReader<'a> {
    fn new(raw: &'a [u8]) -> Self {
        Self {
            raw,
            offset: 0,
            nodes: 0,
        }
    }

    fn read_root(&mut self) -> Result<Value, ()> {
        let tag = self.byte()?;
        if tag == 0 {
            return Ok(json!({}));
        }
        self.string()?;
        self.payload(tag, 0)
    }

    fn payload(&mut self, tag: u8, depth: usize) -> Result<Value, ()> {
        self.nodes += 1;
        if depth > MAX_NBT_DEPTH || self.nodes > MAX_NBT_NODES {
            return Err(());
        }
        match tag {
            1 => Ok(json!(self.signed(1)?)),
            2 => Ok(json!(self.signed(2)?)),
            3 => Ok(json!(self.signed(4)?)),
            4 => Ok(json!(self.signed(8)?)),
            5 => Ok(json!(
                f32::from_be_bytes(self.take(4)?.try_into().unwrap()) as f64
            )),
            6 => Ok(json!(f64::from_be_bytes(self.take(8)?.try_into().unwrap()))),
            7 => {
                let length = self.signed(4)?;
                if length < 0 {
                    return Err(());
                }
                let bytes = self.take(length as usize)?;
                Ok(Value::Array(
                    bytes.iter().map(|byte| json!(*byte as i64)).collect(),
                ))
            }
            8 => Ok(json!(self.string()?)),
            9 => {
                let child = self.byte()?;
                let length = self.signed(4)?;
                if !(0..=MAX_NBT_COLLECTION_ITEMS).contains(&length) {
                    return Err(());
                }
                let mut items = Vec::new();
                for _ in 0..length.max(0) {
                    items.push(self.payload(child, depth + 1)?);
                }
                Ok(Value::Array(items))
            }
            10 => {
                let mut value = Map::new();
                loop {
                    let child = self.byte()?;
                    if child == 0 {
                        return Ok(Value::Object(value));
                    }
                    let name = self.string()?;
                    value.insert(name, self.payload(child, depth + 1)?);
                }
            }
            11 | 12 => {
                let size = if tag == 11 { 4 } else { 8 };
                let length = self.signed(4)?;
                if !(0..=MAX_NBT_COLLECTION_ITEMS).contains(&length) {
                    return Err(());
                }
                let mut items = Vec::new();
                for _ in 0..length {
                    items.push(json!(self.signed(size)?));
                }
                Ok(Value::Array(items))
            }
            _ => Err(()),
        }
    }

    fn byte(&mut self) -> Result<u8, ()> {
        Ok(self.take(1)?[0])
    }

    fn signed(&mut self, size: usize) -> Result<i64, ()> {
        let bytes = self.take(size)?;
        let mut value: i64 = 0;
        for byte in bytes {
            value = (value << 8) | *byte as i64;
        }
        // знаковое расширение от размера поля
        let shift = 64 - size * 8;
        Ok((value << shift) >> shift)
    }

    fn string(&mut self) -> Result<String, ()> {
        let length = u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as usize;
        if length > MAX_NBT_STRING_BYTES {
            return Err(());
        }
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_owned())
            .map_err(|_| ())
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8], ()> {
        if self.offset + size > self.raw.len() {
            return Err(());
        }
        let value = &self.raw[self.offset..self.offset + size];
        self.offset += size;
        Ok(value)
    }
}

// --------------------------------------------------------------- утилиты ----

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn mtime_seconds(meta: &fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

fn strip_prefix_lossy(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn as_positive_int(value: &Value) -> Option<i64> {
    coerce_int(value).filter(|number| *number > 0)
}

fn coerce_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .ok()
            .or_else(|| text.trim().parse::<f64>().ok().map(|float| float as i64)),
        Value::Bool(flag) => Some(*flag as i64),
        _ => None,
    }
}

/// str() из Python для значений JSON (None → "None" — как в воркере).
fn py_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Null => "None".to_owned(),
        other => other.to_string(),
    }
}

fn py_display_or(value: &Value, fallback: &str) -> String {
    match value {
        Value::Null => fallback.to_owned(),
        other => py_display(other),
    }
}

/// truthiness значений JSON как в Python (None/False/0/""/пустое — ложь).
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        Value::Number(number) => number.as_f64().map(|float| float != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn hex_color_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^#[0-9a-fA-F]{6}$").expect("корректный regex"))
}

fn library_version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._-]+)?$").expect("корректный regex")
    })
}

fn asset_version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)[-_ ]v?\d+(?:\.\d+)+(?:[-+][\w.-]+)?$").expect("корректный regex")
    })
}

// ------------------------------------------------------------ тесты ----

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write as _;

    fn normalize(values: Value) -> Result<Value, String> {
        normalize_instance_settings(values.as_object().expect("объект"))
    }

    fn base_settings() -> Value {
        Value::Object(default_instance_settings_map())
    }

    #[test]
    fn normalize_accepts_defaults() {
        let normalized = normalize(base_settings()).expect("дефолты валидны");
        assert_eq!(normalized["memory"], json!("auto"));
        assert_eq!(normalized["security_profile"], json!("strict"));
        assert_eq!(normalized["security_policy_version"], json!(4));
        assert_eq!(normalized["console_colors"]["error"], json!("#ff657a"));
    }

    #[test]
    fn normalize_formats_memory_and_checks_total() {
        let mut values = base_settings();
        values["memory"] = json!("2 GiB");
        let normalized = normalize(values).expect("память разобрана");
        assert_eq!(normalized["memory"], json!("2GiB"));

        let mut values = base_settings();
        values["memory"] = json!("9999999 GiB");
        let error = normalize(values).expect_err("больше системы");
        assert!(
            error.starts_with("Память экземпляра не может превышать доступные"),
            "{error}"
        );
    }

    #[test]
    fn normalize_rejects_unknown_enums() {
        let mut values = base_settings();
        values["graphics_mode"] = json!("vulkan");
        let error = normalize(values).expect_err("graphics mode");
        assert_eq!(error, "Неизвестный graphics mode: vulkan");

        let mut values = base_settings();
        values["security_profile"] = json!("paranoid");
        let error = normalize(values).expect_err("security profile");
        assert_eq!(error, "Неизвестный security profile: paranoid");
    }

    #[test]
    fn normalize_clamps_windows_and_requires_numbers() {
        let mut values = base_settings();
        values["window_width"] = json!(99999);
        values["window_height"] = json!("100");
        let normalized = normalize(values).expect("числовые поля");
        assert_eq!(normalized["window_width"], json!(7680));
        assert_eq!(normalized["window_height"], json!(320));

        let mut values = base_settings();
        values["window_width"] = json!("wide");
        let error = normalize(values).expect_err("не число");
        assert_eq!(error, "window_width должен быть числом");
    }

    #[test]
    fn normalize_sanitizes_colors_and_overrides() {
        let mut values = base_settings();
        values["console_colors"] = json!({
            "error": "#FF657A",
            "info": "не цвет",
            "extra": "#000000",
        });
        let normalized = normalize(values).expect("цвета");
        assert_eq!(normalized["console_colors"]["error"], json!("#ff657a"));
        assert_eq!(normalized["console_colors"]["info"], json!("#e6edf7"));
        assert!(normalized["console_colors"].get("extra").is_none());

        let mut values = base_settings();
        values["library_overrides"] = json!({
            "org.lwjgl:lwjgl": "3.3.3",
            "org.evil:hack": "1.0.0",
            "org.lwjgl:lwjgl-glfw": "bad-version",
        });
        let normalized = normalize(values).expect("overrides");
        assert_eq!(
            normalized["library_overrides"],
            json!({ "org.lwjgl:lwjgl": "3.3.3" })
        );
    }

    #[test]
    fn normalize_migrates_old_policy_versions() {
        let mut values = base_settings();
        values["security_policy_version"] = json!(3);
        values["security_permissions"] = json!(["files:instance"]);
        let normalized = normalize(values).expect("миграция");
        assert_eq!(
            normalized["security_permissions"],
            json!([
                "files:instance",
                "network:game",
                "desktop:display",
                "desktop:audio"
            ])
        );

        // ручной выбор после v4 не перекрашивается
        let mut values = base_settings();
        values["security_policy_version"] = json!(4);
        values["security_permissions"] = json!(["files:instance"]);
        let normalized = normalize(values).expect("без миграции");
        assert_eq!(
            normalized["security_permissions"],
            json!(["files:instance"])
        );
    }

    #[test]
    fn normalize_sanitizes_preset_and_arguments() {
        let mut values = base_settings();
        values["jvm_preset"] = json!("FPS");
        values["jvm_arguments"] = json!(["-Xmx1g", "", 42]);
        let normalized = normalize(values).expect("аргументы");
        assert_eq!(normalized["jvm_preset"], json!("fps"));
        // profile_arguments фильтрует не-строки — как _profile_arguments
        assert_eq!(normalized["jvm_arguments"], json!(["-Xmx1g"]));
    }

    #[test]
    fn pretty_names_drop_versions_and_separators() {
        assert_eq!(pretty_asset_name("My-Cool-Mod_1.2.4.jar"), "My Cool Mod");
        // версия-суффикс срезается, префикс с +mc… остаётся — как в Python
        assert_eq!(
            pretty_asset_name("sodium-fabric+mc1.21.4-0.6.13.jar"),
            "sodium fabric+mc1.21.4"
        );
        assert_eq!(pretty_asset_name("x.jar.disabled"), "x");
        // имя из одних разделителей — пустое → «Неизвестный ресурс»
        assert_eq!(pretty_asset_name("-.jar"), "Неизвестный ресурс");
    }

    #[test]
    fn icon_urls_require_https_without_credentials() {
        assert_eq!(
            safe_asset_icon_url("https://example.com/icon.png"),
            "https://example.com/icon.png"
        );
        assert_eq!(safe_asset_icon_url("http://example.com/icon.png"), "");
        assert_eq!(safe_asset_icon_url("https://user:pw@example.com/i.png"), "");
        assert_eq!(safe_asset_icon_url("notaurl"), "");
    }

    #[test]
    fn image_sniffing_matches_magic_bytes() {
        let png = asset_image_data_url(b"\x89PNG\r\n\x1a\nrest");
        assert!(png.starts_with("data:image/png;base64,"));
        assert_eq!(asset_image_data_url(b"not an image"), "");
        let jpeg = asset_image_data_url(b"\xff\xd8\xffEE");
        assert!(jpeg.starts_with("data:image/jpeg;base64,"));
    }

    fn nbt_string(value: &str) -> Vec<u8> {
        let bytes = value.as_bytes();
        let mut out = (bytes.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(bytes);
        out
    }

    fn servers_nbt() -> Vec<u8> {
        let mut out = vec![10u8];
        out.extend(nbt_string("")); // имя корня
        out.push(9); // TAG_List "servers"
        out.extend(nbt_string("servers"));
        out.push(10); // элемент — compound
        out.extend((1i32).to_be_bytes());
        // первый (и единственный) сервер
        out.push(8);
        out.extend(nbt_string("ip"));
        out.extend(nbt_string("mc.example.com"));
        out.push(8);
        out.extend(nbt_string("name"));
        out.extend(nbt_string("My Server"));
        out.push(1);
        out.extend(nbt_string("acceptTextures"));
        out.push(1);
        out.push(0); // конец compound сервера
        out.push(0); // конец корня
        out
    }

    #[test]
    fn nbt_reader_parses_servers_dat() {
        let root = NbtReader::new(&servers_nbt())
            .read_root()
            .expect("корректный NBT");
        let servers = root["servers"].as_array().expect("список");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["ip"], json!("mc.example.com"));
        assert_eq!(servers[0]["name"], json!("My Server"));
        assert_eq!(servers[0]["acceptTextures"], json!(1i64));
    }

    #[test]
    fn nbt_reader_accepts_gzip_and_rejects_garbage() {
        let raw = servers_nbt();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw).expect("gzip записан");
        let compressed = encoder.finish().expect("gzip завершён");

        let dir = std::env::temp_dir().join(format!("nivora-nbt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("servers.dat");
        fs::write(&path, &compressed).unwrap();
        let root = read_nbt(&path).expect("gzip NBT читается");
        assert_eq!(root["servers"][0]["ip"], json!("mc.example.com"));

        fs::write(&path, b"\x1f\x8b garbage").unwrap();
        let error = read_nbt(&path).expect_err("битый gzip");
        assert!(error.starts_with("Некорректный сжатый NBT-файл"), "{error}");

        fs::write(&path, b"\x0a\x00\x00").unwrap();
        let error = read_nbt(&path).expect_err("обрыв compound");
        assert!(error.starts_with("Некорректный NBT-файл"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_metadata_reads_fabric_manifest() {
        let dir = std::env::temp_dir().join(format!("nivora-asset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let jar = dir.join("demo.jar");
        {
            let file = fs::File::create(&jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("fabric.mod.json", options).unwrap();
            zip.write_all(
                br#"{"id":"demo","name":"Demo Mod","version":"1.0","icon":"assets/icon.png"}"#,
            )
            .unwrap();
            zip.start_file("assets/icon.png", options).unwrap();
            zip.write_all(b"\x89PNG\r\n\x1a\nfake").unwrap();
            zip.finish().unwrap();
        }
        let metadata = read_archive_metadata(&jar);
        assert_eq!(
            metadata.get("display_name").map(String::as_str),
            Some("Demo Mod")
        );
        assert_eq!(metadata.get("version").map(String::as_str), Some("1.0"));
        let icon_data = metadata.get("icon_data").expect("иконка прочитана");
        assert!(
            icon_data.starts_with("data:image/png;base64,"),
            "{icon_data}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn temp_roots(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("nivora-workspace-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let config = base.join("config");
        let data = base.join("data");
        let cache = base.join("cache");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&cache).unwrap();
        (config, data, cache)
    }

    fn workspace_at_roots(
        config_home: PathBuf,
        data_home: PathBuf,
        cache_home: PathBuf,
    ) -> InstanceWorkspace {
        let roots = super::super::state::LauncherRoots {
            config_home,
            data_home,
            cache_home,
        };
        let config = ConfigStore::from_paths(super::super::config::Paths::from_roots(&roots))
            .expect("конфиг");
        let instances = InstanceStore::new(&config).expect("экземпляры");
        InstanceWorkspace { instances }
    }

    #[test]
    fn settings_roundtrip_splits_security_keys() {
        let (config_home, data_home, cache_home) = temp_roots("settings");
        let game = data_home.join("game");
        let instance = game.join("instances").join("demo");
        fs::create_dir_all(&instance).unwrap();
        // легаси-файл с защитными ключами внутри — их выносят в защищённое
        // хранилище и перезаписывают файл без них.
        fs::write(
            instance.join(INSTANCE_SETTINGS_FILE),
            serde_json::to_string(&json!({
                "memory": "1024 MiB",
                "java_path": "/usr/bin/java",
                "security_profile": "off",
            }))
            .unwrap(),
        )
        .unwrap();

        let workspace = workspace_at_roots(config_home.clone(), data_home, cache_home);
        let settings = workspace.settings("demo").expect("настройки прочитаны");
        assert_eq!(settings["memory"], json!("1GiB"));
        // легаси-защищённые ключи из файла не переносятся, а сбрасываются
        // на дефолты защищённого хранилища — и файл переписывается без них
        assert_eq!(settings["java_path"], json!(""));
        assert_eq!(settings["security_profile"], json!("strict"));

        let rewritten = fs::read_to_string(instance.join(INSTANCE_SETTINGS_FILE)).unwrap();
        let rewritten: Value = serde_json::from_str(&rewritten).unwrap();
        assert!(rewritten.get("java_path").is_none());
        assert!(rewritten.get("security_profile").is_none());
        assert_eq!(rewritten["memory"], json!("1GiB"));

        let protected_dir = config_home.join("instance-security");
        assert!(protected_dir.join("demo.json").is_file());
        let _ = fs::remove_dir_all(config_home.parent().unwrap());
    }

    #[test]
    fn snapshot_lists_assets_worlds_and_servers() {
        let (config_home, data_home, cache_home) = temp_roots("snapshot");
        let game = data_home.join("game");
        let instance = game.join("instances").join("demo");
        let mods = instance.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("alpha.jar"), b"jar-bytes").unwrap();
        fs::write(mods.join("beta.jar.disabled"), b"disabled-bytes").unwrap();
        fs::write(
            mods.join(".beta.jar.disabled.trial-project.json"),
            br#"{"project_id":"beta"}"#,
        )
        .unwrap();

        let saves = instance.join("saves").join("World");
        fs::create_dir_all(&saves).unwrap();
        fs::write(saves.join("level.dat"), &servers_nbt()).unwrap();
        let servers = instance.join("servers.dat");
        fs::write(&servers, &servers_nbt()).unwrap();

        let workspace = workspace_at_roots(config_home.clone(), data_home, cache_home);
        let snapshot = workspace.snapshot("demo").expect("снимок");
        assert_eq!(snapshot["instance"]["name"], json!("demo"));
        let assets = snapshot["assets"]["mods"].as_array().expect("моды");
        assert_eq!(assets.len(), 2); // сайдкар-метаданные не считаются ресурсом
        let first = &assets[0];
        assert_eq!(first["name"], json!("alpha.jar"));
        assert_eq!(first["enabled"], json!(true));
        assert_eq!(assets[1]["enabled"], json!(false));
        let worlds = snapshot["worlds"].as_array().expect("миры");
        assert_eq!(worlds.len(), 1);
        assert_eq!(worlds[0]["id"], json!("World"));
        // LevelName в NBT нет — имя падает на имя каталога
        assert_eq!(worlds[0]["name"], json!("World"));
        let server_list = snapshot["servers"].as_array().expect("серверы");
        assert_eq!(server_list.len(), 1);
        assert_eq!(server_list[0]["address"], json!("mc.example.com"));
        assert_eq!(server_list[0]["id"], json!("server-0"));
        let _ = fs::remove_dir_all(config_home.parent().unwrap());
    }

    #[test]
    fn toggle_and_remove_mods_through_paths() {
        let (config_home, data_home, cache_home) = temp_roots("toggle");
        let game = data_home.join("game");
        let instance = game.join("instances").join("demo");
        let mods = instance.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("mod.jar"), b"mod").unwrap();
        fs::write(
            mods.join(".mod.jar.trial-project.json"),
            br#"{"project_id":"mod"}"#,
        )
        .unwrap();

        let workspace = workspace_at_roots(config_home.clone(), data_home, cache_home);
        let snapshot = workspace
            .toggle_mod("demo", "mod.jar", false)
            .expect("выключение");
        assert!(mods.join("mod.jar.disabled").is_file());
        assert!(mods.join(".mod.jar.disabled.trial-project.json").is_file());
        assert!(snapshot["settings"]["memory"].is_string());

        // повторное выключение — no-op снимок
        workspace
            .toggle_mod("demo", "mod.jar.disabled", false)
            .expect("no-op");
        assert!(mods.join("mod.jar.disabled").is_file());

        workspace
            .remove_asset("demo", "mods", "mod.jar.disabled")
            .expect("удаление");
        assert!(!mods.join("mod.jar.disabled").exists());
        assert!(!mods.join(".mod.jar.disabled.trial-project.json").exists());
        let _ = fs::remove_dir_all(config_home.parent().unwrap());
    }

    #[test]
    fn import_asset_rejects_unsafe_names() {
        let (config_home, data_home, cache_home) = temp_roots("import");
        let game = data_home.join("game");
        let instance = game.join("instances").join("demo");
        fs::create_dir_all(&instance).unwrap();

        let workspace = workspace_at_roots(config_home.clone(), data_home, cache_home);
        let error = workspace
            .import_asset("demo", "mods", "../escape.jar", b"x")
            .expect_err("путь с каталогом");
        assert_eq!(error, "Небезопасное имя импортируемого файла");

        let snapshot = workspace
            .import_asset("demo", "mods", "new.jar", b"content")
            .expect("импорт");
        assert!(instance.join("mods").join("new.jar").is_file());
        // второй файл с тем же именем получает суффикс-2
        workspace
            .import_asset("demo", "mods", "new.jar", b"content-2")
            .expect("повтор");
        assert!(instance.join("mods").join("new-2.jar").is_file());
        assert_eq!(snapshot["assets"]["mods"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(config_home.parent().unwrap());
    }

    #[test]
    fn java_only_guard_and_running_check() {
        let (config_home, data_home, cache_home) = temp_roots("guard");
        let game = data_home.join("game");
        let instance = game.join("instances").join("demo");
        fs::create_dir_all(&instance).unwrap();
        fs::write(
            instance.join(".nivora-launcher-instance.json"),
            serde_json::to_string(&json!({"kind": "server", "game_edition": "java"})).unwrap(),
        )
        .unwrap();

        let workspace = workspace_at_roots(config_home.clone(), data_home, cache_home);
        let error = workspace.settings("demo").expect_err("не клиент");
        assert_eq!(
            error,
            "Эта рабочая область предназначена только для Minecraft Java Edition"
        );
        let _ = fs::remove_dir_all(config_home.parent().unwrap());
    }
}
