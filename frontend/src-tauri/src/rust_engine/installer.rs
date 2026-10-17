// Установка версии Minecraft в экземпляр: клиент, библиотеки с нативами,
// assets (параллельная загрузка, fingerprint-маркеры против перекачки),
// лог-конфиг. Каталоги проверяются на символические ссылки на каждом шаге —
// версия ставится в staging и атомарно сменяет старую через renameat2.
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::config::ConfigStore;
use super::constants::MAX_INSTANCE_VERSION_JSON_BYTES;
use super::instances::InstanceStore;
use super::loaders::LoaderRepository;
use super::metadata::{maven_path, merge_version_json, library_url, VersionRepository};
use super::net::{client_from_settings, HttpClient};
use super::platform_support::{canonical_path, simplify_path};
use super::runtime::{safe_process_environment, JavaRuntimeManager};
use super::state::safe_instance_name;

const RENAME_EXCHANGE: u32 = 2;

pub(crate) struct InstallResult {
    pub(crate) version_id: String,
    pub(crate) instance_dir: PathBuf,
    pub(crate) version_json: Value,
    pub(crate) client_jar: PathBuf,
    pub(crate) libraries: Vec<PathBuf>,
    pub(crate) natives_dir: PathBuf,
    pub(crate) assets_root: PathBuf,
    pub(crate) asset_index_name: String,
    pub(crate) logging_file: Option<PathBuf>,
}

pub(crate) struct MinecraftInstaller {
    pub(crate) http: Arc<HttpClient>,
    repository: VersionRepository,
    loaders: LoaderRepository,
    runtime: JavaRuntimeManager,
    instances_root: PathBuf,
    download_cache: PathBuf,
    max_workers: usize,
}

impl MinecraftInstaller {
    pub(crate) fn new(config: &ConfigStore, http: Arc<HttpClient>) -> Result<Self, String> {
        let instances_root = config.game_dir()?.join("instances");
        // load_existing сравнивает канонизированные пути — корень тоже
        // канонизируем, когда он уже существует.
        let instances_root = canonical_path(&instances_root).unwrap_or(instances_root);
        Ok(Self {
            repository: VersionRepository::new(config, Some(http.clone()))?,
            loaders: LoaderRepository::new(http.clone()),
            runtime: JavaRuntimeManager::new(config, Some(http.clone()))?,
            instances_root,
            download_cache: config.paths.download_cache.clone(),
            max_workers: config.int_setting("max_downloads").clamp(1, 32) as usize,
            http,
        })
    }

    fn check_cancelled(&self) -> Result<(), String> {
        match &self.http.cancellation {
            Some(token) => token.check(),
            None => Ok(()),
        }
    }

    pub(crate) fn install(
        &mut self,
        minecraft_version: &str,
        loader: &str,
        loader_version: Option<&str>,
        instance_name: Option<&str>,
        assets: bool,
        replace_existing: bool,
    ) -> Result<InstallResult, String> {
        let base = self.repository.resolve(minecraft_version)?;
        if loader == "forge" || loader == "neoforge" {
            let fallback = format!("{loader}-{minecraft_version}");
            let target = self.instance_dir(instance_name.unwrap_or(&fallback));
            if replace_existing {
                return self.install_staged(&target, |installer, staging| {
                    installer.install_external_loader(minecraft_version, loader, loader_version, assets, staging, &base)
                });
            }
            return self.install_external_loader(minecraft_version, loader, loader_version, assets, &target, &base);
        }
        let (resolved, launch_id) = if loader == "fabric" || loader == "quilt" {
            let profile = self.loaders.profile(loader, minecraft_version, loader_version)?;
            let mut merged = merge_version_json(&base, &profile);
            let launch_id = profile.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
            merged["id"] = json!(launch_id.clone());
            merged["inheritsFrom"] = json!(minecraft_version);
            (merged, launch_id)
        } else if loader == "vanilla" {
            (base.clone(), minecraft_version.to_owned())
        } else {
            return Err(format!("Неизвестный loader: {loader}"));
        };
        let instance = self.instance_dir(instance_name.unwrap_or(&launch_id));
        if replace_existing {
            self.install_staged(&instance, |installer, staging| installer.materialize(&launch_id, staging, &resolved, assets))
        } else {
            self.materialize(&launch_id, &instance, &resolved, assets)
        }
    }

    /// Собрать запускаемый результат из уже лежащих в экземпляре файлов,
    /// по ходу долечивая битое (repair_only).
    pub(crate) fn load_existing(&mut self, instance_dir: &Path, version_id: Option<&str>) -> Result<InstallResult, String> {
        let instance_dir = &instance_absolute(instance_dir);
        if instance_dir.as_path() == self.instances_root.as_path() || !within(instance_dir, &self.instances_root) || !instance_dir.is_dir() {
            return Err(format!("Недопустимый путь экземпляра: {}", instance_dir.display()));
        }
        let versions = instance_dir.join("versions");
        if is_symlink(&versions) {
            return Err("Каталог версий является символьной ссылкой".to_owned());
        }
        let metadata_path;
        let version_id = match version_id {
            Some(value) => {
                safe_version_id(value)?;
                metadata_path = versions.join(value).join(format!("{value}.json"));
                value.to_owned()
            }
            None => {
                let mut candidates: Vec<PathBuf> = Vec::new();
                if versions.is_dir() {
                    if let Ok(entries) = fs::read_dir(&versions) {
                        for entry in entries.filter_map(|entry| entry.ok()) {
                            if !entry.path().is_dir() || is_symlink(&entry.path()) {
                                continue;
                            }
                            if let Ok(inner) = fs::read_dir(entry.path()) {
                                for file in inner.filter_map(|file| file.ok()) {
                                    let path = file.path();
                                    if path.extension().map(|extension| extension == "json").unwrap_or(false)
                                        && !is_symlink(&path)
                                        && !contains_symlink(&path.parent().unwrap(), instance_dir)
                                        && within(&path, instance_dir)
                                    {
                                        candidates.push(path);
                                    }
                                }
                            }
                        }
                    }
                }
                if candidates.is_empty() {
                    return Err(format!("В экземпляре нет установленной версии: {}", instance_dir.display()));
                }
                candidates.sort_by_key(|path| modified_seconds(path));
                let newest = candidates.pop().unwrap();
                let resolved_id = newest.file_stem().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
                metadata_path = newest;
                resolved_id
            }
        };
        if is_symlink(&metadata_path)
            || contains_symlink(metadata_path.parent().unwrap(), instance_dir)
            || !within(&metadata_path, instance_dir)
        {
            return Err("Metadata версии является символьной ссылкой".to_owned());
        }
        let meta = fs::symlink_metadata(&metadata_path);
        if !metadata_path.is_file() || meta.as_ref().map(|value| value.len()).unwrap_or(u64::MAX) > MAX_INSTANCE_VERSION_JSON_BYTES {
            return Err("Metadata версии слишком велика или недоступна".to_owned());
        }
        let raw = fs::read_to_string(&metadata_path).map_err(|_| format!("Не удалось прочитать metadata версии: {}", metadata_path.display()))?;
        let mut version_json: Value = serde_json::from_str(&raw).map_err(|_| format!("Не удалось прочитать metadata версии: {}", metadata_path.display()))?;
        if !version_json.is_object() {
            return Err(format!("Некорректный metadata JSON: {}", metadata_path.display()));
        }
        if version_json.get("mainClass").and_then(Value::as_str).unwrap_or("").is_empty() {
            let parent_id = version_json.get("inheritsFrom").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            if !parent_id.is_empty() && parent_id != version_id {
                // Профили forge/neoforge наследуют ванильный JSON: без
                // родителя нет mainClass и половины classpath.
                if let Ok(parent) = self.repository.resolve(&parent_id) {
                    version_json = merge_version_json(&parent, &version_json);
                }
            }
        }
        safe_version_id(&version_id)?;
        let client_jar = self.download_client(instance_dir, &version_id, &version_json, true)?;
        let natives_dir = metadata_path.parent().unwrap().join("natives");
        let libraries = self.download_libraries(instance_dir, &version_json, &natives_dir, true)?;
        let (asset_index_name, assets_root) = self.download_assets(instance_dir, &version_json, true, true)?;
        let logging_file = self.download_logging(instance_dir, &version_json, Some(metadata_path.parent().unwrap()), true);
        Ok(InstallResult {
            version_id,
            instance_dir: instance_dir.to_path_buf(),
            version_json,
            client_jar,
            libraries,
            natives_dir,
            assets_root,
            asset_index_name,
            logging_file: logging_file.ok().flatten(),
        })
    }

