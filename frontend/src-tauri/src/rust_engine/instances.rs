use base64::Engine;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::{default_settings_object, set_value_on, ConfigStore};
use super::constants::{
    MAX_INSTANCE_LIVE_STATE_JSON_BYTES, MAX_INSTANCE_PROFILE_JSON_BYTES, MAX_INSTANCE_VERSION_JSON_BYTES,
    MAX_SERVER_CONFIG_JSON_BYTES,
};
use super::net::atomic_json_write;
use super::state::{
    home_dir, instance_running, read_bounded_json, resolve_instance_path, safe_instance_name,
    MAX_SETTINGS_JSON_BYTES,
};

pub(crate) const INSTANCE_METADATA_FILE: &str = ".nivora-launcher-instance.json";
const LIVE_STATE_FILE: &str = ".nivora-launcher-live.json";
const SERVER_CONFIG_FILE: &str = ".nivora-launcher-server.json";
const INSTANCE_ASSETS_DIR: &str = ".nivora-launcher-assets";
const PROTECTED_SETTINGS_FORMAT: i64 = 1;
const MAX_PROTECTED_SETTINGS_BYTES: u64 = 64 * 1024;

#[derive(Clone)]
#[derive(Debug)]
pub(crate) struct InstanceInfo {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) version_id: String,
    pub(crate) minecraft_version: String,
    pub(crate) loader: String,
    pub(crate) loader_version: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) group: String,
    pub(crate) icon: String,
    pub(crate) banner: String,
    pub(crate) played_seconds: i64,
    pub(crate) kind: String,
    pub(crate) game_edition: String,
    pub(crate) bedrock_profile: String,
    pub(crate) server_core: String,
    pub(crate) server_core_version: String,
    pub(crate) server_port: i64,
    pub(crate) server_eula: bool,
    pub(crate) running: bool,
    pub(crate) notes: String,
    pub(crate) tags: Vec<String>,
}

impl InstanceInfo {
    /// публичное представление экземпляра
    pub(crate) fn public(&self) -> Value {
        json!({
            "name": self.name,
            "display_name": if self.display_name.is_empty() { self.name.clone() } else { self.display_name.clone() },
            "description": self.description,
            "group": self.group,
            "icon": self.icon,
            "banner": self.banner,
            "played_seconds": self.played_seconds,
            "version_id": self.version_id,
            "minecraft_version": self.minecraft_version,
            "loader": self.loader,
            "loader_version": self.loader_version,
            "path": self.path.to_string_lossy(),
            "kind": self.kind,
            "game_edition": self.game_edition,
            "bedrock_profile": self.bedrock_profile,
            "server_core": self.server_core,
            "server_core_version": self.server_core_version,
            "server_port": self.server_port,
            "server_eula": self.server_eula,
            "running": self.running,
            "notes": self.notes,
            "tags": self.tags,
        })
    }
}

pub(crate) struct ExternalImportCandidate {
    pub(crate) launcher: String,
    pub(crate) source_path: PathBuf,
    pub(crate) game_path: PathBuf,
    pub(crate) display_name: String,
    pub(crate) suggested_name: String,
    pub(crate) version_id: String,
    pub(crate) minecraft_version: String,
    pub(crate) loader: String,
    pub(crate) notes: String,
}

impl ExternalImportCandidate {
    pub(crate) fn public(&self) -> Value {
        json!({
            "launcher": self.launcher,
            "display_name": self.display_name,
            "suggested_name": self.suggested_name,
            "source_path": self.source_path.to_string_lossy(),
            "game_path": self.game_path.to_string_lossy(),
            "version_id": self.version_id,
            "minecraft_version": self.minecraft_version,
            "loader": self.loader,
            "notes": self.notes,
        })
    }
}

/// поля профиля, которые можно изменить через update_profile
#[derive(Default)]
pub(crate) struct ProfileUpdate {
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) group: String,
    pub(crate) icon_file_name: String,
    pub(crate) icon_data: String,
    pub(crate) banner_file_name: String,
    pub(crate) banner_data: String,
    pub(crate) clear_icon: bool,
    pub(crate) clear_banner: bool,
    pub(crate) notes: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
}

pub(crate) struct InstanceStore {
    game_dir: PathBuf,
    root: PathBuf,
    config_home: PathBuf,
    instance_security_dir: PathBuf,
}

