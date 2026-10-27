// каталог Modrinth и установка ресурсов с зависимостями
use serde_json::{json, Map, Value};
use std::cell::Cell;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::net::HttpClient;

pub(crate) const PROJECT_METADATA_SUFFIX: &str = ".trial-project.json";
const MAX_MOD_DEPENDENCY_PROJECTS: usize = 64;
const MAX_MOD_DEPENDENCY_DEPTH: usize = 16;
const MAX_REQUIRED_DEPENDENCIES_PER_VERSION: usize = 128;
const MAX_PROJECT_METADATA_BYTES: u64 = 64 * 1024;

fn project_directory(project_type: &str) -> Option<&'static str> {
    match project_type {
        "mod" => Some("mods"),
        "resourcepack" => Some("resourcepacks"),
        "shader" => Some("shaderpacks"),
        "datapack" => Some("datapacks"),
        _ => None,
    }
}

fn search_ordering_valid(value: &str) -> bool {
    matches!(value, "relevance" | "downloads" | "newest" | "updated")
}

pub(crate) struct ModResult {
    pub(crate) source: String,
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) file_name: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) version_id: String,
    pub(crate) dependencies: Vec<Value>,
}

#[derive(Clone)]
struct ResolvedProject {
    project_id: String,
    version: Value,
    project_metadata: Value,
    primary_file: Value,
    file_name: String,
    dependencies: Vec<String>,
    publisher_hashes: PublisherHashes,
    size: Option<u64>,
    target: Option<PathBuf>,
}

#[derive(Clone, Default)]
struct PublisherHashes {
    sha1: Option<String>,
    sha256: Option<String>,
    sha512: Option<String>,
}

impl PublisherHashes {
    fn any(&self) -> bool {
        self.sha1.is_some() || self.sha256.is_some() || self.sha512.is_some()
    }
}

pub(crate) struct ModrinthProvider {
    http: Arc<HttpClient>,
    base_url: String,
    catalog_id: String,
    last_total: Cell<Option<i64>>,
}

impl ModrinthProvider {
    pub(crate) fn new(http: Arc<HttpClient>, base_url: &str, catalog_id: &str) -> Self {
        let trimmed = base_url.trim_end_matches('/');
        let base_url = if trimmed.is_empty() {
            super::constants::MODRINTH_API_URL.to_owned()
        } else {
            trimmed.to_owned()
        };
        let catalog_id: String = catalog_id.chars().take(80).collect::<String>().trim().to_owned();
        let catalog_id = if catalog_id.is_empty() { "modrinth".to_owned() } else { catalog_id };
        Self { http, base_url, catalog_id, last_total: Cell::new(None) }
    }

    pub(crate) fn last_total(&self) -> Option<i64> {
        self.last_total.get()
    }