    fn materialize(&mut self, version_id: &str, instance: &Path, version_json: &Value, assets: bool) -> Result<InstallResult, String> {
        safe_version_id(version_id)?;
        ensure_safe_directory(instance, &self.instances_root, "экземпляр")?;
        fs::create_dir_all(instance).map_err(|error| format!("Не удалось создать каталог {}: {error}", instance.display()))?;
        set_mode_700(instance);
        let version_dir = instance.join("versions").join(version_id);
        ensure_safe_directory(&version_dir, instance, "каталог версии")?;
        fs::create_dir_all(&version_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", version_dir.display()))?;
        let metadata_path = version_dir.join(format!("{version_id}.json"));
        if is_symlink(&metadata_path) {
            return Err(format!("Metadata версии является символьной ссылкой: {}", metadata_path.display()));
        }
        let mut text = serde_json::to_string_pretty(version_json).map_err(|error| error.to_string())?;
        text.push('\n');
        fs::write(&metadata_path, text).map_err(|error| format!("Не удалось записать {}: {error}", metadata_path.display()))?;

        let client_jar = self.download_client(instance, version_id, version_json, false)?;
        let natives_dir = version_dir.join("natives");
        let libraries = self.download_libraries(instance, version_json, &natives_dir, false)?;
        let (asset_index_name, assets_root) = self.download_assets(instance, version_json, assets, false)?;
        let logging_file = self.download_logging(instance, version_json, Some(&version_dir), false)?;
        Ok(InstallResult {
            version_id: version_id.to_owned(),
            instance_dir: instance.to_path_buf(),
            version_json: version_json.clone(),
            client_jar,
            libraries,
            natives_dir,
            assets_root,
            asset_index_name,
            logging_file,
        })
    }

    fn download_managed_file(
        &self,
        instance: &Path,
        state: &mut Map<String, Value>,
        url: &str,
        destination: &Path,
        info: &Value,
        repair_only: bool,
    ) -> Result<(bool, bool), String> {
        assert_safe_cached_file(destination, instance)?;
        let metadata = integrity_metadata(info)?;
        if let Some(metadata) = &metadata {
            if integrity_cache_matches(instance, destination, state, metadata) {
                return Ok((false, true));
            }
        }
        if repair_only && destination.is_file() && metadata.is_none() {
            return Ok((false, true));
        }
        let downloaded = download_with_metadata(&self.http, url, destination, metadata.as_ref())?;
        if let Some(metadata) = &metadata {
            let relative = relative_posix(destination, instance).ok_or_else(|| format!("Небезопасный путь файла экземпляра: {}", destination.display()))?;
            state.insert(
                relative,
                json!({ "metadata": metadata, "fingerprint": file_fingerprint(destination)? }),
            );
        }
        Ok((downloaded, false))
    }

    fn download_client(&mut self, instance: &Path, version_id: &str, version_json: &Value, repair_only: bool) -> Result<PathBuf, String> {
        safe_version_id(version_id)?;
        let version_dir = instance.join("versions").join(version_id);
        let destination = version_dir.join(format!("{version_id}.jar"));
        let info = version_json.pointer("/downloads/client").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
        if let Some(url) = info.get("url").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            ensure_safe_directory(&version_dir, instance, "каталог версии")?;
            fs::create_dir_all(&version_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", version_dir.display()))?;
            let mut state = read_integrity_state(instance, &version_dir)?;
            let previous = state.clone();
            self.download_managed_file(instance, &mut state, url, &destination, &info, repair_only)?;
            if state != previous {
                write_integrity_state(instance, &version_dir, &state)?;
            }
            return Ok(destination);
        }

        let mut parent_id = version_json.get("jar").and_then(Value::as_str).map(str::to_owned);
        if parent_id.as_deref().map(|value| value == version_id).unwrap_or(true) {
            parent_id = version_json.get("inheritsFrom").and_then(Value::as_str).map(str::to_owned);
        }
        if let Some(parent_id) = parent_id.filter(|value| value != version_id) {
            let parent = self.repository.resolve(&parent_id)?;
            let parent_jar = self.download_client(instance, &parent_id, &parent, repair_only)?;
            let parent_info = parent.pointer("/downloads/client").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
            let parent_metadata = integrity_metadata(&parent_info)?;
            let metadata = integrity_metadata(&info)?.or(parent_metadata.clone());
            assert_safe_cached_file(&destination, instance)?;
            if destination.is_file() && metadata.is_some() {
                let mut state = read_integrity_state(instance, &version_dir)?;
                let metadata = metadata.clone().unwrap();
                if integrity_cache_matches(instance, &destination, &state, &metadata) || verify_with_metadata(&self.http, &destination, Some(&metadata)) {
                    let relative = relative_posix(&destination, instance).unwrap_or_default();
                    state.insert(relative, json!({ "metadata": metadata, "fingerprint": file_fingerprint(&destination)? }));
                    write_integrity_state(instance, &version_dir, &state)?;
                    return Ok(destination);
                }
                let _ = fs::remove_file(&destination);
            } else if destination.is_file() && repair_only && metadata.is_none() {
                return Ok(destination);
            }
            ensure_safe_directory(&version_dir, instance, "каталог версии")?;
            fs::create_dir_all(&version_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", version_dir.display()))?;
            fs::copy(&parent_jar, &destination).map_err(|error| format!("Не удалось скопировать client jar: {error}"))?;
            if let Some(metadata) = metadata {
                if Some(&metadata) != parent_metadata.as_ref() && !verify_with_metadata(&self.http, &destination, Some(&metadata)) {
                    let _ = fs::remove_file(&destination);
                    return Err(format!("Inherited client jar не соответствует metadata версии {version_id}"));
                }
                let mut state = read_integrity_state(instance, &version_dir)?;
                let relative = relative_posix(&destination, instance).unwrap_or_default();
                state.insert(relative, json!({ "metadata": metadata, "fingerprint": file_fingerprint(&destination)? }));
                write_integrity_state(instance, &version_dir, &state)?;
            }
            return Ok(destination);
        }

        let metadata = integrity_metadata(&info)?;
        assert_safe_cached_file(&destination, instance)?;
        if destination.is_file() {
            if let Some(metadata) = metadata {
                ensure_safe_directory(&version_dir, instance, "каталог версии")?;
                fs::create_dir_all(&version_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", version_dir.display()))?;
                let mut state = read_integrity_state(instance, &version_dir)?;
                if integrity_cache_matches(instance, &destination, &state, &metadata) || verify_with_metadata(&self.http, &destination, Some(&metadata)) {
                    let relative = relative_posix(&destination, instance).unwrap_or_default();
                    state.insert(relative, json!({ "metadata": metadata, "fingerprint": file_fingerprint(&destination)? }));
                    write_integrity_state(instance, &version_dir, &state)?;
                    return Ok(destination);
                }
                return Err(format!("Client jar повреждён, но для версии {version_id} нет URL загрузки"));
            }
            return Ok(destination);
        }
        Err(format!("У версии {version_id} нет client download"))
    }

    fn download_libraries(&mut self, instance: &Path, version_json: &Value, natives_dir: &Path, repair_only: bool) -> Result<Vec<PathBuf>, String> {
        ensure_safe_directory(natives_dir, instance, "каталог native")?;
        let had_natives = natives_dir.is_dir();
        fs::create_dir_all(natives_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", natives_dir.display()))?;
        let version_dir = natives_dir.parent().unwrap().to_path_buf();
        let mut state = read_integrity_state(instance, &version_dir)?;
        let previous = state.clone();
        let mut libraries: Vec<PathBuf> = Vec::new();
        let mut needs_native_extraction = !repair_only || !had_natives;
        let empty_libraries = Vec::new();
        let entries = version_json.get("libraries").and_then(Value::as_array).unwrap_or(&empty_libraries);
        for library in entries {
            if !library.is_object() || !rules_allow(library.get("rules")) {
                continue;
            }
            let name = library.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let downloads = library.get("downloads").filter(|value| value.is_object());
            let artifact = downloads.and_then(|value| value.get("artifact")).filter(|value| value.is_object());
            let coordinates: Vec<&str> = name.split(':').collect();
            let native_only = coordinates.len() >= 4 && coordinates[3].starts_with("natives-");
            if native_only && artifact.is_some() {
                let artifact = artifact.unwrap();
                let path = artifact.get("path").and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_owned);
                let path = match path {
                    Some(path) => path,
                    None => maven_path(name, None)?,
                };
                let native_destination = instance.join("libraries").join(safe_relative_path(&path, "native library")?);
                let url = library_url(library, None)?;
                let (downloaded, cached) = self.download_managed_file(instance, &mut state, &url, &native_destination, artifact, repair_only)?;
                if downloaded || !cached {
                    needs_native_extraction = true;
                }
                if needs_native_extraction {
                    let excluded = extract_excludes(library);
                    safe_extract(&native_destination, natives_dir, &excluded)?;
                }
                continue;
            }
            if artifact.is_some() || downloads.is_none() {
                let artifact_value = artifact.cloned().unwrap_or_else(|| json!({}));
                let path = artifact_value.get("path").and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_owned);
                let path = match path {
                    Some(path) => path,
                    None => maven_path(name, None)?,
                };
                let artifact_destination = instance.join("libraries").join(safe_relative_path(&path, "library")?);
                let url = library_url(library, None)?;
                self.download_managed_file(instance, &mut state, &url, &artifact_destination, &artifact_value, repair_only)?;
                if !native_only {
                    libraries.push(artifact_destination);
                }
            }
            let natives = library.get("natives").filter(|value| value.is_object());
            let platform_key = platform_key();
            let native_classifier = natives.and_then(|value| value.get(platform_key)).and_then(Value::as_str).map(str::to_owned);
            if let Some(native_classifier) = native_classifier {
                let classifier = native_classifier.replace("${arch}", "64");
                let classifier_info = downloads
                    .and_then(|value| value.get("classifiers"))
                    .and_then(|value| value.get(&classifier))
                    .filter(|value| value.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let path = classifier_info.get("path").and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_owned);
                let path = match path {
                    Some(path) => path,
                    None => maven_path(name, Some(&classifier))?,
                };
                let native_destination = instance.join("libraries").join(safe_relative_path(&path, "native classifier")?);
                let url = library_url(library, Some(&classifier))?;
                let (downloaded, cached) = self.download_managed_file(instance, &mut state, &url, &native_destination, &classifier_info, repair_only)?;
                if downloaded || !cached {
                    needs_native_extraction = true;
                }
                if needs_native_extraction {
                    let excluded = extract_excludes(library);
                    safe_extract(&native_destination, natives_dir, &excluded)?;
                }
            }
        }
        if state != previous {
            write_integrity_state(instance, &version_dir, &state)?;
        }
        Ok(libraries)
    }

    fn download_assets(&mut self, instance: &Path, version_json: &Value, download_objects: bool, repair_only: bool) -> Result<(String, PathBuf), String> {
        let assets_root = instance.join("assets");
        ensure_safe_directory(&assets_root, instance, "каталог assets")?;
        fs::create_dir_all(&assets_root).map_err(|error| format!("Не удалось создать каталог {}: {error}", assets_root.display()))?;
        let asset_index = version_json.get("assetIndex").filter(|value| value.is_object());
        let Some(asset_index) = asset_index else {
            let name = version_json.get("assets").and_then(Value::as_str).unwrap_or("").to_owned();
            return Ok((name, assets_root));
        };
        let fallback_name = version_json.get("assets").and_then(Value::as_str).unwrap_or("assets");
        let index_id = asset_index.get("id").and_then(Value::as_str).filter(|value| !value.is_empty()).unwrap_or(fallback_name);
        let index_name = safe_component(index_id, "asset index")?;
        let index_path = assets_root.join("indexes").join(format!("{index_name}.json"));
        let marker = assets_root.join(".trial-assets-complete.json");
        let marker_data = read_asset_marker(&marker);
        let index_metadata = integrity_metadata(asset_index)?;
        let index_unchanged = marker_data.get("asset_index_metadata") == index_metadata.as_ref()
            && asset_fingerprint_matches(instance, &index_path, asset_index.get("size"), marker_data.get("asset_index_fingerprint"));
        assert_safe_cached_file(&index_path, instance)?;
        let index_url = asset_index.get("url").and_then(Value::as_str).unwrap_or("");
        if !index_unchanged && !(repair_only && index_path.is_file() && index_metadata.is_none()) {
            download_with_metadata(&self.http, index_url, &index_path, index_metadata.as_ref())?;
        }
        let index_fingerprint = if index_path.is_file() { Some(file_fingerprint(&index_path)?) } else { None };
        if !download_objects {
            return Ok((index_name, assets_root));
        }
        let raw = fs::read_to_string(&index_path).map_err(|_| format!("Некорректный asset index {index_name}"))?;
        let index: Value = serde_json::from_str(&raw).map_err(|_| format!("Некорректный asset index {index_name}"))?;
        if !index.is_object() {
            return Err(format!("Некорректный asset index {index_name}"));
        }
        let Some(objects) = index.get("objects").and_then(Value::as_object) else {
            return Ok((index_name, assets_root));
        };
        let mut assets_by_digest: Map<String, Value> = Map::new();
        for item in objects.values() {
            let Some(digest) = item.get("hash").and_then(Value::as_str) else { continue };
            if !is_sha1_hex(digest) {
                return Err(format!("Некорректный hash asset: {digest}"));
            }
            assets_by_digest.insert(digest.to_owned(), item.clone());
        }
        let fingerprints = asset_marker_fingerprints(&marker, &assets_by_digest);
        if assets_marker_valid(&marker, asset_index, &assets_root, &assets_by_digest) {
            let materialized = materialize_legacy_assets(instance, &assets_root, &index_name, &index, marker_data.get("materialized"))?;
            if Value::Object(materialized.clone()) != marker_data.get("materialized").cloned().unwrap_or(Value::Null)
                || marker_data.get("asset_index_metadata") != index_metadata.as_ref()
                || marker_data.get("asset_index_fingerprint") != index_fingerprint.as_ref()
            {
                write_asset_marker(&marker, asset_index, &assets_by_digest, &fingerprints, &materialized, index_metadata.as_ref(), index_fingerprint.as_ref())?;
            }
            return Ok((index_name, assets_root));
        }
        struct AssetJob {
            url: String,
            path: PathBuf,
            digest: String,
            size: Option<u64>,
        }
        let mut jobs: Vec<AssetJob> = Vec::new();
        for (digest, item) in &assets_by_digest {
            let path = assets_root.join("objects").join(&digest[..2]).join(digest);
            if asset_fingerprint_matches(instance, &path, item.get("size"), fingerprints.get(digest)) {
                continue;
            }
            jobs.push(AssetJob {
                url: format!("https://resources.download.minecraft.net/{}/{}", &digest[..2], digest),
                path,
                digest: digest.clone(),
                size: item.get("size").and_then(Value::as_i64).map(|value| value as u64),
            });
        }
        if !jobs.is_empty() {
            // Пул как в Python: очередь работ, воркеры до max_downloads.
            let workers = self.max_workers.min(32).max(1);
            let queue = Arc::new(std::sync::Mutex::new(jobs.into_iter()));
            // После первой ошибки остальные потоки не берут новые файлы:
            // иначе установка «висела», докачивая тысячи assets впустую.
            let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for _ in 0..workers {
                    let queue = queue.clone();
                    let failed = failed.clone();
                    let http = self.http.clone();
                    handles.push(scope.spawn(move || -> Result<(), String> {
                        loop {
                            if failed.load(std::sync::atomic::Ordering::Relaxed) {
                                return Ok(());
                            }
                            let job = { queue.lock().unwrap_or_else(|poison| poison.into_inner()).next() };
                            let Some(job) = job else { return Ok(()) };
                            if let Err(error) = http.download(&job.url, &job.path, Some(&job.digest), None, None, job.size, None, None, None) {
                                failed.store(true, std::sync::atomic::Ordering::Relaxed);
                                return Err(error);
                            }
                        }
                    }));
                }
                drop(queue);
                let mut first_error: Option<String> = None;
                for handle in handles {
                    let outcome = handle.join().map_err(|_| "Поток загрузки assets упал".to_owned()).and_then(|result| result);
                    if let Err(error) = outcome {
                        first_error.get_or_insert(error);
                    }
                }
                match first_error {
                    Some(error) => Err(error),
                    None => Ok::<(), String>(()),
                }
            })?;
        }
        let materialized = materialize_legacy_assets(instance, &assets_root, &index_name, &index, marker_data.get("materialized"))?;
        let mut fingerprints: Map<String, Value> = Map::new();
        for digest in assets_by_digest.keys() {
            let path = assets_root.join("objects").join(&digest[..2]).join(digest);
            fingerprints.insert(digest.clone(), file_fingerprint(&path)?);
        }
        write_asset_marker(&marker, asset_index, &assets_by_digest, &fingerprints, &materialized, index_metadata.as_ref(), index_fingerprint.as_ref())?;
        Ok((index_name, assets_root))
    }