impl InstanceStore {
    pub(crate) fn new(config: &ConfigStore) -> Result<Self, String> {
        let game_dir = config.game_dir()?;
        let root = game_dir.join("instances");
        if is_symlink(&root) {
            return Err("Каталог экземпляров является ссылкой или junction".to_owned());
        }
        fs::create_dir_all(&root).map_err(|_| "Каталог экземпляров недоступен".to_owned())?;
        if is_symlink(&root) || !root.is_dir() {
            return Err("Каталог экземпляров недоступен".to_owned());
        }
        super::platform::restrict_dir(&root);
        Ok(Self {
            root,
            game_dir,
            config_home: config.paths.config_home.clone(),
            instance_security_dir: config.paths.instance_security_dir.clone(),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn list(&self) -> Vec<InstanceInfo> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| !is_symlink(path) && path.is_dir())
            .collect();
        paths.sort_by_key(|path| file_name_of(path).to_lowercase());
        // проверка каталогов экземпляров нагружает диск, поэтому выполняем её параллельно
        // результат сохраняет исходный порядок независимо от потоков
        let workers = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
            .min(8)
            .max(1);
        if workers <= 1 || paths.len() < 8 {
            return paths.iter().map(|path| self.inspect_with(path, false)).collect();
        }
        let chunk = paths.len().div_ceil(workers);
        let slices: Vec<Vec<InstanceInfo>> = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for slice in paths.chunks(chunk) {
                handles.push(scope.spawn(move || {
                    slice.iter().map(|path| self.inspect_with(path, false)).collect::<Vec<_>>()
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_default())
                .collect()
        });
        slices.into_iter().flatten().collect()
    }

    pub(crate) fn get(&self, name: &str) -> Result<InstanceInfo, String> {
        let path = resolve_instance_path(&self.game_dir, name)
            .ok_or_else(|| format!("Экземпляр не найден: {name}"))?;
        Ok(self.inspect(&path))
    }

    pub(crate) fn delete(&self, name: &str) -> Result<(), String> {
        let value = self.get(name)?;
        if instance_running(&value.path) {
            return Err("Нельзя удалить запущенный экземпляр".to_owned());
        }
        // профиль Bedrock очищает собственные данные
        if value.game_edition == "bedrock" {
            super::bedrock::delete_profile(&value)?;
        }
        remove_tree(&value.path)?;
        self.clear_protected_settings(&value.name);
        Ok(())
    }

    pub(crate) fn duplicate(&self, name: &str, target_name: &str) -> Result<InstanceInfo, String> {
        let source = self.get(name)?;
        if source.game_edition == "bedrock" {
            return Err("Профиль Bedrock нельзя копировать вместе с данными входа Microsoft".to_owned());
        }
        let target = self.root.join(safe_instance_name(target_name));
        if fs::symlink_metadata(&target).is_ok() {
            return Err(format!("Экземпляр уже существует: {}", file_name_of(&target)));
        }
        self.clear_protected_settings(&file_name_of(&target));
        assert_no_symlinks(&source.path)?;
        copy_tree(&source.path, &target)?;
        super::platform::restrict_dir(&target);
        // копируем путь к Java и профиль безопасности, так как они хранятся вне экземпляра
        let defaults = instance_security_defaults();
        if let Ok(values) = self.load_protected_settings(&source.name, &defaults) {
            let _ = self.save_protected_settings(&file_name_of(&target), &values);
        }
        Ok(self.inspect(&target))
    }

    /// ярлыки создает нативный модуль shortcuts.rs (instance_shortcut_create):
    /// единый реестр, иконки и --launch-instance на всех платформах

    pub(crate) fn update_profile(&self, name: &str, values: &ProfileUpdate) -> Result<InstanceInfo, String> {
        let value = self.get(name)?;
        let group = take_chars(values.group.trim(), 40);
        if !group.is_empty() && !self.groups().contains(&group) {
            return Err("Сначала создайте выбранную группу".to_owned());
        }
        let metadata_path = value.path.join(INSTANCE_METADATA_FILE);
        let mut metadata = read_profile(&value.path);
        let display_name = take_chars(values.display_name.trim(), 80);
        metadata.insert("display_name".into(), json!(if display_name.is_empty() { value.name.clone() } else { display_name }));
        metadata.insert("description".into(), json!(take_chars(values.description.trim(), 500)));
        metadata.insert("group".into(), json!(group));
        metadata.insert(
            "played_seconds".into(),
            json!(nonnegative_int(metadata.get("played_seconds").unwrap_or(&Value::Null))),
        );
        if let Some(notes) = &values.notes {
            metadata.insert("notes".into(), json!(take_chars(notes.trim(), 4000)));
        }
        if let Some(tags) = &values.tags {
            metadata.insert("tags".into(), json!(normalize_tags(&json!(tags))));
        }
        let assets = value.path.join(INSTANCE_ASSETS_DIR);
        if values.clear_icon {
            remove_profile_assets(&assets, "icon")?;
            metadata.remove("icon_file");
        } else if !values.icon_data.is_empty() {
            let file = write_profile_asset(&assets, "icon", &values.icon_file_name, &values.icon_data, 2 * 1024 * 1024)?;
            metadata.insert("icon_file".into(), json!(file));
        }
        if values.clear_banner {
            remove_profile_assets(&assets, "banner")?;
            metadata.remove("banner_file");
        } else if !values.banner_data.is_empty() {
            let file = write_profile_asset(&assets, "banner", &values.banner_file_name, &values.banner_data, 8 * 1024 * 1024)?;
            metadata.insert("banner_file".into(), json!(file));
        }
        atomic_json_write(&metadata_path, &Value::Object(metadata), true)?;
        self.get(name)
    }

    pub(crate) fn add_played_seconds(&self, path: &Path, seconds: i64) -> Result<(), String> {
        let root = self.root.canonicalize().map_err(|error| format!("{error}"))?;
        let target = path.canonicalize().map_err(|error| format!("{error}"))?;
        if !within(&target, &root) {
            return Err("Путь экземпляра находится вне каталога игры".to_owned());
        }
        let mut metadata = read_profile(&target);
        let played = nonnegative_int(metadata.get("played_seconds").unwrap_or(&Value::Null));
        metadata.insert("played_seconds".into(), json!(played + seconds.max(0)));
        atomic_json_write(&target.join(INSTANCE_METADATA_FILE), &Value::Object(metadata), true)
    }

    pub(crate) fn groups(&self) -> Vec<String> {
        self.settings_value()
            .get("instance_groups")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    }

    pub(crate) fn create_group(&self, name: &str) -> Result<Vec<String>, String> {
        let value = take_chars(name.trim(), 40);
        if value.is_empty() {
            return Err("Название группы не может быть пустым".to_owned());
        }
        let mut groups = self.groups();
        if !groups.contains(&value) {
            groups.push(value);
            let mut settings = self.settings_value();
            set_value_on(&mut settings, "instance_groups", &json!(groups))?;
            self.write_settings(&settings)?;
        }
        Ok(self.groups())
    }

    pub(crate) fn delete_group(&self, name: &str) -> Result<Vec<String>, String> {
        let value = name.trim();
        if self.list().iter().any(|item| item.group == value) {
            return Err("Нельзя удалить группу, пока в ней есть экземпляры".to_owned());
        }
        let remaining: Vec<String> = self.groups().into_iter().filter(|item| item != value).collect();
        let mut settings = self.settings_value();
        set_value_on(&mut settings, "instance_groups", &json!(remaining))?;
        self.write_settings(&settings)?;
        Ok(self.groups())
    }

    pub(crate) fn rename_group(&self, old_name: &str, new_name: &str) -> Result<Vec<String>, String> {
        let old_value = old_name.trim();
        let value = take_chars(new_name.trim(), 40);
        if value.is_empty() {
            return Err("Название группы не может быть пустым".to_owned());
        }
        let mut groups = self.groups();
        if !groups.iter().any(|group| group == old_value) {
            return Err("Группа не найдена".to_owned());
        }
        if value == old_value {
            return Ok(self.groups());
        }
        if groups.contains(&value) {
            return Err("Группа с таким названием уже существует".to_owned());
        }
        let index = groups.iter().position(|item| item == old_value).expect("проверено выше");
        groups[index] = value.clone();
        let pinned: Vec<Value> = self
            .settings_value()
            .get("pinned_instance_groups")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        if item.as_str() == Some(old_value) { json!(value) } else { item.clone() }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut settings = self.settings_value();
        set_value_on(&mut settings, "instance_groups", &json!(groups))?;
        set_value_on(&mut settings, "pinned_instance_groups", &Value::Array(pinned))?;
        self.write_settings(&settings)?;
        for info in self.list() {
            if info.group == old_value {
                let update = ProfileUpdate {
                    display_name: info.display_name.clone(),
                    description: info.description.clone(),
                    group: value.clone(),
                    ..Default::default()
                };
                self.update_profile(&info.name, &update)?;
            }
        }
        Ok(self.groups())
    }

    pub(crate) fn list_import_candidates(&self) -> Vec<ExternalImportCandidate> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut values: Vec<ExternalImportCandidate> = Vec::new();
        for candidate in discover_external_instance_candidates(&home_dir()) {
            // resolve() здесь нестрогий: достаточно canonicalize существующего
            let Some(resolved) = expand_home(&candidate.source_path).canonicalize().ok() else {
                continue;
            };
            if !seen.insert(resolved) {
                continue;
            }
            values.push(candidate);
        }
        values
    }

    pub(crate) fn import_external_instance(
        &self,
        source_path: &str,
        target_name: Option<&str>,
    ) -> Result<InstanceInfo, String> {
        let requested = expand_home(Path::new(source_path));
        let requested_resolved = requested
            .canonicalize()
            .map_err(|_| "Источник импорта недоступен".to_owned())?;
        let candidate = self
            .list_import_candidates()
            .into_iter()
            .find(|item| expand_home(&item.source_path).canonicalize().ok().as_deref() == Some(requested_resolved.as_path()))
            .ok_or_else(|| "Источник импорта не найден. Обновите список и попробуйте снова.".to_owned())?;
        let source_root = expand_home(&candidate.game_path);
        if is_symlink(&source_root) || !source_root.is_dir() {
            return Err("Каталог исходного экземпляра недоступен".to_owned());
        }
        let mut label = target_name.map(str::to_owned).unwrap_or_default();
        if label.is_empty() {
            label = candidate.suggested_name.clone();
        }
        if label.is_empty() {
            label = candidate.display_name.clone();
        }
        if label.is_empty() {
            label = file_name_of(&candidate.source_path);
        }
        let target = self.root.join(safe_instance_name(label.trim()));
        if fs::symlink_metadata(&target).is_ok() {
            return Err(format!("Экземпляр уже существует: {}", file_name_of(&target)));
        }
        self.clear_protected_settings(&file_name_of(&target));
        assert_no_symlinks(&source_root)?;
        copy_tree(&source_root, &target)?;
        super::platform::restrict_dir(&target);
        ensure_import_version_metadata(&target, &candidate)?;
        let mut metadata = read_profile(&target);
        let display_name = take_chars(candidate.display_name.trim(), 80);
        metadata.insert(
            "display_name".into(),
            json!(if display_name.is_empty() { file_name_of(&target) } else { display_name }),
        );
        let description_source = if candidate.notes.trim().is_empty() {
            format!("Импортировано из {}.", candidate.launcher)
        } else {
            candidate.notes.trim().to_owned()
        };
        metadata.insert("description".into(), json!(take_chars(&description_source, 500)));
        metadata.insert("kind".into(), json!("client"));
        metadata.insert(
            "played_seconds".into(),
            json!(nonnegative_int(metadata.get("played_seconds").unwrap_or(&Value::Null))),
        );
        atomic_json_write(&target.join(INSTANCE_METADATA_FILE), &Value::Object(metadata), true)?;
        Ok(self.inspect(&target))
    }

    fn inspect(&self, path: &Path) -> InstanceInfo {
        self.inspect_with(path, true)
    }

    /// баннеры читаются только для страницы экземпляра, чтобы не загружать их в списке
    fn inspect_with(&self, path: &Path, include_banner: bool) -> InstanceInfo {
        let metadata = read_profile(path);
        let mut info = profile_public(path, &metadata, epoch_seconds(), include_banner);
        if info.game_edition == "bedrock" {
            return info;
        }
        if let Some((version_id, minecraft_version, loader)) = latest_version(path) {
            info.version_id.clone_from(&version_id);
            info.minecraft_version.clone_from(&minecraft_version);
            info.loader.clone_from(&loader);
            info.loader_version = loader_version_of(path, &version_id, &minecraft_version, &loader);
        }
        info
    }

    fn settings_value(&self) -> Value {
        read_bounded_json(&self.config_home.join("settings.json"), MAX_SETTINGS_JSON_BYTES)
            .unwrap_or_else(default_settings_object)
    }

    fn write_settings(&self, settings: &Value) -> Result<(), String> {
        atomic_json_write(&self.config_home.join("settings.json"), settings, true)
    }

    fn protected_settings_path(&self, instance_name: &str) -> Result<PathBuf, String> {
        let path = self.instance_security_dir.join(format!("{}.json", safe_instance_name(instance_name)));
        if is_symlink(&path) {
            return Err("Защищённые настройки экземпляра являются символьной ссылкой".to_owned());
        }
        Ok(path)
    }

    pub(crate) fn load_protected_settings(&self, instance_name: &str, defaults: &Value) -> Result<Value, String> {
        let path = self.protected_settings_path(instance_name)?;
        if fs::symlink_metadata(&path).is_err() {
            let values = defaults.clone();
            self.save_protected_settings(instance_name, &values)?;
            return Ok(values);
        }
        let meta = fs::symlink_metadata(&path).map_err(|error| format!("{error}"))?;
        if !meta.is_file() || meta.len() > MAX_PROTECTED_SETTINGS_BYTES {
            return Err("Защищённые настройки экземпляра повреждены".to_owned());
        }
        let raw_text = fs::read_to_string(&path)
            .map_err(|error| format!("Не удалось прочитать защищённые настройки экземпляра: {error}"))?;
        let raw: Value = serde_json::from_str(&raw_text)
            .map_err(|error| format!("Не удалось прочитать защищённые настройки экземпляра: {error}"))?;
        let Some(fields) = raw.as_object() else {
            return Err("Защищённые настройки экземпляра имеют неподдерживаемый формат".to_owned());
        };
        if fields.get("format").and_then(Value::as_i64) != Some(PROTECTED_SETTINGS_FORMAT) {
            return Err("Защищённые настройки экземпляра имеют неподдерживаемый формат".to_owned());
        }
        let mut values = defaults.as_object().cloned().unwrap_or_default();
        let keys: Vec<String> = values.keys().cloned().collect();
        for key in keys {
            if let Some(stored) = fields.get(&key) {
                values.insert(key, stored.clone());
            }
        }
        Ok(Value::Object(values))
    }

    pub(crate) fn save_protected_settings(&self, instance_name: &str, values: &Value) -> Result<(), String> {
        let path = self.protected_settings_path(instance_name)?;
        let mut payload = values.as_object().cloned().unwrap_or_default();
        payload.insert("format".into(), json!(PROTECTED_SETTINGS_FORMAT));
        atomic_json_write(&path, &Value::Object(payload), true)
    }

    fn clear_protected_settings(&self, instance_name: &str) {
        if let Ok(path) = self.protected_settings_path(instance_name) {
            if fs::symlink_metadata(&path).is_ok() {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

fn instance_security_defaults() -> Value {
    // настройки безопасности по умолчанию для нового экземпляра
    json!({
        "java_path": "",
        "security_profile": "strict",
        "security_permissions": ["network:game", "files:instance", "desktop:display", "desktop:audio"],
        "security_policy_version": 4,
    })
}

fn read_profile(path: &Path) -> Map<String, Value> {
    read_bounded_json(&path.join(INSTANCE_METADATA_FILE), MAX_INSTANCE_PROFILE_JSON_BYTES)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

pub(crate) fn profile_metadata(path: &Path) -> Map<String, Value> {
    read_profile(path)
}

fn profile_public(path: &Path, metadata: &Map<String, Value>, now: f64, include_banner: bool) -> InstanceInfo {
    let name = file_name_of(path);
    let running_seconds = live_play_seconds(path, now);
    let played = nonnegative_int(metadata.get("played_seconds").unwrap_or(&Value::Null));
    let display_name = take_chars(py_str(metadata.get("display_name").unwrap_or(&Value::Null)).trim(), 80);
    let icon_file = py_str(metadata.get("icon_file").unwrap_or(&Value::Null)).trim().to_owned();
    let banner_file = py_str(metadata.get("banner_file").unwrap_or(&Value::Null)).trim().to_owned();
    // конфигурация сервера влияет на тип экземпляра и помогает определить ядро и порт
    let server_config = read_bounded_json(&path.join(SERVER_CONFIG_FILE), MAX_SERVER_CONFIG_JSON_BYTES)
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({}));
    let server_fields = server_config.as_object();
    let is_server = py_str(metadata.get("kind").unwrap_or(&Value::Null)).trim().eq_ignore_ascii_case("server")
        || server_fields.map(|fields| !fields.is_empty()).unwrap_or(false);
    let mut game_edition = py_str(metadata.get("game_edition").unwrap_or(&json!("java")))
        .trim()
        .to_lowercase();
    if (game_edition != "java" && game_edition != "bedrock") || is_server {
        game_edition = "java".to_owned();
    }
    let icon = if icon_file.is_empty() {
        String::new()
    } else {
        image_data_url(&path.join(INSTANCE_ASSETS_DIR).join(&icon_file), 2 * 1024 * 1024)
    };
    let banner = if banner_file.is_empty() || !include_banner {
        String::new()
    } else {
        image_data_url(&path.join(INSTANCE_ASSETS_DIR).join(&banner_file), 8 * 1024 * 1024)
    };
    InstanceInfo {
        loader: "vanilla".to_owned(),
        bedrock_profile: if game_edition == "bedrock" {
            take_chars(py_str(metadata.get("bedrock_profile").unwrap_or(&Value::Null)).trim(), 64)
        } else {
            String::new()
        },
        server_core: take_chars(
            py_str(py_or(
                metadata.get("server_core"),
                server_fields.and_then(|fields| fields.get("core")),
            ))
            .trim(),
            64,
        ),
        server_core_version: take_chars(
            py_str(py_or(
                metadata.get("server_core_version"),
                server_fields.and_then(|fields| fields.get("version")),
            ))
            .trim(),
            32,
        ),
        server_port: nonnegative_int(py_or(
            metadata.get("server_port"),
            server_fields.and_then(|fields| fields.get("port")),
        )),
        server_eula: py_bool(metadata
            .get("server_eula")
            .or_else(|| server_fields.and_then(|fields| fields.get("eula_accepted")))
            .unwrap_or(&json!(false))),
        kind: if is_server { "server" } else { "client" }.to_owned(),
        display_name: if display_name.is_empty() { name.clone() } else { display_name },
        played_seconds: played + running_seconds,
        running: instance_running(path),
        notes: take_chars(&py_str(metadata.get("notes").unwrap_or(&Value::Null)), 4000),
        tags: normalize_tags(metadata.get("tags").unwrap_or(&Value::Null)),
        name,
        path: path.to_path_buf(),
        version_id: String::new(),
        minecraft_version: String::new(),
        loader_version: String::new(),
        description: take_chars(py_str(metadata.get("description").unwrap_or(&Value::Null)).trim(), 500),
        group: take_chars(py_str(metadata.get("group").unwrap_or(&Value::Null)).trim(), 40),
        game_edition,
        icon,
        banner,
    }
}

/// версия и загрузчик из самого свежего versions/<id>/<id>.json
fn latest_version(path: &Path) -> Option<(String, String, String)> {
    let versions = path.join("versions");
    if is_symlink(&versions) || !versions.is_dir() {
        return None;
    }
    let mut candidates: Vec<(SystemTime, PathBuf, Value)> = Vec::new();
    let directories = fs::read_dir(&versions).ok()?;
    for directory in directories.filter_map(|entry| entry.ok()) {
        let version_dir = directory.path();
        if is_symlink(&version_dir) || !version_dir.is_dir() {
            continue;
        }
        let files = match fs::read_dir(&version_dir) {
            Ok(files) => files,
            Err(_) => continue,
        };
        for file in files.filter_map(|entry| entry.ok()) {
            let metadata_path = file.path();
            if !file_name_of(&metadata_path).ends_with(".json") || is_symlink(&metadata_path) {
                continue;
            }
            let Some(value) = read_bounded_json(&metadata_path, MAX_INSTANCE_VERSION_JSON_BYTES)
                .filter(|value| value.is_object())
            else {
                continue;
            };
            let modified = file.metadata().and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            candidates.push((modified, metadata_path, value));
        }
    }
    // при равенстве сохраняем первый максимальный результат
    let (_, metadata_path, value) = candidates
        .into_iter()
        .reduce(|best, item| if item.0 > best.0 { item } else { best })?;
    let stem = metadata_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut version_id = value
        .get("id")
        .filter(|id| py_truthy(id))
        .map(py_str)
        .unwrap_or_default();
    if version_id.is_empty() {
        version_id = stem.clone();
    }
    if !valid_version_id(&version_id) {
        version_id = stem;
    }
    if !valid_version_id(&version_id) {
        return None;
    }
    let mut minecraft_version = take_chars(
        &py_str(
            value
                .get("inheritsFrom")
                .filter(|item| py_truthy(item))
                .or_else(|| value.get("minecraft").filter(|item| py_truthy(item)))
                .unwrap_or(&Value::Null),
        ),
        80,
    );
    if minecraft_version.is_empty() {
        minecraft_version = version_from_id(&version_id);
    }
    let loader = loader_from_version(&version_id);
    Some((version_id, minecraft_version, loader))
}

/// версия загрузчика: сначала libraries из профиля версии, потом разбор id
pub(crate) fn loader_version_of(path: &Path, version_id: &str, minecraft_version: &str, loader: &str) -> String {
    if !matches!(loader, "fabric" | "forge" | "neoforge" | "quilt") {
        return String::new();
    }
    let version_id = version_id.trim();
    if version_id.is_empty() || [".", ".."].contains(&version_id) || version_id.contains(['/', '\\', '\0']) {
        return String::new();
    }
    let versions = path.join("versions");
    let metadata_path = versions.join(version_id).join(format!("{version_id}.json"));
    let profile = if !is_symlink(path)
        && !is_symlink(&versions)
        && metadata_path.parent().map(|parent| !is_symlink(parent)).unwrap_or(false)
        && !is_symlink(&metadata_path)
    {
        read_bounded_json(&metadata_path, 4 * 1024 * 1024)
            .filter(|value| value.is_object())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let (group, artifact) = match loader {
        "fabric" => ("net.fabricmc", "fabric-loader"),
        "forge" => ("net.minecraftforge", "forge"),
        "neoforge" => ("net.neoforged", "neoforge"),
        _ => ("org.quiltmc", "quilt-loader"),
    };
    if let Some(libraries) = profile.get("libraries").and_then(Value::as_array) {
        for library in libraries {
            let name = library.as_object().map(py_str_library_name).unwrap_or_default();
            let parts: Vec<&str> = name.split(':').collect();
            if parts.len() < 3 || parts[0] != group || parts[1] != artifact {
                continue;
            }
            let mut version = parts[2].to_owned();
            if loader == "forge" && !minecraft_version.is_empty() {
                if let Some(rest) = version.strip_prefix(&format!("{minecraft_version}-")) {
                    version = rest.to_owned();
                }
            }
            if !version.is_empty() {
                return version;
            }
        }
    }
    let lowered = version_id.to_lowercase();
    if matches!(loader, "fabric" | "quilt") {
        let prefix = format!("{loader}-loader-");
        if lowered.starts_with(&prefix) {
            // `get()` защищает срез от несовпадения байтового смещения в Unicode
            if let Some(rest) = version_id.get(prefix.len()..) {
                let mut version = rest.to_owned();
                let suffix = format!("-{minecraft_version}");
                if !minecraft_version.is_empty() && version.ends_with(&suffix) {
                    version.truncate(version.len() - suffix.len());
                }
                return version;
            }
        }
    }
    let marker = format!("{loader}-");
    if let Some(index) = lowered.rfind(&marker) {
        if let Some(rest) = version_id.get(index + marker.len()..) {
            let mut version = rest.to_owned();
            let game_prefix = format!("{minecraft_version}-");
            if !minecraft_version.is_empty() {
                if let Some(stripped) = version.strip_prefix(&game_prefix) {
                    version = stripped.to_owned();
                }
            }
            return version;
        }
    }
    String::new()
}

fn py_str_library_name(library: &Map<String, Value>) -> String {
    py_str(library.get("name").unwrap_or(&Value::Null))
}

fn live_play_seconds(path: &Path, now: f64) -> i64 {
    let Some(state) = read_bounded_json(&path.join(LIVE_STATE_FILE), MAX_INSTANCE_LIVE_STATE_JSON_BYTES) else {
        return 0;
    };
    if !state.is_object() || state.get("running").and_then(Value::as_bool) != Some(true) {
        return 0;
    }
    let started_at = py_float(state.get("started_at").unwrap_or(&Value::Null));
    if started_at > 0.0 {
        (now - started_at).max(0.0) as i64
    } else {
        0
    }
}

fn valid_image(content: &[u8], suffix: &str) -> bool {
    match suffix {
        ".png" => content.starts_with(b"\x89PNG\r\n\x1a\n"),
        ".jpg" | ".jpeg" => content.starts_with(b"\xff\xd8\xff"),
        ".webp" => content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP",
        _ => false,
    }
}

/// кэширует URL иконок по пути, времени изменения и размеру файла
struct CachedImage {
    modified_ns: u128,
    len: u64,
    url: String,
}

static IMAGE_URL_CACHE: Mutex<Option<HashMap<PathBuf, CachedImage>>> = Mutex::new(None);
/// верхняя граница кеша, чтобы сотни больших иконок не съели память
const IMAGE_URL_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

fn image_data_url(path: &Path, limit: u64) -> String {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return String::new();
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > limit {
        return String::new();
    }
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_nanos())
        .unwrap_or(0);
    let len = meta.len();
    {
        let guard = IMAGE_URL_CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(map) = guard.as_ref() {
            if let Some(hit) = map.get(path) {
                if hit.modified_ns == modified_ns && hit.len == len {
                    return hit.url.clone();
                }
            }
        }
    }
    let Ok(content) = fs::read(path) else {
        return String::new();
    };
    let suffix = lowercase_suffix(path);
    if !valid_image(&content, &suffix) {
        return String::new();
    }
    let mime = match suffix.as_str() {
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".webp" => "image/webp",
        _ => return String::new(),
    };
    let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&content));
    if url.len() <= IMAGE_URL_CACHE_MAX_BYTES / 4 {
        let mut guard = IMAGE_URL_CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        let budget = map
            .values()
            .map(|entry| entry.url.len())
            .sum::<usize>()
            .saturating_add(url.len());
        if budget > IMAGE_URL_CACHE_MAX_BYTES {
            map.clear();
        }
        map.insert(path.to_path_buf(), CachedImage { modified_ns, len, url: url.clone() });
    }
    url
}

fn write_profile_asset(assets: &Path, kind: &str, file_name: &str, encoded: &str, limit: usize) -> Result<String, String> {
    let normalized = file_name.replace('\\', "/");
    let safe_name = Path::new(&normalized)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let suffix = lowercase_suffix(Path::new(&safe_name));
    if safe_name.is_empty() || safe_name != normalized || !matches!(suffix.as_str(), ".png" | ".jpg" | ".jpeg" | ".webp") {
        return Err("Поддерживаются только PNG, JPEG и WebP".to_owned());
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Изображение профиля повреждено".to_owned())?;
    if content.len() > limit || !valid_image(&content, &suffix) {
        return Err("Изображение профиля имеет неверный формат или слишком большое".to_owned());
    }
    fs::create_dir_all(assets).map_err(|error| format!("{error}"))?;
    super::platform::restrict_dir(assets);
    if is_symlink(assets) {
        return Err("Каталог изображений экземпляра является символьной ссылкой".to_owned());
    }
    remove_profile_assets(assets, kind)?;
    let destination = assets.join(format!("{kind}{suffix}"));
    super::platform::create_private(&destination)
        .and_then(|mut file| file.write_all(&content))
        .map_err(|error| format!("{error}"))?;
    Ok(file_name_of(&destination))
}

fn remove_profile_assets(directory: &Path, kind: &str) -> Result<(), String> {
    if fs::symlink_metadata(directory).is_err() {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(());
    };
    let prefix = format!("{kind}.");
    for entry in entries.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        if is_symlink(&path) {
            return Err("Изображение профиля является символьной ссылкой".to_owned());
        }
        if path.is_file() {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

fn lowercase_suffix(path: &Path) -> String {
    path.extension()
        .map(|extension| format!(".{}", extension.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

fn windows_appdata() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    std::env::var_os("APPDATA").filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub(crate) fn external_instances_scan(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let candidates: Vec<Value> = store.list_import_candidates().iter().map(ExternalImportCandidate::public).collect();
    Ok(json!({ "candidates": candidates }))
}

pub(crate) fn instance_groups(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    Ok(json!({ "groups": InstanceStore::new(&config)?.groups() }))
}

pub(crate) fn create_instance_group(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    Ok(json!({ "groups": InstanceStore::new(&config)?.create_group(name)? }))
}

pub(crate) fn delete_instance_group(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    Ok(json!({ "groups": InstanceStore::new(&config)?.delete_group(name)? }))
}

pub(crate) fn rename_instance_group(request: &Value) -> Result<Value, String> {
    let old_name = request.get("old_name").and_then(Value::as_str).unwrap_or_default();
    let new_name = request.get("new_name").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    Ok(json!({ "groups": InstanceStore::new(&config)?.rename_group(old_name, new_name)? }))
}

pub(crate) fn delete_instance(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    store.delete(name)?;
    super::shortcuts::forget_instance(name.trim());
    let instances: Vec<Value> = store.list().iter().map(|info| info.public()).collect();
    Ok(json!({ "instances": instances }))
}

pub(crate) fn duplicate_instance(request: &Value) -> Result<Value, String> {
    let source = request.get("source").and_then(Value::as_str).unwrap_or_default();
    let target = request.get("target").and_then(Value::as_str).unwrap_or_default();
    let config = ConfigStore::load()?;
    let value = InstanceStore::new(&config)?.duplicate(source, target)?;
    Ok(json!({ "instance": value.public() }))
}

/// импортирует найденный экземпляр из Prism или MultiMC и сообщает об изменении рабочей области
pub(crate) fn external_instance_import(request: &Value) -> Result<Value, String> {
    let source_path = request.get("source_path").and_then(Value::as_str).unwrap_or_default();
    let target_name = request.get("target_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let target = if target_name.is_empty() { None } else { Some(target_name.as_str()) };
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let value = store.import_external_instance(source_path, target)?;
    super::events::emit_status_event("workspace-changed", "Экземпляр импортирован.");
    let instances: Vec<Value> = store.list().iter().map(|info| info.public()).collect();
    Ok(json!({ "instance": value.public(), "instances": instances }))
}

/// обновляет профиль экземпляра, включая иконки, баннер, заметки и метки
pub(crate) fn update_instance_profile(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let text_field = |key: &str| request.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let update = ProfileUpdate {
        display_name: text_field("display_name"),
        description: text_field("description"),
        group: text_field("group"),
        icon_file_name: text_field("icon_file_name"),
        icon_data: text_field("icon_data"),
        banner_file_name: text_field("banner_file_name"),
        banner_data: text_field("banner_data"),
        clear_icon: request.get("clear_icon").and_then(Value::as_bool).unwrap_or(false),
        clear_banner: request.get("clear_banner").and_then(Value::as_bool).unwrap_or(false),
        notes: request.get("notes").and_then(Value::as_str).map(str::to_owned),
        tags: request
            .get("tags")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect()),
    };
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let profile = store.update_profile(name, &update)?;
    super::events::emit_status_event("workspace-changed", "Профиль экземпляра обновлён.");
    Ok(json!({ "instance": profile.public(), "groups": store.groups() }))
}

fn discover_external_instance_candidates(home: &Path) -> Vec<ExternalImportCandidate> {
    let mut values: Vec<ExternalImportCandidate> = Vec::new();
    let appdata = windows_appdata();
    let mut prism_roots = vec![
        home.join(".local/share/PrismLauncher/instances"),
        home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"),
    ];
    let mut multimc_roots = vec![
        home.join(".local/share/MultiMC/instances"),
        home.join(".config/MultiMC/instances"),
        home.join(".config/multimc/instances"),
    ];
    let mut curseforge_roots = vec![home.join("Documents/curseforge/minecraft/Instances")];
    let mut official_roots = vec![
        home.join(".minecraft"),
        home.join("snap/minecraft-launcher/common/.minecraft"),
        home.join(".var/app/com.mojang.Minecraft/data/minecraft"),
    ];
    if let Some(appdata) = appdata.as_ref() {
        prism_roots.push(appdata.join("PrismLauncher").join("instances"));
        multimc_roots.push(appdata.join("MultiMC").join("instances"));
        curseforge_roots.push(home.join("curseforge").join("minecraft").join("Instances"));
        official_roots.insert(0, appdata.join(".minecraft"));
    }
    for root in prism_roots {
        values.extend(discover_prism_family_instances(&root, "Prism Launcher"));
    }
    for root in multimc_roots {
        values.extend(discover_prism_family_instances(&root, "MultiMC"));
    }
    for root in curseforge_roots {
        values.extend(discover_curseforge_instances(&root));
    }
    for root in official_roots {
        if let Some(candidate) = discover_official_launcher_instance(&root) {
            values.push(candidate);
        }
    }
    values
}

fn sorted_directories(root: &Path) -> Vec<PathBuf> {
    if is_symlink(root) || !root.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| !is_symlink(path) && path.is_dir())
        .collect();
    paths.sort_by_key(|path| file_name_of(path).to_lowercase());
    paths
}

fn discover_prism_family_instances(root: &Path, launcher: &str) -> Vec<ExternalImportCandidate> {
    sorted_directories(root)
        .into_iter()
        .filter(|path| {
            ["instance.cfg", "mmc-pack.json", "version.json"].iter().any(|name| {
                let marker = path.join(name);
                marker.is_file() && !is_symlink(&marker)
            })
        })
        .map(|path| {
            let mut game_path = path.join("minecraft");
            if is_symlink(&game_path) || !game_path.is_dir() {
                game_path = path.clone();
            }
            prism_family_candidate(launcher, &path, &game_path)
        })
        .collect()
}

fn prism_family_candidate(launcher: &str, source_path: &Path, game_path: &Path) -> ExternalImportCandidate {
    let config = read_key_value_file(&source_path.join("instance.cfg"), 256 * 1024);
    let pack = read_json_file(&source_path.join("mmc-pack.json"), MAX_INSTANCE_PROFILE_JSON_BYTES);
    let version_info = read_json_file(&source_path.join("version.json"), MAX_INSTANCE_PROFILE_JSON_BYTES);
    let directory_name = file_name_of(source_path);
    let display_name = {
        let from_config = config.get("name").cloned().unwrap_or_default();
        let raw = if !from_config.is_empty() {
            from_config
        } else {
            pack.as_ref()
                .and_then(|value| value.get("name"))
                .filter(|value| py_truthy(value))
                .map(py_str)
                .unwrap_or_default()
        };
        let trimmed = take_chars(raw.trim(), 80);
        if trimmed.is_empty() { directory_name.clone() } else { trimmed }
    };
    let mut minecraft_version = String::new();
    let mut loader = "vanilla".to_owned();
    let mut version_id = take_chars(
        &version_info
            .as_ref()
            .and_then(|value| value.get("id"))
            .filter(|value| py_truthy(value))
            .map(py_str)
            .unwrap_or_default()
            .trim(),
        120,
    );
    let mut loader_version = String::new();
    if let Some(components) = pack.as_ref().and_then(|value| value.get("components")).and_then(Value::as_array) {
        for item in components {
            let Some(fields) = item.as_object() else { continue };
            let uid = py_str(fields.get("uid").unwrap_or(&Value::Null)).trim().to_lowercase();
            let version = py_str(fields.get("version").unwrap_or(&Value::Null)).trim().to_owned();
            if uid == "net.minecraft" && !version.is_empty() {
                minecraft_version = take_chars(&version, 80);
            } else if uid.contains("fabric-loader") && !version.is_empty() {
                loader = "fabric".to_owned();
                loader_version = version;
            } else if uid.contains("quilt-loader") && !version.is_empty() {
                loader = "quilt".to_owned();
                loader_version = version;
            } else if uid.contains("neoforge") && !version.is_empty() {
                loader = "neoforge".to_owned();
                loader_version = version;
            } else if (uid.contains("minecraftforge") || uid.ends_with("forge")) && !version.is_empty() && loader == "vanilla" {
                loader = "forge".to_owned();
                loader_version = version;
            }
        }
        if version_id.is_empty() {
            if loader != "vanilla" && !loader_version.is_empty() && !minecraft_version.is_empty() {
                version_id = take_chars(&format!("{loader}-{loader_version}-{minecraft_version}"), 120);
            } else if !minecraft_version.is_empty() {
                version_id = minecraft_version.clone();
            }
        }
    }
    if let Some(info) = version_info.as_ref().and_then(|value| value.as_object()) {
        let mut updated = py_str(info.get("inheritsFrom").filter(|value| py_truthy(value)).unwrap_or(&Value::Null));
        if updated.is_empty() {
            updated = py_str(info.get("minecraft").filter(|value| py_truthy(value)).unwrap_or(&Value::Null));
        }
        if updated.is_empty() {
            updated = minecraft_version.clone();
        }
        minecraft_version = take_chars(&updated, 80);
        if version_id.is_empty() {
            version_id = take_chars(&py_str(info.get("id").filter(|value| py_truthy(value)).unwrap_or(&Value::Null)).trim(), 120);
        }
    }
    ExternalImportCandidate {
        launcher: launcher.to_owned(),
        source_path: source_path.to_path_buf(),
        game_path: game_path.to_path_buf(),
        suggested_name: display_name.clone(),
        version_id,
        minecraft_version,
        loader,
        notes: format!("Импортировано из {launcher}."),
        display_name,
    }
}

fn discover_curseforge_instances(root: &Path) -> Vec<ExternalImportCandidate> {
    sorted_directories(root)
        .into_iter()
        .filter(|path| {
            ["minecraftinstance.json", "manifest.json", "minecraft"].iter().any(|name| {
                let marker = path.join(name);
                marker.exists() && !is_symlink(&marker)
            })
        })
        .map(|path| {
            let mut game_path = path.join("minecraft");
            if is_symlink(&game_path) || !game_path.is_dir() {
                game_path = path.clone();
            }
            let details = read_json_file(&path.join("minecraftinstance.json"), MAX_INSTANCE_PROFILE_JSON_BYTES)
                .unwrap_or_else(|| json!({}));
            let manifest = read_json_file(&path.join("manifest.json"), MAX_INSTANCE_PROFILE_JSON_BYTES)
                .unwrap_or_else(|| json!({}));
            let directory_name = file_name_of(&path);
            let display_name = {
                let raw = py_or(details.get("name"), manifest.get("name"));
                let trimmed = take_chars(py_str(raw).trim(), 80);
                if trimmed.is_empty() { directory_name.clone() } else { trimmed }
            };
            let minecraft = manifest.get("minecraft").filter(|value| value.is_object());
            let mut minecraft_version = take_chars(
                &py_str(py_or(
                    minecraft.and_then(|value| value.get("version")),
                    details.get("gameVersion"),
                )),
                80,
            );
            let mut mod_loader_token = details
                .get("baseModLoader")
                .map(|value| match value {
                    Value::Object(fields) => py_or(fields.get("name"), fields.get("id")),
                    other => other,
                })
                .map(py_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if mod_loader_token.is_empty() {
                if let Some(loaders) = minecraft.and_then(|value| value.get("modLoaders")).and_then(Value::as_array) {
                    for item in loaders {
                        if let Some(identifier) = item.get("id").filter(|value| py_truthy(value)) {
                            mod_loader_token = py_str(identifier);
                            break;
                        }
                    }
                }
            }
            let (loader, version_id, resolved_version) = loader_details_from_token(&mod_loader_token, &minecraft_version);
            minecraft_version = resolved_version;
            ExternalImportCandidate {
                launcher: "CurseForge".to_owned(),
                suggested_name: display_name.clone(),
                version_id: if version_id.is_empty() { minecraft_version.clone() } else { version_id },
                game_path,
                display_name,
                minecraft_version,
                loader,
                notes: "Импортировано из CurseForge.".to_owned(),
                source_path: path,
            }
        })
        .collect()
}

fn discover_official_launcher_instance(root: &Path) -> Option<ExternalImportCandidate> {
    if is_symlink(root) || !root.is_dir() {
        return None;
    }
    if !["versions", "saves", "mods", "resourcepacks"].iter().any(|name| {
        let marker = root.join(name);
        marker.exists() && !is_symlink(&marker)
    }) {
        return None;
    }
    let mut version_id = String::new();
    let mut minecraft_version = String::new();
    let versions = root.join("versions");
    if !is_symlink(&versions) && versions.is_dir() {
        let mut json_files: Vec<(SystemTime, PathBuf)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&versions) {
            for directory in entries.filter_map(|entry| entry.ok()) {
                let version_dir = directory.path();
                if is_symlink(&version_dir) || !version_dir.is_dir() {
                    continue;
                }
                if let Ok(files) = fs::read_dir(&version_dir) {
                    for file in files.filter_map(|entry| entry.ok()) {
                        let path = file.path();
                        if is_symlink(&path) || !path.is_file() || !file_name_of(&path).ends_with(".json") {
                            continue;
                        }
                        let modified = file.metadata().and_then(|meta| meta.modified()).ok();
                        if let Some(modified) = modified {
                            json_files.push((modified, path));
                        }
                    }
                }
            }
        }
        if let Some((_, metadata_path)) = json_files
            .into_iter()
            .reduce(|best, item| if item.0 > best.0 { item } else { best })
        {
            let metadata = read_json_file(&metadata_path, MAX_INSTANCE_PROFILE_JSON_BYTES).unwrap_or_else(|| json!({}));
            let identifier = py_str(metadata.get("id").filter(|value| py_truthy(value)).unwrap_or(&Value::Null));
            let stem = metadata_path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            version_id = take_chars(&if identifier.is_empty() { stem } else { identifier }, 120);
            minecraft_version = take_chars(
                &py_str(
                    metadata
                        .get("inheritsFrom")
                        .filter(|value| py_truthy(value))
                        .or_else(|| metadata.get("minecraft").filter(|value| py_truthy(value)))
                        .unwrap_or(&Value::Null),
                ),
                80,
            );
            if minecraft_version.is_empty() {
                minecraft_version = take_chars(&version_from_id(&version_id), 80);
            }
        }
    }
    let combined = if version_id.is_empty() { minecraft_version.clone() } else { version_id.clone() };
    Some(ExternalImportCandidate {
        launcher: "Minecraft Launcher".to_owned(),
        display_name: "Official Minecraft".to_owned(),
        suggested_name: "official-minecraft".to_owned(),
        version_id: combined.clone(),
        minecraft_version,
        loader: loader_from_version(&combined),
        notes: "Импортировано из официального лаунчера Minecraft.".to_owned(),
        source_path: root.to_path_buf(),
        game_path: root.to_path_buf(),
    })
}

fn loader_details_from_token(token: &str, minecraft_version: &str) -> (String, String, String) {
    let lowered = token.trim().to_lowercase();
    let mut loader = "vanilla".to_owned();
    if !lowered.is_empty() {
        for name in ["neoforge", "fabric", "quilt", "forge"] {
            if lowered.contains(name) {
                loader = name.to_owned();
                break;
            }
        }
    }
    let mut minecraft_version = minecraft_version.to_owned();
    if minecraft_version.is_empty() {
        minecraft_version = take_chars(&version_from_id(token.trim()), 80);
    }
    let mut version_id = take_chars(token.trim(), 120);
    if version_id.is_empty() {
        version_id = if loader != "vanilla" && !minecraft_version.is_empty() {
            format!("{loader}-{minecraft_version}")
        } else {
            minecraft_version.clone()
        };
    } else if loader != "vanilla" && !version_id.to_lowercase().contains(&loader) {
        version_id = take_chars(&format!("{loader}-{version_id}"), 120);
    }
    (loader, version_id, minecraft_version)
}

fn ensure_import_version_metadata(target: &Path, candidate: &ExternalImportCandidate) -> Result<(), String> {
    let versions_dir = target.join("versions");
    if !is_symlink(&versions_dir) && version_json_exists(&versions_dir) {
        return Ok(());
    }
    let directory_name = file_name_of(target);
    let raw = if !candidate.version_id.is_empty() {
        candidate.version_id.clone()
    } else if !candidate.minecraft_version.is_empty() {
        candidate.minecraft_version.clone()
    } else if !candidate.suggested_name.is_empty() {
        candidate.suggested_name.clone()
    } else {
        directory_name.clone()
    };
    static UNSAFE_CHARACTERS: OnceLock<Regex> = OnceLock::new();
    let regex = UNSAFE_CHARACTERS.get_or_init(|| Regex::new(r"[^A-Za-z0-9._+-]+").unwrap());
    let cleaned: String = regex.replace_all(&raw, "-").trim_matches(['.', '-']).to_owned();
    let version_id = take_chars(&cleaned, 120);
    let version_id = if version_id.is_empty() { directory_name } else { version_id };
    let mut payload = Map::new();
    payload.insert("id".into(), json!(version_id));
    if candidate.loader != "vanilla" && !candidate.minecraft_version.is_empty() {
        payload.insert("inheritsFrom".into(), json!(candidate.minecraft_version));
        payload.insert("minecraft".into(), json!(candidate.minecraft_version));
    } else if !candidate.minecraft_version.is_empty() {
        payload.insert("id".into(), json!(candidate.minecraft_version));
    }
    let final_id = payload.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let version_dir = versions_dir.join(&final_id);
    fs::create_dir_all(&version_dir).map_err(|error| format!("{error}"))?;
    let mut text = serde_json::to_string_pretty(&Value::Object(payload)).unwrap_or_default();
    text.push('\n');
    fs::write(version_dir.join(format!("{final_id}.json")), text).map_err(|error| format!("{error}"))
}

fn version_json_exists(versions_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(versions_dir) else {
        return false;
    };
    for directory in entries.filter_map(|entry| entry.ok()) {
        let path = directory.path();
        if is_symlink(&path) || !path.is_dir() {
            continue;
        }
        if let Ok(files) = fs::read_dir(&path) {
            for file in files.filter_map(|entry| entry.ok()) {
                let name = file.file_name().to_string_lossy().to_string();
                if name.ends_with(".json") {
                    return true;
                }
            }
        }
    }
    false
}

fn read_json_file(path: &Path, limit: u64) -> Option<Value> {
    read_bounded_json(path, limit).filter(|value| value.is_object())
}

fn read_key_value_file(path: &Path, limit: u64) -> HashMap<String, String> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return HashMap::new();
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > limit {
        return HashMap::new();
    }
    let Ok(raw) = fs::read(path) else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&raw);
    let mut values: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        if !line.contains('=') || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if !key.is_empty() {
            values.insert(key.to_owned(), value.trim().to_owned());
        }
    }
    values
}

/// рекурсивная проверка "в дереве нет символьных ссылок": копирование и
/// архивирование экземпляра не должны следовать по ссылкам за его пределы
fn assert_no_symlinks(root: &Path) -> Result<(), String> {
    if is_symlink(root) {
        return Err(format!("Каталог экземпляра является символьной ссылкой: {}", root.display()));
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| format!("Не удалось прочитать {}: {error}", directory.display()))?;
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if is_symlink(&path) {
                return Err(format!(
                    "Экземпляр содержит символьную ссылку: {}",
                    path.strip_prefix(root).unwrap_or(&path).display()
                ));
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

/// копирует дерево, предварительно проверенное `assert_no_symlinks`
fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| format!("Не удалось создать {}: {error}", destination.display()))?;
    let entries = fs::read_dir(source).map_err(|error| format!("Не удалось прочитать {}: {error}", source.display()))?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let target = destination.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target)?;
            continue;
        }
        if path.is_file() {
            fs::copy(&path, &target).map_err(|error| {
                format!("Не удалось скопировать {}: {error}", path.display())
            })?;
        }
    }
    Ok(())
}

/// удаляет дерево экземпляра; отсутствие каталога не считается ошибкой
fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{error}")),
    }
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

fn within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix('~') {
        return home_dir().join(rest.trim_start_matches(['/', '\\']));
    }
    path.to_path_buf()
}