    pub(crate) fn search(
        &self,
        query: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
        limit: i64,
        offset: i64,
        project_type: &str,
        category: Option<&str>,
        sort_by: &str,
    ) -> Result<Vec<Value>, String> {
        validate_catalog_project_type(project_type)?;
        let sort_by = sort_by.trim().to_lowercase();
        if !search_ordering_valid(&sort_by) {
            return Err(format!("Неизвестный порядок сортировки Modrinth: {sort_by}"));
        }
        let limit = limit.clamp(1, 100);
        let offset = offset.max(0);

        let facets = search_facets(project_type, game_version, loader, category)?;
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
            ("index", sort_by),
            ("facets", serde_json::to_string(&facets).map_err(|error| error.to_string())?),
        ];
        let trimmed_query = query.trim();
        if !trimmed_query.is_empty() {
            params.push(("query", trimmed_query.to_owned()));
        }
        let url = format!("{}/search?{}", self.base_url, form_urlencode(&params));
        let value = self.http.get_json(&url, None)?;
        self.last_total
            .set(value.get("total_hits").and_then(Value::as_i64).filter(|total| *total >= 0));
        let hits = value.get("hits").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(hits
            .into_iter()
            .filter(|item| item.is_object() && matches_project_type(item, project_type))
            .collect())
    }

    pub(crate) fn project_details(&self, project_id: &str, project_type: &str) -> Result<Value, String> {
        validate_catalog_project_type(project_type)?;
        let project_id = validated_project_id(project_id)?;
        let url = format!("{}/project/{}", self.base_url, percent_encode(&project_id));
        let value = self.http.get_json(&url, None)?;
        if !value.is_object() {
            return Err(format!("Modrinth вернул некорректные данные проекта {project_id}"));
        }
        validate_detail_project_type(&value, &project_id, project_type)?;
        Ok(value)
    }

    pub(crate) fn compatible_versions(
        &self,
        project_id: &str,
        game_version: &str,
        loader: Option<&str>,
        project_type: &str,
    ) -> Result<Vec<Value>, String> {
        self.project_details(project_id, project_type)?;
        self.compatible_versions_inner(project_id, game_version, loader, project_type)
    }

    fn compatible_versions_inner(
        &self,
        project_id: &str,
        game_version: &str,
        loader: Option<&str>,
        project_type: &str,
    ) -> Result<Vec<Value>, String> {
        validate_project_type(project_type)?;
        let project_id = validated_project_id(project_id)?;
        let game_version = game_version.trim();
        let loader = loader.map(str::trim).unwrap_or("").to_lowercase();
        let mut params: Vec<(&str, String)> = Vec::new();
        if !game_version.is_empty() {
            params.push(("game_versions", format!("[\"{game_version}\"]")));
        }
        if !loader.is_empty() && loader != "vanilla" && project_type == "mod" {
            params.push(("loaders", format!("[\"{loader}\"]")));
        }
        let encoded = percent_encode(&project_id);
        let endpoint = format!("{}/project/{encoded}/version", self.base_url);
        let url = if params.is_empty() {
            endpoint
        } else {
            format!("{endpoint}?{}", form_urlencode(&params))
        };
        let value = self.http.get_json(&url, None)?;
        let versions = value.as_array().cloned().unwrap_or_default();
        Ok(versions
            .into_iter()
            .filter(|item| {
                item.is_object()
                    && matches_project_type(item, project_type)
                    && version_is_compatible(item, game_version, &loader, project_type)
                    && (item.get("project_id").is_none()
                        || item.get("project_id").and_then(Value::as_str) == Some(project_id.as_str()))
            })
            .collect())
    }

    pub(crate) fn install(
        &self,
        project: &str,
        instance_dir: &Path,
        minecraft_version: &str,
        loader: Option<&str>,
        project_type: &str,
        target_dir: Option<&Path>,
        confirm_unverified: bool,
        version_id: Option<&str>,
        allow_replace: bool,
    ) -> Result<ModResult, String> {
        validate_project_type(project_type)?;
        let project_id = validated_project_id(project)?;
        let game_version = minecraft_version.trim();
        let loader_value = loader.map(str::trim).unwrap_or("").to_lowercase();
        let loader_value = if loader_value.is_empty() { None } else { Some(loader_value) };
        let (install_root, destination_dir) = resource_paths(instance_dir, project_type, target_dir)?;
        validate_install_paths(&install_root, &destination_dir, project_type)?;

        let (mut projects, by_id) =
            self.resolve_project_tree(&project_id, game_version, loader_value.as_deref(), project_type, version_id)?;
        for item in projects.iter_mut() {
            let hashes = item.primary_file.get("hashes").filter(|value| value.is_object());
            item.publisher_hashes = publisher_hashes(hashes);
            item.size = optional_size(item.primary_file.get("size"));
            if !item.publisher_hashes.any() && !confirm_unverified {
                return Err(format!(
                    "[unverified-download] Modrinth не предоставил доверенный хеш файла для проекта {}. Подтвердите скачивание явно.",
                    item.project_id
                ));
            }
        }

        self.prepare_targets(&mut projects[..], &destination_dir, project_type)?;
        let root_position = projects
            .iter()
            .position(|item| item.project_id == project_id)
            .ok_or_else(|| "Не удалось определить путь установленного ресурса".to_owned())?;
        {
            let root = &projects[root_position];
            if !allow_replace {
                if let Some(target) = &root.target {
                    if target.symlink_metadata().is_ok() || metadata_path(target).symlink_metadata().is_ok() {
                        return Err(format!(
                            "Ресурс {project_id} уже установлен в этом экземпляре. Для смены версии используйте обновление, либо сначала удалите ресурс."
                        ));
                    }
                }
            }
        }
        self.stage_and_commit(
            &mut projects[..],
            &by_id,
            &install_root,
            &destination_dir,
            game_version,
            loader_value.as_deref(),
            project_type,
            &project_id,
            allow_replace,
        )?;

        let root = &projects[root_position];
        let target = root
            .target
            .clone()
            .ok_or_else(|| "Не удалось определить путь установленного ресурса".to_owned())?;
        let name = root
            .project_metadata
            .get("title")
            .or_else(|| root.version.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| project_id.clone());
        let dependencies = root
            .dependencies
            .iter()
            .map(|dependency_id| {
                json!({
                    "project_id": dependency_id,
                    "version_id": by_id.get(dependency_id).map(|version| version_id_of(version)).unwrap_or_default(),
                    "dependency_type": "required",
                })
            })
            .collect();
        Ok(ModResult {
            source: "modrinth".to_owned(),
            project_id,
            name,
            version: root
                .version
                .get("version_number")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            file_name: target.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default(),
            path: Some(target),
            version_id: version_id_of(&root.version),
            dependencies,
        })
    }

    pub(crate) fn check_updates(
        &self,
        instance_dir: &Path,
        game_version: &str,
        loader: Option<&str>,
        project_type: &str,
        target_dir: Option<&Path>,
    ) -> Result<Vec<Value>, String> {
        validate_project_type(project_type)?;
        let (install_root, directory) = resource_paths(instance_dir, project_type, target_dir)?;
        if is_symlink(&install_root) {
            return Err("Каталог экземпляра является символьной ссылкой".to_owned());
        }
        if !install_root.exists() {
            return Ok(Vec::new());
        }
        if !install_root.is_dir() || is_symlink(&directory) {
            return Err("Каталог установленного ресурса недоступен".to_owned());
        }
        if !directory.exists() {
            return Ok(Vec::new());
        }
        if !directory.is_dir() {
            return Err("Каталог установленного ресурса недоступен".to_owned());
        }

        let mut updates: Vec<Value> = Vec::new();
        let mut checked: Vec<String> = Vec::new();
        let mut sidecars = collect_sidecars(&directory);
        sidecars.sort_by_key(|path| path.to_string_lossy().to_lowercase());
        for sidecar in sidecars {
            let metadata = read_metadata(&sidecar);
            if !self.trusted_project_metadata(&metadata, project_type) {
                continue;
            }
            let project_id = metadata.get("project_id").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            let Some(target) = target_for_sidecar(&sidecar) else { continue };
            if project_id.is_empty()
                || checked.contains(&project_id)
                || is_symlink(&target)
                || !target.is_file()
                || !within_resolved(&sidecar, &directory)
                || !within_resolved(&target, &directory)
            {
                continue;
            }
            checked.push(project_id.clone());

            let encoded = percent_encode(&project_id);
            let details = self.http.get_json(&format!("{}/project/{encoded}", self.base_url), None)?;
            if !details.is_object()
                || details.get("project_type").and_then(Value::as_str).map(str::trim) != Some(project_type)
            {
                continue;
            }
            let versions = self.compatible_versions_inner(&project_id, game_version, loader, project_type)?;
            let Some(latest) = versions.first() else { continue };
            let latest_version_id = version_id_of(latest);
            let installed_version_id = metadata.get("version_id").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            if latest_version_id.is_empty() || latest_version_id == installed_version_id {
                continue;
            }
            updates.push(json!({
                "source": "Modrinth",
                "catalog": "modrinth",
                "catalog_id": self.catalog_id,
                "project_id": project_id,
                "project_type": project_type,
                "name": metadata.get("display_name").or_else(|| metadata.get("name")).and_then(Value::as_str)
                    .unwrap_or(&project_id),
                "current_version": metadata.get("version").and_then(Value::as_str).unwrap_or(""),
                "current_version_id": installed_version_id,
                "version": latest.get("version_number").and_then(Value::as_str).unwrap_or(""),
                "version_id": latest_version_id,
                "game_version": game_version,
                "loader": loader.map(str::trim).unwrap_or("vanilla").to_lowercase(),
                "path": target.to_string_lossy(),
            }));
        }
        Ok(updates)
    }

    fn resolve_project_tree(
        &self,
        project_id: &str,
        game_version: &str,
        loader: Option<&str>,
        project_type: &str,
        version_id: Option<&str>,
    ) -> Result<(Vec<ResolvedProject>, HashMap<String, Value>), String> {
        let mut selected: Vec<ResolvedProject> = Vec::new();
        // сохраняем выбранную версию проекта для итоговых зависимостей
                let mut by_id: HashMap<String, Value> = HashMap::new();
        self.visit_project(
            project_id,
            version_id.filter(|value| !value.trim().is_empty()),
            0,
            game_version,
            loader,
            project_type,
            &mut selected,
            &mut by_id,
        )?;
        Ok((selected, by_id))
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_project(
        &self,
        current_id: &str,
        pinned_version_id: Option<&str>,
        depth: usize,
        game_version: &str,
        loader: Option<&str>,
        project_type: &str,
        selected: &mut Vec<ResolvedProject>,
        by_id: &mut HashMap<String, Value>,
    ) -> Result<(), String> {
        let current_id = validated_project_id(current_id)?;
        if let Some(existing) = selected.iter().find(|item| item.project_id == current_id) {
            if let Some(pinned) = pinned_version_id {
                if version_id_of(&existing.version) != pinned {
                    return Err(format!(
                        "Зависимости требуют разные версии проекта {current_id}; установить обе версии одновременно нельзя"
                    ));
                }
            }
            return Ok(());
        }
        if depth > MAX_MOD_DEPENDENCY_DEPTH {
            return Err("Цепочка зависимостей Modrinth слишком глубокая".to_owned());
        }
        if selected.len() >= MAX_MOD_DEPENDENCY_PROJECTS {
            return Err("В дереве зависимостей Modrinth слишком много проектов".to_owned());
        }

        let project_metadata = self.project_metadata_for_install(&current_id, project_type);
        let versions = self.compatible_versions_inner(&current_id, game_version, loader, project_type)?;
        let mut version = pinned_version_id
            .and_then(|pinned| versions.iter().find(|item| version_id_of(item) == pinned).cloned());
        if version.is_none() && pinned_version_id.is_none() {
            version = versions.first().cloned();
        }
        let Some(version) = version else {
            if let Some(pinned) = pinned_version_id {
                return Err(format!(
                    "Обязательная версия {pinned} проекта {current_id} несовместима с выбранной игрой или загрузчиком"
                ));
            }
            return Err(format!("Modrinth не нашёл совместимую версию для {current_id}"));
        };
        let reported_project_id = version.get("project_id").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if !reported_project_id.is_empty() && reported_project_id != current_id {
            return Err(format!("Modrinth вернул версию другого проекта для {current_id}"));
        }
        let primary = primary_file(&version, &current_id)?;
        let file_name = safe_file_name(
            primary
                .get("filename")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&format!("{current_id}.jar")),
        )?;
        let project = ResolvedProject {
            project_id: current_id.clone(),
            version,
            project_metadata,
            primary_file: primary,
            file_name,
            dependencies: Vec::new(),
            publisher_hashes: PublisherHashes::default(),
            size: None,
            target: None,
        };
        selected.push(project);
        by_id.insert(current_id.clone(), selected.last().unwrap().version.clone());

        let mut direct_dependencies: Vec<String> = Vec::new();
        let required = self.required_dependencies(&selected.last().unwrap().version, &current_id)?;
        for (dependency_id, dependency_version_id) in required {
            let dependency_id = if dependency_id.is_empty() {
                self.project_id_for_version(dependency_version_id.as_deref().unwrap_or(""))?
            } else {
                dependency_id
            };
            if direct_dependencies.contains(&dependency_id) {
                if let Some(pinned) = &dependency_version_id {
                    if let Some(prior) = by_id.get(&dependency_id) {
                        if version_id_of(prior) != *pinned {
                            return Err(format!("Зависимости требуют разные версии проекта {dependency_id}"));
                        }
                    }
                }
                continue;
            }
            direct_dependencies.push(dependency_id.clone());
            self.visit_project(
                &dependency_id,
                dependency_version_id.as_deref(),
                depth + 1,
                game_version,
                loader,
                project_type,
                selected,
                by_id,
            )?;
            selected
                .iter_mut()
                .find(|item| item.project_id == current_id)
                .expect("проект добавлен выше")
                .dependencies
                .push(dependency_id);
        }
        Ok(())
    }

    fn required_dependencies(&self, version: &Value, project_id: &str) -> Result<Vec<(String, Option<String>)>, String> {
        let Some(raw) = version.get("dependencies") else {
            return Ok(Vec::new());
        };
        let Some(raw) = raw.as_array() else {
            return Err(format!("Список зависимостей проекта {project_id} имеет неожиданный формат"));
        };
        if raw.len() > MAX_REQUIRED_DEPENDENCIES_PER_VERSION {
            return Err(format!("У проекта {project_id} слишком много зависимостей"));
        }
        let mut dependencies: Vec<(String, Option<String>)> = Vec::new();
        for item in raw {
            if !item.is_object()
                || item
                    .get("dependency_type")
                    .and_then(Value::as_str)
                    .map(|value| value.to_lowercase() != "required")
                    .unwrap_or(true)
            {
                continue;
            }
            let dependency_id = item.get("project_id").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            let dependency_version_id = item
                .get("version_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            if dependency_id.is_empty() && dependency_version_id.is_none() {
                return Err(format!("Обязательная зависимость проекта {project_id} не имеет ID"));
            }
            let dependency_id = if dependency_id.is_empty() {
                String::new()
            } else {
                validated_project_id(&dependency_id)?
            };
            dependencies.push((dependency_id, dependency_version_id));
        }
        Ok(dependencies)
    }

    fn project_id_for_version(&self, version_id: &str) -> Result<String, String> {
        if version_id.is_empty() {
            return Err("Обязательная зависимость Modrinth не имеет project ID".to_owned());
        }
        let encoded = percent_encode(version_id);
        let value = self.http.get_json(&format!("{}/version/{encoded}", self.base_url), None)?;
        let project_id = value.get("project_id").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if !value.is_object() || project_id.is_empty() {
            return Err(format!("Не удалось определить проект для версии зависимости {version_id}"));
        }
        validated_project_id(&project_id)
    }

    fn project_metadata_for_install(&self, project_id: &str, project_type: &str) -> Value {
        let encoded = percent_encode(project_id);
        let Ok(value) = self.http.get_json(&format!("{}/project/{encoded}", self.base_url), None) else {
            // если каталог недоступен, устанавливаем без названия и иконки
                        return json!({});
        };
        if !value.is_object() {
            return json!({});
        }
        if validate_detail_project_type(&value, project_id, project_type).is_err() {
            return json!({});
        }
        value
    }

    fn trusted_project_metadata(&self, metadata: &Map<String, Value>, project_type: &str) -> bool {
        metadata.get("source").and_then(Value::as_str).map(|value| value.to_lowercase() == "modrinth").unwrap_or(false)
            && metadata.get("catalog").and_then(Value::as_str).map(|value| value.trim().to_lowercase() == "modrinth").unwrap_or(false)
            && metadata.get("catalog_id").and_then(Value::as_str).map(|value| value.trim() == self.catalog_id).unwrap_or(false)
            && metadata.get("catalog_url").and_then(Value::as_str).map(|value| value.trim() == self.base_url).unwrap_or(false)
            && metadata.get("project_type").and_then(Value::as_str).map(|value| value.trim() == project_type).unwrap_or(false)
            && metadata.get("project_id").and_then(Value::as_str).map(|value| !value.trim().is_empty()).unwrap_or(false)
            && metadata.get("version_id").and_then(Value::as_str).map(|value| !value.trim().is_empty()).unwrap_or(false)
            && metadata.get("game_version").map(Value::is_string).unwrap_or(false)
            && metadata.get("loader").map(Value::is_string).unwrap_or(false)
    }

    fn same_catalog_metadata(&self, metadata: &Map<String, Value>) -> bool {
        let catalog = metadata.get("catalog").and_then(Value::as_str).unwrap_or("modrinth").trim().to_lowercase();
        let catalog_id = metadata
            .get("catalog_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| self.catalog_id.clone())
            .trim()
            .to_owned();
        let catalog_url = metadata.get("catalog_url").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        catalog == "modrinth" && catalog_id == self.catalog_id && (catalog_url.is_empty() || catalog_url == self.base_url)
    }

    fn prepare_targets(&self, projects: &mut [ResolvedProject], directory: &Path, project_type: &str) -> Result<(), String> {
        if is_symlink(directory) {
            return Err("Каталог ресурса является символьной ссылкой".to_owned());
        }
        let mut used: Vec<PathBuf> = Vec::new();
        for project in projects.iter_mut() {
            let target = self
                .existing_project_target(directory, project, project_type)?
                .unwrap_or_else(|| directory.join(&project.file_name));
            let sidecar = metadata_path(&target);
            if used.contains(&target) || used.contains(&sidecar) {
                return Err(format!("Несколько проектов Modrinth используют файл {}", target.file_name().map(|value| value.to_string_lossy()).unwrap_or_default()));
            }
            if !within(&target, directory) || !within(&sidecar, directory) {
                return Err(format!("Небезопасный путь файла проекта: {}", target.file_name().map(|value| value.to_string_lossy()).unwrap_or_default()));
            }
            used.push(target.clone());
            used.push(sidecar.clone());
            self.validate_replaceable_pair(&target, &sidecar, project, project_type)?;
            project.target = Some(target);
        }
        Ok(())
    }

    fn existing_project_target(
        &self,
        directory: &Path,
        project: &ResolvedProject,
        project_type: &str,
    ) -> Result<Option<PathBuf>, String> {
        if !directory.is_dir() {
            return Ok(None);
        }
        let mut matches: Vec<PathBuf> = Vec::new();
        for sidecar in flat_list(directory) {
            if is_symlink(&sidecar) || !sidecar.is_file() {
                continue;
            }
            let metadata = read_metadata(&sidecar);
            if metadata.get("project_id").and_then(Value::as_str).unwrap_or("").trim() != project.project_id {
                continue;
            }
            if metadata.get("project_type").and_then(Value::as_str).unwrap_or("").trim() != project_type {
                return Err(format!("Метаданные проекта {} содержат другой тип ресурса", project.project_id));
            }
            if !self.same_catalog_metadata(&metadata) {
                continue;
            }
            if let Some(target) = target_for_sidecar(&sidecar) {
                matches.push(target);
            }
        }
        if matches.len() > 1 {
            return Err(format!("Для проекта {} найдено несколько установленных файлов", project.project_id));
        }
        Ok(matches.pop())
    }

    fn validate_replaceable_pair(
        &self,
        target: &Path,
        sidecar: &Path,
        project: &ResolvedProject,
        project_type: &str,
    ) -> Result<(), String> {
        if is_symlink(target) || is_symlink(sidecar) {
            return Err("Файл ресурса или его метаданные являются символьной ссылкой".to_owned());
        }
        if target.exists() && !target.is_file() {
            return Err(format!("Путь ресурса уже занят: {}", target.file_name().map(|value| value.to_string_lossy()).unwrap_or_default()));
        }
        if sidecar.exists() && !sidecar.is_file() {
            return Err(format!("Путь метаданных уже занят: {}", sidecar.file_name().map(|value| value.to_string_lossy()).unwrap_or_default()));
        }
        if !target.exists() && !sidecar.exists() {
            return Ok(());
        }
        let metadata = read_metadata(sidecar);
        if metadata.get("project_id").and_then(Value::as_str).unwrap_or("").trim() != project.project_id
            || metadata.get("project_type").and_then(Value::as_str).unwrap_or("").trim() != project_type
            || metadata.get("source").and_then(Value::as_str).unwrap_or("").to_lowercase() != "modrinth"
            || !self.same_catalog_metadata(&metadata)
        {
            return Err(format!(
                "Файл уже существует и не принадлежит проекту {}: {}",
                project.project_id,
                target.file_name().map(|value| value.to_string_lossy()).unwrap_or_default()
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_and_commit(
        &self,
        projects: &mut [ResolvedProject],
        by_id: &HashMap<String, Value>,
        install_root: &Path,
        destination_dir: &Path,
        game_version: &str,
        loader: Option<&str>,
        project_type: &str,
        root_project_id: &str,
        allow_replace: bool,
    ) -> Result<(), String> {
        let staging_parent = if destination_dir.is_dir() {
            destination_dir
        } else if install_root.is_dir() {
            install_root
        } else {
            install_root.parent().unwrap_or(install_root)
        };
        if is_symlink(staging_parent) || !staging_parent.is_dir() {
            return Err("Не удалось создать временный каталог для ресурсов".to_owned());
        }
        let staging = StagingDirectory::new(staging_parent)?;
        let mut staged_files: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (index, project) in projects.iter().enumerate() {
            let staged_asset = staging.path.join(format!("asset-{index}"));
            let url = project
                .primary_file
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("У Modrinth-проекта {} нет файла для скачивания", project.project_id))?;
            self.http.download(
                url,
                &staged_asset,
                project.publisher_hashes.sha1.as_deref(),
                project.publisher_hashes.sha256.as_deref(),
                project.publisher_hashes.sha512.as_deref(),
                project.size,
                None,
                None,
                None,
            )?;
            let metadata = sidecar_metadata(project, by_id, game_version, loader, project_type, &self.catalog_id, &self.base_url);
            let staged_metadata = staging.path.join(format!("metadata-{index}.json"));
            write_private_json(&staged_metadata, &metadata)?;
            staged_files.push((staged_asset, staged_metadata));
        }
        self.commit_staged_files(
            projects,
            &staged_files,
            install_root,
            destination_dir,
            &staging.path,
            project_type,
            root_project_id,
            allow_replace,
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_staged_files(
        &self,
        projects: &mut [ResolvedProject],
        staged_files: &[(PathBuf, PathBuf)],
        install_root: &Path,
        destination_dir: &Path,
        staging: &Path,
        project_type: &str,
        root_project_id: &str,
        allow_replace: bool,
    ) -> Result<(), String> {
        let mut created_directories: Vec<PathBuf> = Vec::new();
        let mut installed_paths: Vec<PathBuf> = Vec::new();
        let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
        let result = (|| -> Result<(), String> {
            for directory in [install_root, destination_dir] {
                if !directory.exists() {
                    fs::create_dir(directory).map_err(|error| format!("Не удалось создать каталог {}: {error}", directory.display()))?;
                    created_directories.push(directory.to_path_buf());
                }
                if is_symlink(directory) || !directory.is_dir() {
                    return Err("Каталог установки изменился во время скачивания".to_owned());
                }
            }
            let backup_directory = staging.join("backups");
            fs::create_dir(&backup_directory).map_err(|error| error.to_string())?;
            for (index, project) in projects.iter().enumerate() {
                let Some(target) = project.target.clone() else {
                    return Err("Не удалось определить путь установленного ресурса".to_owned());
                };
                let sidecar = metadata_path(&target);
                if project.project_id == root_project_id
                    && !allow_replace
                    && (target.symlink_metadata().is_ok() || sidecar.symlink_metadata().is_ok())
                {
                    return Err(format!("Ресурс {root_project_id} уже установлен в этом экземпляре"));
                }
                self.validate_replaceable_pair(&target, &sidecar, project, project_type)?;
                let (staged_asset, staged_metadata) = &staged_files[index];
                for (suffix, existing) in [("asset", &target), ("metadata", &sidecar)] {
                    if existing.symlink_metadata().is_ok() {
                        let backup = backup_directory.join(format!("{index}-{suffix}"));
                        fs::rename(existing, &backup).map_err(|error| error.to_string())?;
                        backups.push((backup, existing.clone()));
                    }
                }
                fs::rename(staged_asset, &target).map_err(|error| error.to_string())?;
                installed_paths.push(target);
                fs::rename(staged_metadata, &sidecar).map_err(|error| error.to_string())?;
                installed_paths.push(sidecar);
            }
            Ok(())
        })();
        if result.is_err() {
            for path in installed_paths.iter().rev() {
                if is_symlink(path) || path.is_file() {
                    let _ = fs::remove_file(path);
                }
            }
            for (backup, original) in backups.iter().rev() {
                if original.symlink_metadata().is_ok() {
                    let _ = fs::remove_file(original);
                }
                let _ = fs::rename(backup, original);
            }
            for directory in created_directories.iter().rev() {
                let _ = fs::remove_dir(directory);
            }
        }
        result
    }
}

fn resource_paths(instance_dir: &Path, project_type: &str, target_dir: Option<&Path>) -> Result<(PathBuf, PathBuf), String> {
    let directory = project_directory(project_type).ok_or_else(|| format!("Неизвестный тип Modrinth-проекта: {project_type}"))?;
    if project_type == "datapack" {
        let Some(target) = target_dir else {
            return Err("Для датапака нужно выбрать мир".to_owned());
        };
        let root = target.to_path_buf();
        Ok((root.clone(), root.join(directory)))
    } else {
        let root = instance_dir.to_path_buf();
        Ok((root.clone(), root.join(directory)))
    }
}

fn validate_install_paths(install_root: &Path, destination_dir: &Path, project_type: &str) -> Result<(), String> {
    if is_symlink(install_root) {
        return Err("Каталог установки является символьной ссылкой".to_owned());
    }
    if install_root.exists() && !install_root.is_dir() {
        return Err("Каталог установки недоступен".to_owned());
    }
    if project_type == "datapack" && !install_root.is_dir() {
        return Err("Мир для установки датапака не найден".to_owned());
    }
    if is_symlink(destination_dir) {
        return Err("Каталог ресурса является символьной ссылкой".to_owned());
    }
    if destination_dir.exists() && !destination_dir.is_dir() {
        return Err("Каталог ресурса недоступен".to_owned());
    }
    let parent = install_root.parent().unwrap_or(install_root);
    if !install_root.exists() && (is_symlink(parent) || !parent.is_dir()) {
        return Err("Родительский каталог установки недоступен".to_owned());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sidecar_metadata(
    project: &ResolvedProject,
    by_id: &HashMap<String, Value>,
    game_version: &str,
    loader: Option<&str>,
    project_type: &str,
    catalog_id: &str,
    base_url: &str,
) -> Value {
    let dependencies: Vec<Value> = project
        .dependencies
        .iter()
        .map(|dependency_id| {
            json!({
                "project_id": dependency_id,
                "version_id": by_id.get(dependency_id).map(version_id_of).unwrap_or_default(),
                "dependency_type": "required",
            })
        })
        .collect();
    let display_name = project
        .project_metadata
        .get("title")
        .or_else(|| project.version.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| project.project_id.clone());
    json!({
        "source": "Modrinth",
        "catalog": "modrinth",
        "catalog_id": catalog_id,
        "catalog_url": base_url,
        "project_id": project.project_id,
        "project_type": project_type,
        "name": display_name,
        "display_name": display_name,
        "version": project.version.get("version_number").and_then(Value::as_str).unwrap_or(""),
        "version_id": version_id_of(&project.version),
        "game_version": game_version,
        "loader": loader.unwrap_or("vanilla").trim().to_lowercase(),
        "dependencies": dependencies,
        "author": project.project_metadata.get("author").and_then(Value::as_str).unwrap_or(""),
        "icon_url": project
            .project_metadata
            .get("icon_url")
            .or_else(|| project.project_metadata.get("icon"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    })
}

fn write_private_json(path: &Path, value: &Value) -> Result<(), String> {
    let body = serde_json::to_string(value).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| format!("Не удалось записать метаданные ресурса: {error}"))?;
        file.write_all(body.as_bytes())
            .map_err(|error| format!("Не удалось записать метаданные ресурса: {error}"))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, body).map_err(|error| format!("Не удалось записать метаданные ресурса: {error}"))?;
    }
    Ok(())
}

struct StagingDirectory {
    path: PathBuf,
}

impl StagingDirectory {
    fn new(parent: &Path) -> Result<Self, String> {
        let mut last_error = "Не удалось создать временный каталог для ресурсов".to_owned();
        for _ in 0..16 {
            let candidate = parent.join(format!(".trial-mods-{}", random_hex(8)));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { path: candidate }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    last_error = format!("Не удалось создать временный каталог для ресурсов: {error}");
                    break;
                }
            }
        }
        Err(last_error)
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn random_hex(bytes: usize) -> String {
    use std::cell::RefCell;
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

fn flat_list(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect();
    out.sort();
    out
}

fn collect_sidecars(directory: &Path) -> Vec<PathBuf> {
    fn walk(directory: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else { return };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if is_symlink(&path) {
                continue;
            }
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .file_name()
                .map(|name| name.to_string_lossy().ends_with(PROJECT_METADATA_SUFFIX))
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(directory, &mut out);
    out
}

pub(crate) fn read_metadata(path: &Path) -> Map<String, Value> {
    if is_symlink(path) || !path.is_file() {
        return Map::new();
    }
    let Ok(meta) = fs::symlink_metadata(path) else { return Map::new() };
    if meta.len() > MAX_PROJECT_METADATA_BYTES {
        return Map::new();
    }
    let Ok(raw) = fs::read_to_string(path) else { return Map::new() };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value.as_object().cloned().unwrap_or_default(),
        Err(_) => Map::new(),
    }
}

fn metadata_path(target: &Path) -> PathBuf {
    let name = target.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
    target.with_file_name(format!(".{name}{PROJECT_METADATA_SUFFIX}"))
}

fn target_for_sidecar(sidecar: &Path) -> Option<PathBuf> {
    let name = sidecar.file_name()?.to_string_lossy();
    let stripped = name.strip_prefix('.')?;
    let file_name = stripped.strip_suffix(PROJECT_METADATA_SUFFIX)?;
    if file_name.is_empty() {
        return None;
    }
    Some(sidecar.with_file_name(file_name))
}

pub(crate) fn safe_file_name(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    let name = Path::new(&normalized).file_name().map(|inner| inner.to_string_lossy().to_string());
    let valid = matches!(&name, Some(name) if !name.is_empty() && name != "." && name != "..")
        && !normalized.contains('/')
        && name.as_deref() == Some(value);
    if valid {
        Ok(value.to_owned())
    } else {
        Err(format!("Небезопасное имя файла мода: {value}"))
    }
}

fn publisher_hashes(value: Option<&Value>) -> PublisherHashes {
    let mut hashes = PublisherHashes::default();
    let Some(value) = value.filter(|value| value.is_object()) else { return hashes };
    for (algorithm, length) in [("sha1", 40usize), ("sha256", 64), ("sha512", 128)] {
        let digest = value.get(algorithm).and_then(Value::as_str).unwrap_or("").trim();
        let digest = if digest.len() == length && digest.chars().all(|character| character.is_ascii_hexdigit()) {
            digest
        } else {
            ""
        };
        let slot = match algorithm {
            "sha1" => &mut hashes.sha1,
            "sha256" => &mut hashes.sha256,
            _ => &mut hashes.sha512,
        };
        *slot = (!digest.is_empty()).then(|| digest.to_owned());
    }
    hashes
}

fn optional_size(value: Option<&Value>) -> Option<u64> {
    let size = value?.as_i64().or_else(|| value?.as_str().and_then(|text| text.trim().parse::<i64>().ok()))?;
    (size > 0).then_some(size as u64)
}

fn version_id_of(version: &Value) -> String {
    version
        .get("id")
        .or_else(|| version.get("version_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn primary_file(version: &Value, project_id: &str) -> Result<Value, String> {
    let files: Vec<&Value> = version
        .get("files")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|item| item.is_object()).collect())
        .unwrap_or_default();
    let primary = files
        .iter()
        .find(|item| truthy(item.get("primary")))
        .or_else(|| files.first())
        .copied();
    let valid = primary
        .and_then(|item| item.get("url").and_then(Value::as_str))
        .map(|url| !url.trim().is_empty())
        .unwrap_or(false);
    if !valid {
        return Err(format!("У Modrinth-проекта {project_id} нет файла для скачивания"));
    }
    Ok(primary.unwrap().clone())
}

/// null, false, ноль и пустая строка считаются ложью
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

fn search_facets(
    project_type: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<Vec<String>>, String> {
    let category = category.map(str::trim).filter(|value| !value.is_empty());
    if category.is_some() && project_type != "mod" {
        return Err("Категории доступны только для поиска модов".to_owned());
    }
    let category = category.map(|value| value.to_lowercase());
    if category.as_ref().is_some_and(|value| {
        value.len() > 80 || !value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    }) {
        return Err("Некорректная категория Modrinth".to_owned());
    }

    let mut facets: Vec<Vec<String>> = vec![vec![format!("project_type:{project_type}")]];
    if let Some(game_version) = game_version.filter(|value| !value.is_empty()) {
        facets.push(vec![format!("versions:{game_version}")]);
    }
    if let Some(loader) = loader.map(str::trim).filter(|value| !value.is_empty()) {
        let lowered = loader.to_lowercase();
        if lowered != "vanilla" && project_type == "mod" {
            facets.push(vec![format!("categories:{lowered}")]);
        }
    }
    if let Some(category) = category {
        facets.push(vec![format!("categories:{category}")]);
    }
    Ok(facets)
}

/// типы проектов, разрешённые при поиске/просмотре каталога: четыре
fn validate_catalog_project_type(project_type: &str) -> Result<(), String> {
    if project_type == "modpack" || project_directory(project_type).is_some() {
        Ok(())
    } else {
        Err(format!("Неизвестный тип Modrinth-проекта: {project_type}"))
    }
}

fn validate_project_type(project_type: &str) -> Result<(), String> {
    if project_directory(project_type).is_some() {
        Ok(())
    } else {
        Err(format!("Неизвестный тип Modrinth-проекта: {project_type}"))
    }
}

fn validated_project_id(value: &str) -> Result<String, String> {
    let project_id = value.trim();
    let valid = !project_id.is_empty()
        && project_id.chars().count() <= 200
        && project_id != "."
        && project_id != ".."
        && project_id.chars().all(|character| !character.is_control());
    if valid {
        Ok(project_id.to_owned())
    } else {
        Err("Некорректный project ID ресурса".to_owned())
    }
}

fn validate_detail_project_type(value: &Value, project_id: &str, expected: &str) -> Result<(), String> {
    let actual = value.get("project_type").and_then(Value::as_str).unwrap_or("").trim();
    if !actual.is_empty() && actual != expected {
        return Err(format!("Modrinth-проект {project_id} имеет тип {actual}, ожидался {expected}"));
    }
    Ok(())
}

fn matches_project_type(value: &Value, expected: &str) -> bool {
    let actual = value.get("project_type").and_then(Value::as_str).unwrap_or("").trim();
    actual.is_empty() || actual == expected
}

fn version_is_compatible(version: &Value, game_version: &str, loader: &str, project_type: &str) -> bool {
    if !game_version.is_empty() && version.get("game_versions").is_some() {
        let game_versions = version.get("game_versions").and_then(Value::as_array);
        let listed = game_versions.map(|items| items.iter().any(|item| item.as_str() == Some(game_version)));
        if listed != Some(true) {
            return false;
        }
    }
    if project_type == "mod" && !loader.is_empty() && loader != "vanilla" && version.get("loaders").is_some() {
        let loaders = version.get("loaders").and_then(Value::as_array);
        let listed = loaders.map(|items| items.iter().any(|item| item.as_str() == Some(loader)));
        if listed != Some(true) {
            return false;
        }
    }
    true
}

fn within(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok()
}

fn within_resolved(path: &Path, root: &Path) -> bool {
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(path), Ok(root)) => path.strip_prefix(root).is_ok(),
        _ => false,
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

/// кодирует байты, кроме букв, цифр и `.-_~`; пробел становится `%20`
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// кодирует параметры формы, заменяя пробел на `+`
pub(crate) fn form_urlencode(params: &[(&str, String)]) -> String {
    let encode = |out: &mut String, value: &str| {
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b'~' => out.push(byte as char),
                b' ' => out.push('+'),
                other => out.push_str(&format!("%{other:02X}")),
            }
        }
    };
    let mut out = String::new();
    for (index, (key, value)) in params.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        encode(&mut out, key);
        out.push('=');
        encode(&mut out, value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-mods-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn search_facets_include_modrinth_category_without_losing_context() {
        let facets = search_facets("mod", Some("1.20.1"), Some("fabric"), Some("Food")).unwrap();
        assert_eq!(
            facets,
            vec![
                vec!["project_type:mod".to_owned()],
                vec!["versions:1.20.1".to_owned()],
                vec!["categories:fabric".to_owned()],
                vec!["categories:food".to_owned()],
            ]
        );
        assert!(search_facets("resourcepack", None, None, Some("food")).is_err());
        assert!(search_facets("mod", None, None, Some("food:OR project_type:mod")).is_err());
    }

    #[test]
    fn project_id_rejects_bad_input() {
        assert_eq!(validated_project_id("  sodium  ").unwrap(), "sodium");
        assert!(validated_project_id("").is_err());
        assert!(validated_project_id(".").is_err());
        assert!(validated_project_id("..").is_err());
        assert!(validated_project_id(&"x".repeat(201)).is_err());
        assert!(validated_project_id("bad\nid").is_err());
    }

    #[test]
    fn file_name_must_be_plain() {
        assert_eq!(safe_file_name("sodium-0.5.jar").unwrap(), "sodium-0.5.jar");
        assert!(safe_file_name("../evil.jar").is_err());
        assert!(safe_file_name("dir/mod.jar").is_err());
        assert!(safe_file_name("a\\b.jar").is_err());
        assert!(safe_file_name("").is_err());
    }

    #[test]
    fn publisher_hashes_validate_length_and_hex() {
        let sha1 = "0".repeat(40);
        let sha256 = "a".repeat(64);
        let hashes = publisher_hashes(Some(&json!({ "sha1": sha1, "sha256": sha256, "sha512": "short" })));
        assert_eq!(hashes.sha1.as_deref(), Some(sha1.as_str()));
        assert_eq!(hashes.sha256.as_deref(), Some(sha256.as_str()));
        assert!(hashes.sha512.is_none());
        assert!(hashes.any());
        assert!(!publisher_hashes(None).any());
        let bad = publisher_hashes(Some(&json!({ "sha1": "z".repeat(40) })));
        assert!(bad.sha1.is_none());
    }

    #[test]
    fn optional_size_takes_positive_numbers() {
        assert_eq!(optional_size(Some(&json!(1024))), Some(1024));
        assert_eq!(optional_size(Some(&json!("2048"))), Some(2048));
        assert_eq!(optional_size(Some(&json!(0))), None);
        assert_eq!(optional_size(Some(&json!("nope"))), None);
        assert_eq!(optional_size(None), None);
    }

    #[test]
    fn sidecar_name_roundtrip() {
        let target = Path::new("/tmp/instance/mods/sodium.jar");
        let sidecar = metadata_path(target);
        assert_eq!(sidecar.file_name().unwrap(), ".sodium.jar.trial-project.json");
        assert_eq!(target_for_sidecar(&sidecar).as_deref(), Some(target));
        assert!(target_for_sidecar(Path::new("/x/plain.json")).is_none());
        assert!(target_for_sidecar(Path::new("/x/..trial-project.json")).is_none());
    }

    #[test]
    fn url_encoding_matches_python_shapes() {
        assert_eq!(percent_encode("a/b c"), "a%2Fb%20c");
        let params = vec![
            ("limit", "10".to_owned()),
            ("index", "relevance".to_owned()),
            ("facets", "[[\"project_type:mod\"]]".to_owned()),
            ("query", "create plus".to_owned()),
        ];
        assert_eq!(
            form_urlencode(&params),
            "limit=10&index=relevance&facets=%5B%5B%22project_type%3Amod%22%5D%5D&query=create+plus"
        );
    }

    #[test]
    fn version_compatibility_follows_type_and_loader() {
        let version = json!({ "game_versions": ["1.20.4"], "loaders": ["fabric"] });
        assert!(version_is_compatible(&version, "1.20.4", "fabric", "mod"));
        assert!(!version_is_compatible(&version, "1.20.1", "fabric", "mod"));
        assert!(!version_is_compatible(&version, "1.20.4", "quilt", "mod"));
        assert!(version_is_compatible(&version, "1.20.4", "vanilla", "mod"));
        assert!(version_is_compatible(&json!({}), "1.20.4", "fabric", "mod"));
        // фильтр загрузчика применяется только к модам, не к resourcepack
        assert!(version_is_compatible(&version, "1.20.4", "forge", "resourcepack"));
    }

    #[test]
    fn metadata_reading_is_bounded_and_forgiving() {
        let root = temp_root("metadata");
        let sidecar = root.join(".meta.trial-project.json");
        fs::write(&sidecar, json!({ "project_id": "sodium" }).to_string()).unwrap();
        assert_eq!(read_metadata(&sidecar).get("project_id"), Some(&json!("sodium")));
        fs::write(&sidecar, "not json").unwrap();
        assert!(read_metadata(&sidecar).is_empty());
        let oversized = root.join(".big.trial-project.json");
        fs::write(&oversized, vec![b'x'; (MAX_PROJECT_METADATA_BYTES + 1) as usize]).unwrap();
        assert!(read_metadata(&oversized).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn private_json_is_created_exclusive_with_mode() {
        let root = temp_root("private");
        let path = root.join(".sidecar.json");
        write_private_json(&path, &json!({ "a": 1 })).unwrap();
        assert!(write_private_json(&path, &json!({ "a": 2 })).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"a":1}"#);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_directory_cleans_up_on_drop() {
        let root = temp_root("staging");
        let path = {
            let staging = StagingDirectory::new(&root).unwrap();
            assert!(staging.path.is_dir());
            fs::write(staging.path.join("asset-0"), b"data").unwrap();
            staging.path.clone()
        };
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