    fn download_logging(&mut self, instance: &Path, version_json: &Value, version_dir: Option<&Path>, repair_only: bool) -> Result<Option<PathBuf>, String> {
        let file_info = version_json
            .pointer("/logging/client/file")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let has_url = file_info.get("url").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false);
        let has_id = file_info.get("id").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false);
        if !has_url && !has_id {
            return Ok(None);
        }
        let file_id = safe_component(file_info.get("id").and_then(Value::as_str).unwrap_or("client.xml"), "logging config")?;
        let destination = instance.join("assets").join("log_configs").join(file_id);
        if !has_url {
            assert_safe_cached_file(&destination, instance)?;
            return Ok(if destination.is_file() { Some(destination) } else { None });
        }
        let version_dir = version_dir.ok_or_else(|| "Каталог версии недоступен".to_owned())?;
        ensure_safe_directory(version_dir, instance, "каталог версии")?;
        fs::create_dir_all(version_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", version_dir.display()))?;
        let mut state = read_integrity_state(instance, version_dir)?;
        let previous = state.clone();
        let url = file_info.get("url").and_then(Value::as_str).unwrap_or("");
        self.download_managed_file(instance, &mut state, url, &destination, &file_info, repair_only)?;
        if state != previous {
            write_integrity_state(instance, version_dir, &state)?;
        }
        Ok(Some(destination))
    }

    fn install_staged(&mut self, target: &Path, build: impl FnOnce(&mut Self, &Path) -> Result<InstallResult, String>) -> Result<InstallResult, String> {
        if is_symlink(&self.instances_root) {
            return Err("Каталог экземпляров является символьной ссылкой".to_owned());
        }
        fs::create_dir_all(&self.instances_root).map_err(|error| format!("Не удалось создать каталог {}: {error}", self.instances_root.display()))?;
        set_mode_700(&self.instances_root);
        if is_symlink(&self.instances_root) || !self.instances_root.is_dir() {
            return Err("Каталог экземпляров недоступен".to_owned());
        }
        let target = self.instances_root.join(target.file_name().unwrap_or_default());
        ensure_safe_directory(&target, &self.instances_root, "экземпляр")?;
        if target.symlink_metadata().is_ok() && (is_symlink(&target) || !target.is_dir()) {
            return Err(format!("Путь экземпляра не является каталогом: {}", target.display()));
        }
        let staging = self.instances_root.join(format!(".trial-stage-{}", random_hex(8)));
        fs::create_dir(&staging).map_err(|error| format!("Не удалось создать staging-каталог: {error}"))?;
        set_mode_700(&staging);
        let staged = match build(self, &staging) {
            Ok(staged) => staged,
            error => {
                let _ = remove_instance_path(&staging);
                return error;
            }
        };
        let result = rebase_install_result(staged, &staging, &target);
        let old = swap_staged_directory(&staging, &target)?;
        if let Some(old) = &old {
            if old != &staging {
                let _ = remove_instance_path(old);
            }
        }
        let _ = remove_instance_path(&staging);
        Ok(result)
    }

    fn install_external_loader(
        &mut self,
        minecraft_version: &str,
        loader: &str,
        loader_version: Option<&str>,
        assets: bool,
        instance_dir: &Path,
        base_version_json: &Value,
    ) -> Result<InstallResult, String> {
        safe_version_id(minecraft_version)?;
        let base = self.materialize(minecraft_version, instance_dir, base_version_json, false)?;
        if let Some((local_json_path, local_json)) = find_external_version(&base.instance_dir, loader, loader_version, minecraft_version) {
            let parent_id = local_json.get("inheritsFrom").and_then(Value::as_str).map(str::to_owned);
            let resolved = match parent_id {
                Some(parent) => merge_version_json(&self.repository.resolve(&parent)?, &local_json),
                None => local_json,
            };
            let version_id = local_json_path.file_stem().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
            return self.materialize(&version_id, &base.instance_dir, &resolved, assets);
        }
        let loader_version = match loader_version {
            Some(value) => value.to_owned(),
            None => {
                let choices = self.loaders.choices(loader, minecraft_version)?;
                let Some(last) = choices.last() else {
                    return Err(format!("Не найден {loader} для Minecraft {minecraft_version}"));
                };
                last.get("version").and_then(Value::as_str).unwrap_or("").to_owned()
            }
        };
        safe_component(&loader_version, "loader version")?;
        let installer_url = self.loaders.installer_url(loader, minecraft_version, &loader_version)?;
        validate_loader_installer_url(&installer_url)?;
        let (digest_algorithm, installer_digest) = self.loader_installer_digest(&installer_url)?;
        let installer_path = self
            .download_cache
            .join("installers")
            .join(format!("{}-{}.jar", safe_component(loader, "loader")?, safe_component(&loader_version, "loader version")?));
        let validator = |url: &str| validate_loader_installer_url(url);
        self.http.download(
            &installer_url,
            &installer_path,
            matches!(digest_algorithm.as_str(), "sha1").then_some(installer_digest.as_str()),
            matches!(digest_algorithm.as_str(), "sha256").then_some(installer_digest.as_str()),
            matches!(digest_algorithm.as_str(), "sha512").then_some(installer_digest.as_str()),
            None,
            None,
            None,
            Some(&validator),
        )?;
        let java = self.runtime.java_for(&base.version_json, None)?;
        let local_installer = base.instance_dir.join(format!(".trial-loader-installer-{loader}-{loader_version}.jar"));
        if is_symlink(&local_installer) {
            return Err(format!("Локальный installer является символьной ссылкой: {}", local_installer.display()));
        }
        fs::copy(&installer_path, &local_installer).map_err(|error| format!("Не удалось подготовить installer: {error}"))?;
        set_mode_600(&local_installer);
        let output = run_loader_installer(loader, &java, &local_installer, &base.instance_dir, &self.http);
        let _ = fs::remove_file(&local_installer);
        let output = output?;
        if !output.success {
            return Err(format!(
                "{loader} installer завершился с кодом {}:\n{}",
                output.code.unwrap_or(-1),
                tail_text(&output.stdout, &output.stderr, 3000)
            ));
        }
        let versions_dir = base.instance_dir.join("versions");
        let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&versions_dir) {
            for entry in entries.filter_map(|entry| entry.ok()) {
                let dir = entry.path();
                if is_symlink(&dir) || !dir.is_dir() {
                    continue;
                }
                if let Ok(files) = fs::read_dir(&dir) {
                    for file in files.filter_map(|file| file.ok()) {
                        let path = file.path();
                        if path.extension().map(|extension| extension == "json").unwrap_or(false) == false {
                            continue;
                        }
                        if is_symlink(&path) || is_symlink(&dir) {
                            continue;
                        }
                        let resolved = match canonical_path(&path) {
                            Ok(resolved) => resolved,
                            Err(_) => continue,
                        };
                        let instance_root = canonical_path(&base.instance_dir).unwrap_or_else(|_| base.instance_dir.clone());
                        if !within(&resolved, &instance_root) {
                            continue;
                        }
                        let dir_name = dir.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default();
                        let file_name = path.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default();
                        if !dir_name.contains(loader) && !file_name.contains(loader) {
                            continue;
                        }
                        if let Ok(meta) = fs::metadata(&path) {
                            candidates.push((path.clone(), meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
                        }
                    }
                }
            }
        }
        if candidates.is_empty() {
            return Err(format!("{loader} installer не создал version JSON в {}", versions_dir.display()));
        }
        candidates.sort_by_key(|(_, modified)| *modified);
        let local_json_path = candidates.pop().map(|(path, _)| path).unwrap();
        if is_symlink(&local_json_path) {
            return Err(format!("Результат {loader} installer является символьной ссылкой"));
        }
        let size = fs::symlink_metadata(&local_json_path).map(|meta| meta.len()).unwrap_or(u64::MAX);
        if size > MAX_INSTANCE_VERSION_JSON_BYTES {
            return Err(format!("Результат {loader} installer слишком велик"));
        }
        let raw = fs::read_to_string(&local_json_path).map_err(|_| format!("Не удалось прочитать результат {loader} installer"))?;
        let local_json: Value = serde_json::from_str(&raw).map_err(|_| format!("Не удалось прочитать результат {loader} installer"))?;
        if !local_json.is_object() {
            return Err(format!("Результат {loader} installer имеет неожиданный формат"));
        }
        let parent_id = local_json.get("inheritsFrom").and_then(Value::as_str).map(str::to_owned);
        let resolved = match parent_id {
            Some(parent) => merge_version_json(&self.repository.resolve(&parent)?, &local_json),
            None => local_json,
        };
        let version_id = local_json_path.file_stem().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
        self.materialize(&version_id, &base.instance_dir, &resolved, assets)
    }

    fn instance_dir(&self, name: &str) -> PathBuf {
        self.instances_root.join(safe_instance_name(name))
    }

    fn loader_installer_digest(&self, installer_url: &str) -> Result<(String, String), String> {
        let validator = |url: &str| validate_loader_installer_url(url);
        for (algorithm, digest_length) in [("sha512", 128), ("sha256", 64), ("sha1", 40)] {
            let Ok(raw) = self.http.request(&format!("{installer_url}.{algorithm}"), "GET", None, None, None, Some(&validator)) else { continue };
            let checksum = String::from_utf8_lossy(&raw).to_string();
            let pattern = regex::Regex::new(&format!(r"\b([A-Fa-f0-9]{{{digest_length}}})\b")).unwrap();
            if let Some(found) = pattern.captures(&checksum) {
                return Ok((algorithm.to_owned(), found.get(1).unwrap().as_str().to_owned()));
            }
        }
        Err("Loader installer не имеет корректного SHA-512, SHA-256 или SHA-1 checksum".to_owned())
    }
}