fn take_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// преобразует скаляр в строку; null и контейнеры дают пустую строку
fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|value| value != 0.0).unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn py_bool(value: &Value) -> bool {
    py_truthy(value)
}

fn py_float(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(0.0),
        Value::String(text) => text.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn nonnegative_int(value: &Value) -> i64 {
    if !py_truthy(value) {
        return 0;
    }
    match value {
        Value::Bool(flag) => i64::from(*flag),
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .unwrap_or(0)
            .max(0),
        Value::String(text) => text.trim().parse::<i64>().unwrap_or(0).max(0),
        _ => 0,
    }
}

fn py_or<'a>(primary: Option<&'a Value>, fallback: Option<&'a Value>) -> &'a Value {
    match primary.filter(|value| py_truthy(value)) {
        Some(value) => value,
        None => fallback.unwrap_or(&Value::Null),
    }
}

fn normalize_tags(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    let mut tags: Vec<String> = Vec::new();
    for item in items.iter().take(24) {
        let tag: String = py_str(item).trim().trim_start_matches('#').chars().take(32).collect();
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags.truncate(12);
    tags
}

fn valid_version_id(id: &str) -> bool {
    id.chars().count() <= 200
        && id != "."
        && id != ".."
        && !id.is_empty()
        && id.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-'))
}

/// версия игры из id: последний фрагмент, предпочтительно 1.x/2.x
fn version_from_id(version_id: &str) -> String {
    static VERSION_RE: OnceLock<Regex> = OnceLock::new();
    let regex = VERSION_RE.get_or_init(|| Regex::new(r"\d+\.\d+(?:\.\d+)?").unwrap());
    let matches: Vec<&str> = regex.find_iter(version_id).map(|found| found.as_str()).collect();
    if matches.is_empty() {
        return version_id.to_owned();
    }
    let preferred: Vec<&str> = matches
        .iter()
        .rev()
        .copied()
        .filter(|found| found.starts_with("1.") || found.starts_with("2."))
        .collect();
    preferred.first().copied().unwrap_or(*matches.last().unwrap()).to_owned()
}

fn loader_from_version(version_id: &str) -> String {
    let lowered = version_id.to_lowercase();
    for name in ["neoforge", "forge", "quilt", "fabric"] {
        if lowered.contains(name) {
            return name.to_owned();
        }
    }
    "vanilla".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, MutexGuard};

    /// тесты меняют XDG/HOME, поэтому замок исключает гонки
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-instances-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_with_env(tag: &str) -> (PathBuf, InstanceStore) {
        let root = temp_root(tag);
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        // каталог игры должен существовать до валидации в ConfigStore
        fs::create_dir_all(root.join("game")).unwrap();
        let config = ConfigStore::load().unwrap();
        // свой каталог игры, как это делает GUI после выбора папки
        let mut settings = config.settings().clone();
        set_value_on(&mut settings, "game_dir", &json!(root.join("game").to_string_lossy())).unwrap();
        atomic_json_write(&config.paths.settings_file, &settings, true).unwrap();
        let config = ConfigStore::load().unwrap();
        let store = InstanceStore::new(&config).unwrap();
        (root, store)
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn corrupt_live_state_and_oversized_metadata_are_ignored() {
        let _guard = env_guard();
        let (_root, store) = store_with_env("live-state");
        let broken = store.root().join("broken-state");
        fs::create_dir_all(&broken).unwrap();
        write_json(&broken.join(INSTANCE_METADATA_FILE), json!({}));
        fs::write(broken.join(LIVE_STATE_FILE), b"[]").unwrap();
        let instance = store.get("broken-state").unwrap();
        assert!(!instance.running);
        assert_eq!(instance.played_seconds, 0);

        let oversized = store.root().join("oversized-metadata");
        let version_json = oversized.join("versions").join("fabric-test").join("fabric-test.json");
        fs::create_dir_all(version_json.parent().unwrap()).unwrap();
        write_json(&oversized.join(INSTANCE_METADATA_FILE), json!({}));
        let big = fs::File::create(&version_json).unwrap();
        big.set_len(MAX_INSTANCE_VERSION_JSON_BYTES + 1).unwrap();
        let instance = store.get("oversized-metadata").unwrap();
        assert_eq!(instance.version_id, "");
        assert_eq!(instance.minecraft_version, "");
    }

    #[test]
    fn version_detection_reports_loader_and_version() {
        let _guard = env_guard();
        let (_root, store) = store_with_env("versions");
        // get() прогоняет имя через safe_instance_name: "Fabric Test" →
        let instance = store.root().join("Fabric-Test");
        let version_dir = instance.join("versions").join("fabric-loader-0.92.2-1.20.4");
        fs::create_dir_all(&version_dir).unwrap();
        write_json(&instance.join(INSTANCE_METADATA_FILE), json!({ "display_name": "Мой мир" }));
        write_json(
            &version_dir.join("fabric-loader-0.92.2-1.20.4.json"),
            json!({ "id": "fabric-loader-0.92.2-1.20.4", "inheritsFrom": "1.20.4" }),
        );
        let info = store.get("Fabric Test").unwrap();
        assert_eq!(info.name, "Fabric-Test");
        assert_eq!(info.minecraft_version, "1.20.4");
        assert_eq!(info.loader, "fabric");
        assert_eq!(info.loader_version, "0.92.2");
        assert_eq!(info.version_id, "fabric-loader-0.92.2-1.20.4");
        let public = info.public();
        assert_eq!(public["display_name"], json!("Мой мир"));
        assert_eq!(public["path"], json!(instance.to_string_lossy()));
        assert_eq!(public["tags"], json!([]));
    }

    #[test]
    fn group_lifecycle_guards_non_empty_groups() {
        let _guard = env_guard();
        let (_root, store) = store_with_env("groups");
        let profile = store.root().join("survival");
        fs::create_dir_all(&profile).unwrap();
        write_json(
            &profile.join(INSTANCE_METADATA_FILE),
            json!({ "display_name": "Survival", "kind": "client", "group": "" }),
        );
        assert_eq!(store.create_group("  Co-op  ").unwrap(), vec!["Co-op"]);
        let assigned = store
            .update_profile(
                "survival",
                &ProfileUpdate {
                    display_name: "Survival".into(),
                    description: "Friends world".into(),
                    group: "Co-op".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(assigned.group, "Co-op");
        let error = store.delete_group("Co-op").unwrap_err();
        assert!(error.contains("пока в ней есть экземпляры"), "{error}");
        let unassigned = store
            .update_profile(
                "survival",
                &ProfileUpdate {
                    display_name: "Survival".into(),
                    description: "Friends world".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(unassigned.group, "");
        assert_eq!(store.delete_group("Co-op").unwrap(), Vec::<String>::new());
        // группу нельзя создать с пустым названием
        assert!(store.create_group("   ").unwrap_err().contains("не может быть пустым"));
    }

    #[test]
    fn update_profile_rejects_unknown_group_and_bad_images() {
        let _guard = env_guard();
        let (_root, store) = store_with_env("profile");
        let profile = store.root().join("alpha");
        fs::create_dir_all(&profile).unwrap();
        write_json(&profile.join(INSTANCE_METADATA_FILE), json!({}));
        let error = store
            .update_profile("alpha", &ProfileUpdate { group: "Nope".into(), ..Default::default() })
            .unwrap_err();
        assert_eq!(error, "Сначала создайте выбранную группу");
        let damaged = store
            .update_profile(
                "alpha",
                &ProfileUpdate {
                    icon_file_name: "icon.png".into(),
                    icon_data: "%%%".into(),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(damaged, "Изображение профиля повреждено");
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nbody");
        let updated = store
            .update_profile(
                "alpha",
                &ProfileUpdate {
                    display_name: "  Альфа  ".into(),
                    notes: Some("заметка".into()),
                    tags: Some(vec!["#one".into(), " one ".into(), "".into()]),
                    icon_file_name: "icon.png".into(),
                    icon_data: png,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.display_name, "Альфа");
        assert_eq!(updated.tags, vec!["one"]);
        assert!(updated.icon.starts_with("data:image/png;base64,"));
        assert_eq!(updated.notes, "заметка");
    }

    #[test]
    fn duplicate_copies_files_and_keeps_game_data() {
        let _guard = env_guard();
        let (_root, store) = store_with_env("duplicate");
        let alpha = store.root().join("alpha");
        fs::create_dir_all(alpha.join("mods")).unwrap();
        write_json(
            &alpha.join(INSTANCE_METADATA_FILE),
            json!({ "display_name": "Alpha", "played_seconds": 3 }),
        );
        fs::write(alpha.join("mods").join("mod.jar"), b"jar").unwrap();

        let copy = store.duplicate("alpha", "beta").unwrap();
        assert!(copy.path.join("mods").join("mod.jar").is_file());
        assert_eq!(copy.display_name, "Alpha");
        let again = store.duplicate("alpha", "beta").unwrap_err();
        assert!(again.contains("Экземпляр уже существует"), "{again}");

    }

    #[test]
    fn prism_instances_are_discovered_and_imported() {
        let _guard = env_guard();
        let (root, store) = store_with_env("prism");
        let home = root.join("home");
        let prism = home.join(".local/share/PrismLauncher/instances/Friends Pack");
        let minecraft = prism.join("minecraft");
        fs::create_dir_all(minecraft.join("mods")).unwrap();
        fs::write(prism.join("instance.cfg"), "name=Friends Pack\n").unwrap();
        write_json(
            &prism.join("mmc-pack.json"),
            json!({
                "components": [
                    { "uid": "net.minecraft", "version": "1.20.1" },
                    { "uid": "net.fabricmc.fabric-loader", "version": "0.15.11" },
                ]
            }),
        );
        fs::write(minecraft.join("mods").join("example.jar"), b"jar").unwrap();

        let candidates = store.list_import_candidates();
        let candidate = candidates.iter().find(|item| item.launcher == "Prism Launcher").expect("prism найден");
        assert_eq!(candidate.minecraft_version, "1.20.1");
        assert_eq!(candidate.loader, "fabric");

        let imported = store
            .import_external_instance(&candidate.source_path.to_string_lossy(), None)
            .unwrap();
        assert_eq!(imported.display_name, "Friends Pack");
        assert_eq!(imported.loader, "fabric");
        assert!(imported.path.join("mods").join("example.jar").is_file());
        assert!(version_json_exists(&imported.path.join("versions")));
    }

    #[test]
    fn loader_and_version_parsing_helpers() {
        assert_eq!(loader_from_version("Neoforge 21"), "neoforge");
        assert_eq!(version_from_id("1.8.9-forge1.8.9"), "1.8.9");
        assert!(!valid_version_id("../evil"));
    }

    /// измеряет время чтения списка из 500 экземпляров; запуск описан в атрибуте `ignore`
    #[test]
    #[ignore = "бенчмарк: cargo test --lib benchmark_list_500 -- --ignored --nocapture"]
    fn benchmark_list_500_instances() {
        let _guard = env_guard();
        let (root, store) = store_with_env("bench500");
        let icon = crate::rust_engine::shortcuts::encode_png(&crate::rust_engine::shortcuts::Rgba::from_pixels(
            64,
            64,
            (0..64 * 64 * 4).map(|index| (index % 251) as u8).collect(),
        ))
        .unwrap();
        for index in 0..500 {
            let name = format!("instance-{index:03}");
            let instance = store.root().join(&name);
            fs::create_dir_all(instance.join(INSTANCE_ASSETS_DIR)).unwrap();
            fs::create_dir_all(instance.join("versions").join(&name)).unwrap();
            fs::write(
                instance.join(INSTANCE_METADATA_FILE),
                json!({ "display_name": name, "icon_file": "icon.png", "banner_file": "banner.png" }).to_string(),
            )
            .unwrap();
            fs::write(instance.join(INSTANCE_ASSETS_DIR).join("icon.png"), &icon).unwrap();
            fs::write(instance.join(INSTANCE_ASSETS_DIR).join("banner.png"), &icon).unwrap();
            fs::write(
                instance.join("versions").join(&name).join(format!("{name}.json")),
                json!({ "id": name, "minecraftVersion": "1.21.1" }).to_string(),
            )
            .unwrap();
        }
        for run in 0..3 {
            let started = std::time::Instant::now();
            let list = store.list();
            let ms = started.elapsed().as_millis();
            let with_banner = list.iter().filter(|info| !info.banner.is_empty()).count();
            let with_icon = list.iter().filter(|info| !info.icon.is_empty()).count();
            println!("list() прогон {run}: {ms} мс; экземпляров: {}, с иконкой: {}, с баннером: {with_banner}", list.len(), with_icon);
        }
        // детальная страница по-прежнему отдаёт баннер
        let details = store.get("instance-000").unwrap();
        assert!(!details.banner.is_empty(), "get()/instance_details обязан читать баннер");
        let _ = fs::remove_dir_all(&root);
    }
}