struct ProcessOutput {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn tail_text(stdout: &str, stderr: &str, limit: usize) -> String {
    let combined = format!("{stdout}\n{stderr}").trim().to_owned();
    let length = combined.chars().count();
    if length <= limit {
        combined
    } else {
        combined.chars().skip(length - limit).collect()
    }
}

/// Аргументы Forge/NeoForge installer: каталог — значение `--installClient`.
/// Ключа `--target` у installer нет (jopt-simple падал на нём с
/// UnrecognizedOptionException ещё до установки).
fn loader_installer_args(installer: &Path, target: &Path) -> Vec<String> {
    vec![
        "-jar".to_owned(),
        simplify_path(installer.to_path_buf()).to_string_lossy().into_owned(),
        "--installClient".to_owned(),
        simplify_path(target.to_path_buf()).to_string_lossy().into_owned(),
    ]
}

/// Installer отказывается ставить клиент без launcher_profiles.json
/// («There is no minecraft launcher profile…») — создаём пустой профиль.
fn ensure_launcher_profiles(target: &Path) -> Result<(), String> {
    let path = target.join("launcher_profiles.json");
    if is_symlink(&path) {
        return Err("launcher_profiles.json экземпляра является символьной ссылкой".to_owned());
    }
    if path.is_file() {
        return Ok(());
    }
    fs::write(&path, "{\"profiles\":{},\"selectedProfile\":\"\",\"version\":3}\n")
        .map_err(|error| format!("Не удалось подготовить launcher_profiles.json: {error}"))
}

/// Последние `limit` байт потока: вывод installer бывает многомегабайтным,
/// а для ошибки нужен только хвост.
fn read_stream_tail(mut stream: impl Read, limit: usize) -> String {
    let mut tail: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                tail.extend_from_slice(&buffer[..count]);
                if tail.len() > limit * 2 {
                    let excess = tail.len() - limit;
                    tail.drain(..excess);
                }
            }
        }
    }
    if tail.len() > limit {
        let excess = tail.len() - limit;
        tail.drain(..excess);
    }
    String::from_utf8_lossy(&tail).into_owned()
}

/// forge/neoforge installer — внешний java-процесс. bwrap-песочница из
/// security.py пока не портирована, поэтому процесс идёт в окружении
/// compat: отдельная сессия, чистое окружение, каталог экземпляра.
///
/// stdout/stderr читаются в отдельных потоках с самого старта: раньше они
/// читались только после выхода процесса, и installer, напечатавший больше
/// буфера канала (на Windows — 4 КиБ), навсегда блокировался на записи —
/// установка Forge/NeoForge «висела» до таймаута в 10 минут.
fn run_loader_installer(loader: &str, java: &Path, installer: &Path, working_dir: &Path, http: &HttpClient) -> Result<ProcessOutput, String> {
    use std::process::{Command, Stdio};
    ensure_launcher_profiles(working_dir)?;
    let mut command = Command::new(simplify_path(java.to_path_buf()));
    command
        .args(loader_installer_args(installer, working_dir))
        .current_dir(working_dir)
        .env_clear()
        .envs(safe_process_environment(false))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| format!("Не удалось запустить installer: {error}"))?;
    const OUTPUT_LIMIT: usize = 256 * 1024;
    let stdout_reader = child.stdout.take().map(|stream| std::thread::spawn(move || read_stream_tail(stream, OUTPUT_LIMIT)));
    let stderr_reader = child.stderr.take().map(|stream| std::thread::spawn(move || read_stream_tail(stream, OUTPUT_LIMIT)));
    let collect = |reader: Option<std::thread::JoinHandle<String>>| reader.and_then(|handle| handle.join().ok()).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(10 * 60);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = collect(stdout_reader);
                let stderr = collect(stderr_reader);
                return Ok(ProcessOutput { success: status.success(), code: status.code(), stdout, stderr });
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Не удалось запустить installer: {error}"));
            }
        }
        let failure = if let Some(Err(error)) = http.cancellation.as_ref().map(|token| token.check()) {
            Some(error)
        } else if Instant::now() >= deadline {
            Some(format!("{loader} installer превысил лимит времени"))
        } else {
            None
        };
        if let Some(error) = failure {
            let _ = child.kill();
            let _ = child.wait();
            let _ = collect(stdout_reader);
            let _ = collect(stderr_reader);
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ------------------------- вспомогательные -------------------------

fn instance_absolute(path: &Path) -> PathBuf {
    let expanded = shellexpand_home(path);
    canonical_path(&expanded).unwrap_or(expanded)
}

fn shellexpand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    path.to_path_buf()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

fn within(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok()
}

fn relative_posix(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root).ok().map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn modified_seconds(path: &Path) -> std::time::SystemTime {
    fs::metadata(path).and_then(|meta| meta.modified()).unwrap_or(std::time::UNIX_EPOCH)
}

fn set_mode_700(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn set_mode_600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

fn platform_key() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(not(windows))]
    {
        match std::env::consts::OS {
            "linux" => "linux",
            "macos" => "darwin",
            other => other,
        }
    }
}

/// bool() из Python для полей запроса: пустая строка и ноль — ложь.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// Поднимаемся от path к root: любая символическая ссылка по пути —
/// небезопасно; выход за root — тоже.
fn contains_symlink(path: &Path, root: &Path) -> bool {
    let mut current = path;
    loop {
        if is_symlink(current) {
            return true;
        }
        if current == root {
            return false;
        }
        let Some(parent) = current.parent() else { return true };
        if parent == current || !within(current, root) {
            return true;
        }
        current = parent;
    }
}

fn ensure_safe_directory(path: &Path, root: &Path, label: &str) -> Result<(), String> {
    if is_symlink(root) {
        return Err(format!("Корень {label} является символьной ссылкой"));
    }
    let mut current = path;
    loop {
        if is_symlink(current) {
            return Err(format!("Небезопасный путь {label}: {}", path.display()));
        }
        if current == root {
            return Ok(());
        }
        let Some(parent) = current.parent() else {
            return Err(format!("Небезопасный путь {label}: {}", path.display()));
        };
        if parent == current || !within(current, root) {
            return Err(format!("Небезопасный путь {label}: {}", path.display()));
        }
        current = parent;
    }
}

fn safe_version_id(value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.chars().count() <= 200
        && value != "."
        && value != ".."
        && value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-'));
    if valid {
        Ok(())
    } else {
        Err(format!("Небезопасный идентификатор версии: {value}"))
    }
}

fn safe_component(value: &str, label: &str) -> Result<String, String> {
    let valid = !value.is_empty() && value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-'));
    if valid {
        Ok(value.to_owned())
    } else {
        Err(format!("Небезопасный {label}: {value}"))
    }
}

fn safe_relative_path(value: &str, label: &str) -> Result<PathBuf, String> {
    let normalized = value.replace('\\', "/");
    let relative = Path::new(&normalized);
    if relative.is_absolute() || normalized.is_empty() || normalized.split('/').any(|part| part.is_empty() || part == ".." || part == ".") {
        // Python пропускает только «..» и абсолютные; пустые части между
        // слешами Path нормализует, мы отвергаем сразу — строже и предсказуемее.
        if relative.is_absolute() || normalized.split('/').any(|part| part == "..") {
            return Err(format!("Небезопасный путь {label}: {value}"));
        }
    }
    let parts: Vec<&str> = normalized.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    if parts.is_empty() {
        return Err(format!("Небезопасный путь {label}: {value}"));
    }
    Ok(parts.iter().collect::<PathBuf>())
}

/// Метаданные целостности из записи манифеста: подмножество
/// {sha1,sha256,sha512,size} с нижним регистром хешей.
fn integrity_metadata(info: &Value) -> Result<Option<Value>, String> {
    let mut metadata = Map::new();
    for algorithm in ["sha1", "sha256", "sha512"] {
        if let Some(digest) = info.get(algorithm).and_then(Value::as_str).filter(|value| !value.is_empty()) {
            metadata.insert(algorithm.into(), json!(digest.to_lowercase()));
        }
    }
    if let Some(size) = info.get("size").filter(|value| !value.is_null()) {
        let size = size
            .as_i64()
            .or_else(|| size.as_str().and_then(|text| text.trim().parse::<i64>().ok()))
            .ok_or_else(|| "Размер файла в metadata некорректен".to_owned())?;
        if size < 0 {
            return Err("Размер файла в metadata некорректен".to_owned());
        }
        metadata.insert("size".into(), json!(size));
    }
    Ok(if metadata.is_empty() { None } else { Some(Value::Object(metadata)) })
}

fn download_with_metadata(http: &HttpClient, url: &str, destination: &Path, metadata: Option<&Value>) -> Result<bool, String> {
    let digest = |key: &str| metadata.and_then(|value| value.get(key)).and_then(Value::as_str).map(str::to_owned);
    let size = metadata.and_then(|value| value.get("size")).and_then(Value::as_i64).map(|value| value as u64);
    http.download(
        url,
        destination,
        digest("sha1").as_deref(),
        digest("sha256").as_deref(),
        digest("sha512").as_deref(),
        size,
        None,
        None,
        None,
    )
}

fn verify_with_metadata(_http: &HttpClient, path: &Path, metadata: Option<&Value>) -> bool {
    let Some(metadata) = metadata else { return false };
    let digest = |key: &str| metadata.get(key).and_then(Value::as_str).map(str::to_owned);
    let size = metadata.get("size").and_then(Value::as_i64).map(|value| value as u64);
    super::net::verify_file(path, digest("sha1").as_deref(), digest("sha256").as_deref(), digest("sha512").as_deref(), size)
}

#[cfg(unix)]
fn file_fingerprint(path: &Path) -> Result<Value, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))?;
    Ok(json!({
        "device": meta.dev() as i64,
        "inode": meta.ino() as i64,
        "size": meta.size() as i64,
        "mtime_ns": meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
        "ctime_ns": meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
    }))
}

#[cfg(not(unix))]
fn file_fingerprint(path: &Path) -> Result<Value, String> {
    let meta = fs::metadata(path).map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))?;
    let stamp = meta.modified().ok().and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok()).map(|value| value.as_nanos() as i64).unwrap_or(0);
    Ok(json!({ "device": 0, "inode": 0, "size": meta.len() as i64, "mtime_ns": stamp, "ctime_ns": stamp }))
}

fn assert_safe_cached_file(path: &Path, root: &Path) -> Result<(), String> {
    if is_symlink(path) || contains_symlink(path.parent().unwrap_or(path), root) {
        return Err(format!("Небезопасный путь файла экземпляра: {}", path.display()));
    }
    if path.symlink_metadata().is_ok() && !path.is_file() {
        return Err(format!("Путь файла экземпляра не является обычным файлом: {}", path.display()));
    }
    Ok(())
}

fn integrity_cache_matches(instance: &Path, path: &Path, files: &Map<String, Value>, metadata: &Value) -> bool {
    if assert_safe_cached_file(path, instance).is_err() || !path.is_file() {
        return false;
    }
    let Some(relative) = relative_posix(path, instance) else { return false };
    let Some(entry) = files.get(&relative) else { return false };
    if entry.get("metadata") != Some(metadata) {
        return false;
    }
    match file_fingerprint(path) {
        Ok(fingerprint) => entry.get("fingerprint") == Some(&fingerprint),
        Err(_) => false,
    }
}

fn read_integrity_state(instance: &Path, version_dir: &Path) -> Result<Map<String, Value>, String> {
    ensure_safe_directory(version_dir, instance, "каталог версии")?;
    let marker = version_dir.join(".trial-integrity");
    if is_symlink(&marker) || contains_symlink(marker.parent().unwrap_or(&marker), instance) {
        return Err("Файл проверки целостности является ссылкой".to_owned());
    }
    let Ok(meta) = fs::symlink_metadata(&marker) else { return Ok(Map::new()) };
    if !marker.is_file() || meta.len() > 4 * 1024 * 1024 {
        return Ok(Map::new());
    }
    let Ok(raw) = fs::read_to_string(&marker) else { return Ok(Map::new()) };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else { return Ok(Map::new()) };
    match value.get("files").and_then(Value::as_object) {
        Some(files) => Ok(files.clone()),
        None => Ok(Map::new()),
    }
}

fn write_integrity_state(instance: &Path, version_dir: &Path, files: &Map<String, Value>) -> Result<(), String> {
    ensure_safe_directory(version_dir, instance, "каталог версии")?;
    fs::create_dir_all(version_dir).map_err(|error| format!("Не удалось создать каталог {}: {error}", version_dir.display()))?;
    let marker = version_dir.join(".trial-integrity");
    if is_symlink(&marker) || (marker.symlink_metadata().is_ok() && !marker.is_file()) {
        return Err("Файл проверки целостности недоступен".to_owned());
    }
    let temporary = marker.with_file_name(format!(".trial-integrity-{}.part", random_hex(16)));
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("Файл проверки целостности недоступен: {error}"))?;
        let body = serde_json::to_string(&json!({ "files": files })).map_err(|error| error.to_string())?;
        file.write_all(body.as_bytes()).and_then(|_| file.write_all(b"\n")).map_err(|error| format!("Файл проверки целостности недоступен: {error}"))?;
        file.sync_all().ok();
    }
    #[cfg(not(unix))]
    {
        let body = serde_json::to_string(&json!({ "files": files })).map_err(|error| error.to_string())?;
        fs::write(&temporary, format!("{body}\n")).map_err(|error| format!("Файл проверки целостности недоступен: {error}"))?;
    }
    if fs::rename(&temporary, &marker).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("Файл проверки целостности недоступен".to_owned());
    }
    Ok(())
}

fn rules_allow(rules: Option<&Value>) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else { return true };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if !rule.is_object() || !rule_matches(rule) {
            continue;
        }
        allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
    }
    allowed
}

fn rule_matches(rule: &Value) -> bool {
    if let Some(operating_system) = rule.get("os").filter(|value| value.is_object()) {
        if let Some(name) = operating_system.get("name").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            if name != platform_key() {
                return false;
            }
        }
        if let Some(arch) = operating_system.get("arch").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            let machine = machine_arch();
            let aliases: Vec<&str> = match machine {
                "x86_64" => vec!["x86_64", "amd64", "64"],
                "aarch64" => vec!["arm64", "aarch64"],
                other => vec![other],
            };
            if !aliases.contains(&arch.to_lowercase().as_str()) {
                return false;
            }
        }
    }
    if let Some(features) = rule.get("features").and_then(Value::as_object) {
        // Лаунчер — обычный десктопный клиент, флаговые сборки не поддержаны.
        if features.values().any(|value| truthy(Some(value))) {
            return false;
        }
    }
    true
}

fn machine_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
}

fn validate_loader_installer_url(url: &str) -> Result<(), String> {
    let Ok(parsed) = url::Url::parse(url) else {
        return Err("Loader installer имеет некорректный URL".to_owned());
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.fragment().is_some()
        || !matches!(parsed.host_str(), Some("maven.minecraftforge.net") | Some("maven.neoforged.net"))
    {
        return Err("Loader installer должен загружаться с официального Maven endpoint".to_owned());
    }
    Ok(())
}

fn extract_excludes(library: &Value) -> Vec<String> {
    library
        .get("extract")
        .and_then(|value| value.get("exclude"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Распаковка native-архива с лимитами: не более 256 МиБ на файл,
/// 1 ГиБ суммарно, без символьных ссылок и выходов за каталог.
fn safe_extract(archive: &Path, destination: &Path, excluded: &[String]) -> Result<(), String> {
    if is_symlink(destination) {
        return Err("Каталог native archive является символьной ссылкой".to_owned());
    }
    fs::create_dir_all(destination).map_err(|error| format!("Не удалось создать каталог {}: {error}", destination.display()))?;
    let file = File::open(archive).map_err(|_| format!("Native library не является ZIP/JAR: {}", archive.display()))?;
    let mut handle = zip::ZipArchive::new(BufReader::new(file)).map_err(|_| format!("Native library не является ZIP/JAR: {}", archive.display()))?;
    let mut total_size: u64 = 0;
    for index in 0..handle.len() {
        let mut item = handle.by_index(index).map_err(|_| format!("Native library не является ZIP/JAR: {}", archive.display()))?;
        let name = item.name().replace('\\', "/");
        if name.ends_with('/') || name.starts_with("META-INF/") || excluded.iter().any(|value| name == *value || name.starts_with(&format!("{}/", value.trim_end_matches('/')))) {
            continue;
        }
        if item.size() > 256 * 1024 * 1024 || total_size + item.size() > 1024 * 1024 * 1024 {
            return Err("Native archive слишком большой".to_owned());
        }
        if (item.unix_mode().unwrap_or(0o100600) & 0o170000) == 0o120000 {
            return Err(format!("Небезопасная ссылка в native archive: {name}"));
        }
        let parts: Vec<&str> = name.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
        if name.starts_with('/') || parts.is_empty() || parts.iter().any(|part| *part == "..") {
            return Err(format!("Небезопасный путь в native archive: {name}"));
        }
        let written = extract_member(&mut item, destination, &parts, &mut total_size)?;
        total_size += written;
    }
    Ok(())
}

#[cfg(unix)]
fn extract_member(item: &mut zip::read::ZipFile<'_>, destination: &Path, parts: &[&str], total_size: &u64) -> Result<u64, String> {
    use std::os::fd::FromRawFd;
    fn to_cstring(value: &str) -> Result<std::ffi::CString, String> {
        std::ffi::CString::new(value).map_err(|_| "Небезопасный путь в native archive".to_owned())
    }
    let open_directory = |parent: i32, name: &str| -> Option<i32> {
        let cname = to_cstring(name).ok()?;
        let descriptor = unsafe { libc::openat(parent, cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        (descriptor >= 0).then_some(descriptor)
    };
    let root = unsafe { libc::open(to_cstring(&destination.to_string_lossy())?.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if root < 0 {
        return Err(format!("Каталог native archive недоступен: {}", destination.display()));
    }
    // Спускаемся по цепочке каталогов, создавая недостающие (mkdirat).
    let mut parent = root;
    let mut opened = vec![root];
    for part in &parts[..parts.len() - 1] {
        if let Some(descriptor) = open_directory(parent, part) {
            opened.push(descriptor);
            parent = descriptor;
            continue;
        }
        let cname = to_cstring(part)?;
        unsafe { libc::mkdirat(parent, cname.as_ptr(), 0o700) };
        let descriptor = open_directory(parent, part).ok_or_else(|| format!("Небезопасный путь в native archive: {part}"))?;
        opened.push(descriptor);
        parent = descriptor;
    }
    let cname = to_cstring(parts[parts.len() - 1])?;
    let descriptor = unsafe { libc::openat(parent, cname.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
    let result = (|| -> Result<u64, String> {
        if descriptor < 0 {
            return Err(format!("Небезопасный путь в native archive: {}", parts[parts.len() - 1]));
        }
        let mut output = unsafe { File::from_raw_fd(descriptor) };
        let mut written: u64 = 0;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let count = item.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            written += count as u64;
            if written > 256 * 1024 * 1024 || *total_size + written > 1024 * 1024 * 1024 {
                return Err("Native archive слишком большой".to_owned());
            }
            output.write_all(&buffer[..count]).map_err(|error| error.to_string())?;
        }
        Ok(written)
    })();
    for descriptor in opened.into_iter().rev() {
        unsafe { libc::close(descriptor) };
    }
    result
}

#[cfg(not(unix))]
fn extract_member(item: &mut zip::read::ZipFile<'_>, destination: &Path, parts: &[&str], total_size: &u64) -> Result<u64, String> {
    let target = parts.iter().collect::<PathBuf>();
    let parent = target.parent().unwrap_or(destination);
    fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог: {error}"))?;
    let mut output = File::create(&target).map_err(|error| error.to_string())?;
    let mut written: u64 = 0;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = item.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        written += count as u64;
        if written > 256 * 1024 * 1024 || *total_size + written > 1024 * 1024 * 1024 {
            return Err("Native archive слишком большой".to_owned());
        }
        output.write_all(&buffer[..count]).map_err(|error| error.to_string())?;
    }
    Ok(written)
}

fn is_sha1_hex(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn read_asset_marker(marker: &Path) -> Map<String, Value> {
    if is_symlink(marker) || contains_symlink(marker.parent().unwrap_or(marker), marker.parent().and_then(Path::parent).unwrap_or(marker)) || !marker.is_file() {
        return Map::new();
    }
    let Ok(meta) = fs::symlink_metadata(marker) else { return Map::new() };
    if meta.len() > 16 * 1024 * 1024 {
        return Map::new();
    }
    let Ok(raw) = fs::read_to_string(marker) else { return Map::new() };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value.as_object().cloned().unwrap_or_default(),
        Err(_) => Map::new(),
    }
}

fn asset_fingerprint_matches(root: &Path, path: &Path, expected_size: Option<&Value>, fingerprint: Option<&Value>) -> bool {
    let Some(fingerprint) = fingerprint.filter(|value| value.is_object()) else { return false };
    if assert_safe_cached_file(path, root).is_err() || !path.is_file() {
        return false;
    }
    if let Some(expected) = expected_size.filter(|value| !value.is_null()) {
        let Some(expected) = expected.as_i64().or_else(|| expected.as_str().and_then(|text| text.trim().parse::<i64>().ok())) else { return false };
        let Ok(meta) = fs::metadata(path) else { return false };
        if meta.len() as i64 != expected {
            return false;
        }
    }
    matches!(file_fingerprint(path), Ok(actual) if Some(&actual) == Some(fingerprint))
}

fn asset_marker_fingerprints(marker: &Path, objects: &Map<String, Value>) -> Map<String, Value> {
    let value = read_asset_marker(marker);
    let (Some(cached_objects), Some(fingerprints)) = (value.get("objects").and_then(Value::as_object), value.get("fingerprints").and_then(Value::as_object)) else {
        return Map::new();
    };
    let mut result = Map::new();
    for (digest, item) in objects {
        if cached_objects.get(digest) == item.get("size") {
            if let Some(fingerprint) = fingerprints.get(digest).filter(|value| value.is_object()) {
                result.insert(digest.clone(), fingerprint.clone());
            }
        }
    }
    result
}

fn assets_marker_valid(marker: &Path, asset_index: &Value, assets_root: &Path, objects: &Map<String, Value>) -> bool {
    let value = read_asset_marker(marker);
    if value.is_empty() {
        return false;
    }
    if value.get("asset_index_sha1") != asset_index.get("sha1") {
        return false;
    }
    let Some(cached_objects) = value.get("objects").and_then(Value::as_object) else { return false };
    if cached_objects.len() != objects.len() {
        return false;
    }
    let Some(fingerprints) = value.get("fingerprints").and_then(Value::as_object) else { return false };
    for (digest, item) in objects {
        if cached_objects.get(digest) != item.get("size") {
            return false;
        }
        let path = assets_root.join("objects").join(&digest[..2]).join(digest);
        if !asset_fingerprint_matches(assets_root, &path, item.get("size"), fingerprints.get(digest)) {
            return false;
        }
    }
    true
}

fn write_asset_marker(
    marker: &Path,
    asset_index: &Value,
    objects: &Map<String, Value>,
    fingerprints: &Map<String, Value>,
    materialized: &Map<String, Value>,
    index_metadata: Option<&Value>,
    index_fingerprint: Option<&Value>,
) -> Result<(), String> {
    if is_symlink(marker) {
        return Err("Файл состояния assets является символьной ссылкой".to_owned());
    }
    let mut object_sizes = Map::new();
    for (digest, item) in objects {
        object_sizes.insert(digest.clone(), item.get("size").cloned().unwrap_or(Value::Null));
    }
    let value = json!({
        "asset_index_sha1": asset_index.get("sha1").cloned().unwrap_or(Value::Null),
        "asset_index_metadata": index_metadata.cloned().unwrap_or(Value::Null),
        "asset_index_fingerprint": index_fingerprint.cloned().unwrap_or(Value::Null),
        "objects": object_sizes,
        "fingerprints": fingerprints,
        "materialized": materialized,
    });
    let mut text = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    text.push('\n');
    fs::write(marker, text).map_err(|error| format!("Файл состояния assets недоступен: {error}"))
}

/// Legacy-схемы assets (virtual / map_to_resources): объекты раскладываются
/// по человекочитаемым путям поверх копий из objects.
fn materialize_legacy_assets(instance: &Path, assets_root: &Path, index_name: &str, index: &Value, cached: Option<&Value>) -> Result<Map<String, Value>, String> {
    let destination_root = if index.get("virtual").and_then(Value::as_bool).unwrap_or(false) {
        assets_root.join("virtual").join(index_name)
    } else if index.get("map_to_resources").and_then(Value::as_bool).unwrap_or(false) {
        instance.join("resources")
    } else {
        return Ok(Map::new());
    };
    ensure_safe_directory(&destination_root, instance, "legacy assets")?;
    fs::create_dir_all(&destination_root).map_err(|error| format!("Не удалось создать каталог {}: {error}", destination_root.display()))?;
    let Some(objects) = index.get("objects").and_then(Value::as_object) else { return Ok(Map::new()) };
    let cached_files = cached.and_then(Value::as_object).cloned().unwrap_or_default();
    let mut materialized = Map::new();
    for (logical_name, item) in objects {
        let Some(digest) = item.get("hash").and_then(Value::as_str) else { continue };
        let relative = Path::new(logical_name);
        if relative.is_absolute() || relative.components().any(|part| part.as_os_str() == "..") {
            return Err(format!("Небезопасный virtual asset path: {logical_name}"));
        }
        let source = assets_root.join("objects").join(&digest[..2]).join(digest);
        let target = destination_root.join(relative);
        if is_symlink(&target) || contains_symlink(target.parent().unwrap_or(&target), instance) {
            return Err(format!("Небезопасная ссылка в virtual asset path: {logical_name}"));
        }
        if !source.is_file() || is_symlink(&source) || contains_symlink(source.parent().unwrap_or(&source), assets_root) {
            continue;
        }
        fs::create_dir_all(target.parent().unwrap_or(&target)).map_err(|error| format!("Не удалось создать каталог: {error}"))?;
        let target_key = relative_posix(&target, instance).unwrap_or_default();
        let old_fingerprint = cached_files.get(&target_key).and_then(|entry| {
            if entry.get("digest").and_then(Value::as_str) == Some(digest) {
                entry.get("fingerprint")
            } else {
                None
            }
        });
        if !asset_fingerprint_matches(instance, &target, item.get("size"), old_fingerprint) {
            let temporary = target.with_file_name(format!(".{}-{}.part", target.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default(), random_hex(8)));
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temporary)
                    .map_err(|error| format!("Не удалось создать файл asset: {error}"))?;
                let mut input = File::open(&source).map_err(|error| format!("Не удалось прочитать asset: {error}"))?;
                std::io::copy(&mut input, &mut output).map_err(|error| format!("Не удалось записать asset: {error}"))?;
            }
            #[cfg(not(unix))]
            {
                let mut input = File::open(&source).map_err(|error| format!("Не удалось прочитать asset: {error}"))?;
                let mut output = File::create(&temporary).map_err(|error| format!("Не удалось создать файл asset: {error}"))?;
                std::io::copy(&mut input, &mut output).map_err(|error| format!("Не удалось записать asset: {error}"))?;
            }
            fs::rename(&temporary, &target).map_err(|error| {
                let _ = fs::remove_file(&temporary);
                format!("Не удалось записать asset: {error}")
            })?;
        }
        materialized.insert(target_key, json!({ "digest": digest, "fingerprint": file_fingerprint(&target)? }));
    }
    Ok(materialized)
}

fn find_external_version(instance: &Path, loader: &str, loader_version: Option<&str>, minecraft_version: &str) -> Option<(PathBuf, Value)> {
    if is_symlink(instance) || !instance.is_dir() {
        return None;
    }
    let instance_root = canonical_path(instance).ok()?;
    let versions_dir = instance.join("versions");
    if is_symlink(&versions_dir) || !versions_dir.is_dir() {
        return None;
    }
    let mut candidates: Vec<(PathBuf, Value, std::time::SystemTime)> = Vec::new();
    let Ok(entries) = fs::read_dir(&versions_dir) else { return None };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let dir = entry.path();
        if is_symlink(&dir) || !dir.is_dir() {
            continue;
        }
        let lowered = dir.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default();
        let matches_loader = if loader == "forge" {
            lowered.contains("forge") && !lowered.contains("neoforge")
        } else {
            lowered.contains(loader)
        };
        if !matches_loader {
            continue;
        }
        if let Some(loader_version) = loader_version {
            if !lowered.contains(&loader_version.to_lowercase()) {
                continue;
            }
        }
        let Ok(files) = fs::read_dir(&dir) else { continue };
        for file in files.filter_map(|file| file.ok()) {
            let path = file.path();
            if is_symlink(&path) || is_symlink(&dir) {
                continue;
            }
            let Ok(resolved) = canonical_path(&path) else { continue };
            if !within(&resolved, &instance_root) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if !path.is_file() || meta.len() > MAX_INSTANCE_VERSION_JSON_BYTES {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else { continue };
            let Ok(value) = serde_json::from_str::<Value>(&raw) else { continue };
            if !value.is_object() {
                continue;
            }
            let parent = value.get("inheritsFrom").or_else(|| value.get("minecraft")).and_then(Value::as_str);
            if parent != Some(minecraft_version) {
                continue;
            }
            candidates.push((path, value, meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
        }
    }
    candidates.sort_by_key(|(_, _, modified)| *modified);
    candidates.pop().map(|(path, value, _)| (path, value))
}

fn rebase_install_result(result: InstallResult, source: &Path, destination: &Path) -> InstallResult {
    let rebase = |path: &Path| -> PathBuf {
        match path.strip_prefix(source) {
            Ok(relative) => destination.join(relative),
            Err(_) => destination.join(path.file_name().unwrap_or_default()),
        }
    };
    InstallResult {
        version_id: result.version_id,
        instance_dir: destination.to_path_buf(),
        version_json: result.version_json,
        client_jar: rebase(&result.client_jar),
        libraries: result.libraries.iter().map(|path| rebase(path)).collect(),
        natives_dir: rebase(&result.natives_dir),
        assets_root: rebase(&result.assets_root),
        asset_index_name: result.asset_index_name,
        logging_file: result.logging_file.as_ref().map(|path| rebase(path)),
    }
}

/// Смена staging ↔ target: если есть renameat2(RENAME_EXCHANGE) — атомарно,
/// иначе через резервную копию с откатом.
fn swap_staged_directory(staging: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
    if target.symlink_metadata().is_err() {
        fs::rename(staging, target).map_err(|error| format!("Не удалось установить экземпляр: {error}"))?;
        return Ok(None);
    }
    if is_symlink(target) || !target.is_dir() {
        return Err(format!("Путь экземпляра не является каталогом: {}", target.display()));
    }
    if rename_exchange(staging, target)? {
        return Ok(Some(staging.to_path_buf()));
    }
    let backup = target.with_file_name(format!(".trial-backup-{}", random_hex(16)));
    fs::rename(target, &backup).map_err(|error| format!("Не удалось заменить экземпляр: {error}"))?;
    if let Err(error) = fs::rename(staging, target) {
        if fs::rename(&backup, target).is_err() {
            return Err(format!("Не удалось восстановить предыдущий экземпляр; он сохранён в {}", backup.display()));
        }
        return Err(format!("Не удалось установить экземпляр: {error}"));
    }
    Ok(Some(backup))
}

#[cfg(unix)]
fn rename_exchange(source: &Path, destination: &Path) -> Result<bool, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| "Небезопасный путь экземпляра".to_owned())?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| "Небезопасный путь экземпляра".to_owned())?;
    let result = unsafe { libc::renameat2(libc::AT_FDCWD, source.as_ptr(), libc::AT_FDCWD, destination.as_ptr(), RENAME_EXCHANGE) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if matches!(error, libc::ENOSYS | libc::EINVAL | libc::ENOTSUP) {
        return Ok(false);
    }
    Err(std::io::Error::from_raw_os_error(error).to_string())
}

#[cfg(not(unix))]
fn rename_exchange(_source: &Path, _destination: &Path) -> Result<bool, String> {
    Ok(false)
}

fn remove_instance_path(path: &Path) -> Result<(), String> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    if is_symlink(path) || !path.is_dir() {
        fs::remove_file(path).map_err(|error| error.to_string())
    } else {
        fs::remove_dir_all(path).map_err(|error| error.to_string())
    }
}

/// Уникальный суффикс имён временных файлов: уникальность достаточна,
/// криптографичность не нужна.
fn random_hex(bytes: usize) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    thread_local!(static STATE: RefCell<u64> = RefCell::new(0));
    STATE.with(|state| {
        let mut seed = state.borrow_mut();
        if *seed == 0 {
            *seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos() as u64)
                .unwrap_or(0)
                ^ (std::process::id() as u64).rotate_left(32)
                ^ COUNTER.fetch_add(1, Ordering::Relaxed).rotate_left(17);
        }
        let mut out = String::with_capacity(bytes * 2);
        for _ in 0..bytes {
            *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            out.push_str(&format!("{:02x}", (z & 0xFF) as u8));
        }
        out
    })
}

// ------------------------------ операция ------------------------------

pub fn install(request: &Value) -> Result<Value, String> {
    let mut config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    if truthy(request.get("fail_if_exists")) {
        let name = request.get("instance_name").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if name.is_empty() {
            return Err("Для нового экземпляра укажите уникальное имя".to_owned());
        }
        let target = store.root().join(safe_instance_name(&name));
        if target.symlink_metadata().is_ok() {
            return Err(format!(
                "Экземпляр уже существует: {}. Для модпака выберите другое имя.",
                target.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default()
            ));
        }
    }
    if let Some(java_path) = request.get("java_path") {
        config.set_values(&json!({ "java_path": java_path.as_str().unwrap_or("").trim() }))?;
    }
    let memory = request.get("memory").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    if !memory.is_empty() {
        config.set_values(&json!({ "memory": memory }))?;
    }
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let http = client_from_settings(config.settings(), operation_id)?;
    let mut installer = MinecraftInstaller::new(&config, http)?;
    let loader_version = request.get("loader_version").and_then(Value::as_str).filter(|value| !value.is_empty());
    let instance_name = request.get("instance_name").and_then(Value::as_str).filter(|value| !value.is_empty());
    let loader = request.get("loader").and_then(Value::as_str).unwrap_or("vanilla");
    let version = request.get("version").and_then(Value::as_str).unwrap_or("");
    let result = installer.install(
        version,
        loader,
        loader_version,
        instance_name,
        !truthy(request.get("no_assets")),
        truthy(request.get("replace_existing")),
    )?;
    let list: Vec<Value> = store.list().iter().map(|info| info.public()).collect();
    Ok(json!({
        "version_id": result.version_id,
        "instance_dir": result.instance_dir.to_string_lossy(),
        "instances": list,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-installer-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn loader_installer_uses_install_client_directory_argument() {
        let args = loader_installer_args(Path::new("/x/installer.jar"), Path::new("/x/instance"));
        assert_eq!(args, vec!["-jar", "/x/installer.jar", "--installClient", "/x/instance"]);
        assert!(!args.iter().any(|value| value == "--target"));
    }

    #[test]
    fn launcher_profiles_is_created_once() {
        let root = temp_root("launcher-profiles");
        ensure_launcher_profiles(&root).unwrap();
        let path = root.join("launcher_profiles.json");
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["profiles"].is_object());
        fs::write(&path, "{\"profiles\":{\"keep\":{}}}").unwrap();
        ensure_launcher_profiles(&root).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("keep"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stream_tail_keeps_only_last_bytes() {
        let data = vec![b'a'; 100_000].into_iter().chain(b"END".iter().copied()).collect::<Vec<u8>>();
        let tail = read_stream_tail(std::io::Cursor::new(data), 10);
        assert_eq!(tail.len(), 10);
        assert!(tail.ends_with("END"));
    }

    #[test]
    fn version_id_and_component_guards() {
        assert!(safe_version_id("1.20.4").is_ok());
        assert!(safe_version_id("").is_err());
        assert!(safe_version_id("../evil").is_err());
        assert!(safe_version_id(&"x".repeat(201)).is_err());
        assert_eq!(safe_component("1.20.4", "asset index").unwrap(), "1.20.4");
        assert!(safe_component("a/b", "asset index").is_err());
    }

    #[test]
    fn relative_path_rejects_traversal_and_normalizes() {
        let path = safe_relative_path("com/mojang/lib.jar", "library").unwrap();
        assert_eq!(path, Path::new("com/mojang/lib.jar"));
        assert!(safe_relative_path("../escape.jar", "library").is_err());
        assert!(safe_relative_path("/abs.jar", "library").is_err());
        assert!(safe_relative_path("a//b/./c.jar", "library").is_ok());
    }

    #[test]
    fn integrity_metadata_takes_known_digests_only() {
        let metadata = integrity_metadata(&json!({ "sha1": "ABC", "sha256": "", "size": 5 })).unwrap().unwrap();
        assert_eq!(metadata["sha1"], json!("abc"));
        assert!(metadata.get("sha256").is_none());
        assert_eq!(metadata["size"], json!(5));
        assert!(integrity_metadata(&json!({ "md5": "x" })).unwrap().is_none());
        assert_eq!(integrity_metadata(&json!({ "size": -1 })).unwrap_err(), "Размер файла в metadata некорректен");
    }

    #[test]
    fn rules_allow_follows_last_matching_action() {
        assert!(rules_allow(None));
        assert!(rules_allow(Some(&json!([]))));
        let os_rules = json!([
            { "action": "allow", "os": { "name": platform_key() } },
            { "action": "disallow", "os": { "name": "definitely-not" } },
        ]);
        assert!(rules_allow(Some(&os_rules)));
        let feature_rules = json!([{ "action": "allow", "features": { "is_demo_user": true } }]);
        assert!(!rules_allow(Some(&feature_rules)));
    }

    #[test]
    fn loader_installer_url_whitelist() {
        assert!(validate_loader_installer_url("https://maven.minecraftforge.net/net/x.jar").is_ok());
        assert!(validate_loader_installer_url("https://maven.neoforged.net/x.jar").is_ok());
        assert!(validate_loader_installer_url("http://maven.minecraftforge.net/x.jar").is_err());
        assert!(validate_loader_installer_url("https://evil.example.com/x.jar").is_err());
        assert!(validate_loader_installer_url("https://maven.minecraftforge.net:8443/x.jar").is_err());
        assert!(validate_loader_installer_url("https://user@maven.minecraftforge.net/x.jar").is_err());
    }

    #[test]
    fn symlink_guards_detect_links_and_escapes() {
        let root = temp_root("guards");
        let inner = root.join("instance");
        fs::create_dir_all(&inner).unwrap();
        assert!(!contains_symlink(&inner, &root));
        // Путь вне root останавливает подъём к корню.
        assert!(contains_symlink(Path::new("/etc/passwd"), &root));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&inner, root.join("link")).unwrap();
            assert!(is_symlink(&root.join("link")));
        }
        // «..»-суффикс лексически внутри root — как в Python, подъём по
        // parent() сворачивает его раньше сравнения; реальную защиту даёт
        // канонизация пути в load_existing.
        assert!(!contains_symlink(&root.join(".."), &root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn swap_uses_exchange_or_backup() {
        let root = temp_root("swap");
        let staging = root.join(".trial-stage-x");
        let target = root.join("target");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("old.txt"), "old").unwrap();
        fs::write(staging.join("new.txt"), "new").unwrap();
        let old = swap_staged_directory(&staging, &target).unwrap();
        assert!(target.join("new.txt").is_file());
        if let Some(old) = old {
            assert!(old.join("old.txt").is_file());
            let _ = remove_instance_path(&old);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn random_hex_is_unique_per_call() {
        assert_ne!(random_hex(8), random_hex(8));
        assert_eq!(random_hex(4).len(), 8);
        assert!(random_hex(8).chars().all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn asset_helpers_validate_hashes_and_markers() {
        assert!(is_sha1_hex("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_sha1_hex("short"));
        let root = temp_root("assets");
        let marker = root.join("marker.json");
        assert!(read_asset_marker(&marker).is_empty());
        fs::write(&marker, json!({ "asset_index_sha1": "abc" }).to_string()).unwrap();
        assert_eq!(read_asset_marker(&marker).get("asset_index_sha1"), Some(&json!("abc")));
        let mut fingerprints = Map::new();
        fingerprints.insert("d".into(), json!({ "size": 1 }));
        let mut objects = Map::new();
        objects.insert("d".into(), json!({ "hash": "d", "size": 1 }));
        write_asset_marker(&marker, &json!({ "sha1": "abc" }), &objects, &fingerprints, &Map::new(), None, None).unwrap();
        let raw = fs::read_to_string(&marker).unwrap();
        assert!(raw.ends_with("}\n") && raw.contains("\"asset_index_sha1\":\"abc\""));
        let _ = fs::remove_dir_all(&root);
    }
}


