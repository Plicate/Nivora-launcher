//! каталог серверных плагинов Modrinth, Spigot, Hangar и пользовательских источников
//! метаданные плагинов хранятся в `.<jar>.trial-plugin.json` рядом с JAR

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::net::{client_from_settings, file_hash, verify_file, HttpClient};
use super::config::ConfigStore;
use super::mods::{form_urlencode, percent_encode, safe_file_name};

pub const PLUGIN_METADATA_SCHEMA: i64 = 1;
pub const PLUGIN_METADATA_MAX_BYTES: u64 = 64 * 1024;
pub const MAX_PLUGIN_DEPENDENCY_PROJECTS: usize = 64;
pub const MAX_PLUGIN_DEPENDENCY_DEPTH: usize = 16;
pub const MAX_PLUGIN_DEPENDENCIES_PER_PROJECT: usize = 100;
pub const UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE: &str =
    "[unverified-download] Каталог не предоставил доверенный хеш плагина. Подтвердите скачивание явно.";
const PLUGIN_SORT_MODES: &[&str] = &["relevance", "downloads", "newest", "updated"];

pub fn default_plugin_catalogs() -> Value {
    json!([
        { "id": "modrinth-plugins", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2", "enabled": true },
        { "id": "spigot", "name": "Spigot", "kind": "spigot", "api_url": "https://api.spiget.org/v2", "enabled": true },
        { "id": "hangar", "name": "Hangar", "kind": "hangar", "api_url": "https://hangar.papermc.io/api/v1", "enabled": true },
    ])
}

pub fn normalize_plugin_catalogs(value: &Value) -> Value {
    let raw: Vec<Value> = match value.as_array() {
        Some(items) => items.clone(),
        None => default_plugin_catalogs()
            .as_array()
            .cloned()
            .unwrap_or_default(),
    };
    let mut result: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in raw.iter().take(20) {
        let Some(fields) = item.as_object() else { continue };
        let catalog_id = fields.get("id").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        let name = fields.get("name").and_then(Value::as_str).unwrap_or("").trim().chars().take(80).collect::<String>();
        let kind = fields.get("kind").and_then(Value::as_str).unwrap_or("custom").trim().to_lowercase();
        let api_url = fields.get("api_url").and_then(Value::as_str).unwrap_or("").trim().trim_end_matches('/').to_owned();
        if catalog_id.is_empty()
            || seen.contains(&catalog_id)
            || name.is_empty()
            || !["modrinth", "spigot", "hangar", "custom"].contains(&kind.as_str())
            || !api_url.starts_with("https://")
        {
            continue;
        }
        seen.push(catalog_id.clone());
        result.push(json!({
            "id": catalog_id.chars().take(64).collect::<String>(),
            "name": name,
            "kind": kind,
            "api_url": api_url.chars().take(512).collect::<String>(),
            "enabled": fields.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        }));
    }
    if result.is_empty() {
        return default_plugin_catalogs();
    }
    Value::Array(result)
}

// ------------------------------------------------------------- результат ----

#[derive(Clone, Default)]
pub(crate) struct CatalogResult {
    source: String,
    project_id: String,
    title: String,
    description: String,
    author: String,
    icon_url: String,
    downloads: i64,
    version: String,
    download_url: String,
    file_name: String,
    sha1: String,
    sha256: String,
    sha512: String,
    size: Option<i64>,
    created_at: String,
    updated_at: String,
    dependencies: Vec<Value>,
    version_id: String,
    downloads_known: bool,
}

impl CatalogResult {
    pub(crate) fn as_dict(&self) -> Map<String, Value> {
        let mut value = Map::new();
        value.insert("source".to_owned(), json!(self.source));
        value.insert("project_id".to_owned(), json!(self.project_id));
        value.insert("title".to_owned(), json!(self.title));
        value.insert("description".to_owned(), json!(self.description));
        value.insert("author".to_owned(), json!(self.author));
        value.insert("icon_url".to_owned(), json!(self.icon_url));
        value.insert("downloads".to_owned(), json!(self.downloads));
        value.insert("version".to_owned(), json!(self.version));
        value.insert("download_url".to_owned(), json!(self.download_url));
        value.insert("file_name".to_owned(), json!(self.file_name));
        value.insert("sha1".to_owned(), json!(self.sha1));
        value.insert("sha256".to_owned(), json!(self.sha256));
        value.insert("sha512".to_owned(), json!(self.sha512));
        value.insert("size".to_owned(), self.size.map(|size| json!(size)).unwrap_or(Value::Null));
        value.insert("created_at".to_owned(), json!(self.created_at));
        value.insert("updated_at".to_owned(), json!(self.updated_at));
        value.insert("dependencies".to_owned(), Value::Array(self.dependencies.clone()));
        value.insert("version_id".to_owned(), json!(self.version_id));
        value
    }
}

// -------------------------------------------------------------- каталог ----

pub(crate) struct PluginCatalog {
    http: Arc<HttpClient>,
    catalog_id: String,
    pub(crate) kind: String,
    base_url: String,
}

impl PluginCatalog {
    pub(crate) fn new(http: Arc<HttpClient>, catalog: &Value) -> Self {
        let kind = catalog.get("kind").and_then(Value::as_str).unwrap_or("custom").trim().to_lowercase();
        let base_url = catalog.get("api_url").and_then(Value::as_str).unwrap_or("").trim_end_matches('/').to_owned();
        let catalog_id = catalog.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
        Self { http, kind, base_url, catalog_id }
    }

    pub(crate) fn search(
        &self,
        query: &str,
        core: &str,
        version: &str,
        limit: i64,
        sort_by: &str,
    ) -> Result<Vec<CatalogResult>, String> {
        let limit = limit.clamp(1, 30);
        let sort = validate_sort(sort_by)?;
        let query = query.trim();
        let mut results: Vec<CatalogResult> = Vec::new();
        if self.kind == "modrinth" {
            let mut facets: Vec<Vec<String>> = vec![vec!["project_type:plugin".to_owned()]];
            if !version.is_empty() {
                facets.push(vec![format!("versions:{version}")]);
            }
            if !core.is_empty() && core != "custom" && core != "bukkit" {
                facets.push(vec![format!("categories:{core}")]);
            }
            let mut params: Vec<(&str, String)> = vec![
                ("limit", limit.to_string()),
                ("facets", serde_json::to_string(&facets).map_err(|error| error.to_string())?),
                ("index", sort.to_owned()),
            ];
            if !query.is_empty() {
                params.push(("query", query.to_owned()));
            }
            let url = format!("{}/search?{}", self.base_url, form_urlencode(&params));
            let value = self.http.get_json(&url, None)?;
            let hits = value.get("hits").and_then(Value::as_array).cloned().unwrap_or_default();
            results = hits.iter().filter_map(|item| item.as_object()).map(modrinth_result).collect();
            sort_results(&mut results, &sort);
            results.truncate(limit as usize);
            return Ok(results);
        }
        if self.kind == "spigot" {
            let mut params: Vec<(&str, String)> = vec![("size", limit.to_string())];
            let provider_sort = provider_sort(&self.kind, &sort);
            if !provider_sort.is_empty() {
                params.push(("sort", provider_sort));
            }
            let endpoint = format!(
                "{}/search/resources/{}?{}",
                self.base_url,
                percent_encode(if query.is_empty() { "plugin" } else { query }),
                form_urlencode(&params)
            );
            let value = self.http.get_json(&endpoint, None)?;
            if let Some(rows) = value.as_array() {
                results = rows.iter().filter_map(|item| item.as_object()).map(spigot_result).collect();
            }
            sort_results(&mut results, &sort);
            results.truncate(limit as usize);
            return Ok(results);
        }
        if self.kind == "hangar" {
            let mut params: Vec<(&str, String)> = vec![("query", query.to_owned()), ("limit", limit.to_string())];
            let provider_sort = provider_sort(&self.kind, &sort);
            if !provider_sort.is_empty() {
                params.push(("sort", provider_sort));
            }
            let url = format!("{}/projects?{}", self.base_url, form_urlencode(&params));
            let value = self.http.get_json(&url, None)?;
            let rows = value
                .get("result")
                .or_else(|| value.get("projects"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| value.as_array().cloned().unwrap_or_default());
            results = rows.iter().filter_map(|item| item.as_object()).map(hangar_result).collect();
            sort_results(&mut results, &sort);
            results.truncate(limit as usize);
            return Ok(results);
        }
        let params: Vec<(&str, String)> = vec![
            ("query", query.to_owned()),
            ("limit", limit.to_string()),
            ("core", core.to_owned()),
            ("version", version.to_owned()),
        ];
        let url = format!("{}/?{}", self.base_url, form_urlencode(&params));
        let value = self.http.get_json(&url, None)?;
        let rows = value
            .get("results")
            .cloned()
            .unwrap_or_else(|| value.clone());
        if let Some(rows) = rows.as_array() {
            results = rows
                .iter()
                .filter_map(|item| item.as_object())
                .map(|item| custom_result(&self.kind, item))
                .collect();
        }
        sort_results(&mut results, &sort);
        results.truncate(limit as usize);
        Ok(results)
    }

    pub(crate) fn details(&self, project_id: &str, core: &str, version: &str) -> Result<Value, String> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err("Укажите идентификатор плагина".to_owned());
        }
        let mut result: CatalogResult;
        let candidate: Option<Map<String, Value>>;
        if self.kind == "modrinth" {
            let details = self.modrinth_project_details(project_id)?;
            result = modrinth_result(&details);
            candidate = self.resolve_candidate(project_id, core, version, Some(&details), "")?;
        } else if self.kind == "spigot" {
            let url = format!("{}/resources/{}", self.base_url, percent_encode(project_id));
            let value = self.http.get_json(&url, None)?;
            let Some(fields) = value.as_object() else {
                return Err("Spigot не вернул сведения о плагине".to_owned());
            };
            result = spigot_result(fields);
            candidate = Some(self.spigot_candidate(project_id, fields, core, version));
        } else if self.kind == "hangar" {
            let url = format!("{}/projects/{}", self.base_url, hangar_project_path(project_id)?);
            let value = self.http.get_json(&url, None)?;
            result = value
                .as_object()
                .map(|fields| hangar_result(fields))
                .unwrap_or_else(|| empty_result(&self.kind, project_id));
            candidate = self.resolve_candidate(project_id, core, version, None, "")?;
        } else {
            let matches = self.search(project_id, core, version, 30, "relevance")?;
            let mut filtered: Vec<CatalogResult> = matches
                .into_iter()
                .filter(|item| item.project_id == project_id)
                .collect();
            sort_results(&mut filtered, "updated");
            result = filtered
                .into_iter()
                .next()
                .ok_or_else(|| "Каталог не вернул сведения о плагине".to_owned())?;
            let compatible = !core.is_empty() && !version.is_empty();
            candidate = Some(candidate_from_result(&result, compatible));
        }
        if let Some(candidate) = candidate {
            apply_candidate(&mut result, &candidate);
        }
        Ok(Value::Object(result.as_dict()))
    }

    pub(crate) fn install(
        &self,
        project_id: &str,
        target: &Path,
        version: &str,
        core: &str,
        confirm_unverified: bool,
    ) -> Result<Value, String> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err("Укажите идентификатор плагина".to_owned());
        }
        let candidate = self
            .resolve_candidate(project_id, core, version, None, "")?
            .ok_or_else(|| "Каталог не вернул файл плагина".to_owned())?;
        if candidate.get("download_url").and_then(Value::as_str).unwrap_or_default().is_empty() {
            return Err("Каталог не вернул файл плагина".to_owned());
        }
        if !candidate.get("compatible").and_then(Value::as_bool).unwrap_or(false) {
            return Err("Каталог не подтвердил совместимость плагина с выбранным ядром и версией Minecraft".to_owned());
        }
        if !confirm_unverified && !candidate_has_trusted_hash(&candidate) {
            return Err(UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE.to_owned());
        }
        let candidates = self.resolve_install_plan(project_id, &candidate, core, version)?;
        if !confirm_unverified && candidates.iter().any(|item| !candidate_has_trusted_hash(item)) {
            return Err(UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE.to_owned());
        }
        let target = plugin_directory(target, true)?;
        self.install_candidate_plan(&candidates, project_id, &target, core, version, confirm_unverified)
    }

    fn resolve_install_plan(
        &self,
        project_id: &str,
        root_candidate: &Map<String, Value>,
        core: &str,
        version: &str,
    ) -> Result<Vec<Map<String, Value>>, String> {
        let mut selected: Vec<(String, Map<String, Value>)> = Vec::new();
        self.visit_install_candidate(project_id, "", 0, Some(root_candidate.clone()), core, version, &mut selected)?;
        Ok(selected.into_iter().map(|(_, value)| value).collect())
    }

    fn visit_install_candidate(
        &self,
        current_id: &str,
        pinned_version_id: &str,
        depth: usize,
        candidate: Option<Map<String, Value>>,
        core: &str,
        version: &str,
        selected: &mut Vec<(String, Map<String, Value>)>,
    ) -> Result<(), String> {
        let current_id = plugin_project_id(current_id)?;
        let pinned_version_id = plugin_version_id(pinned_version_id)?;
        if let Some(position) = selected.iter().position(|(id, _)| *id == current_id) {
            let selected_version_id = selected[position].1.get("version_id").and_then(Value::as_str).unwrap_or_default();
            if !pinned_version_id.is_empty() && pinned_version_id != selected_version_id {
                return Err(format!("Зависимости требуют разные версии проекта {current_id}"));
            }
            return Ok(());
        }
        if depth > MAX_PLUGIN_DEPENDENCY_DEPTH {
            return Err("Цепочка зависимостей серверных плагинов слишком глубокая".to_owned());
        }
        if selected.len() >= MAX_PLUGIN_DEPENDENCY_PROJECTS {
            return Err("В дереве зависимостей серверных плагинов слишком много проектов".to_owned());
        }
        let current = match candidate {
            Some(value) => value,
            None => self
                .resolve_candidate(&current_id, core, version, None, &pinned_version_id)?
                .ok_or_else(|| format!("Каталог не нашёл обязательную зависимость {current_id}"))?,
        };
        let returned_id = current.get("project_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        if returned_id != current_id {
            return Err(format!("Каталог вернул сведения о другом проекте вместо {current_id}"));
        }
        if !current.get("compatible").and_then(Value::as_bool).unwrap_or(false) {
            return Err(format!("Каталог не подтвердил совместимость обязательной зависимости {current_id}"));
        }
        if current.get("download_url").and_then(Value::as_str).unwrap_or_default().is_empty() {
            return Err(format!("Каталог не вернул файл обязательной зависимости {current_id}"));
        }
        let selected_version_id = current.get("version_id").and_then(Value::as_str).unwrap_or_default().to_owned();
        if !pinned_version_id.is_empty() && pinned_version_id != selected_version_id {
            return Err(format!("Каталог не разрешил закреплённую версию {pinned_version_id} проекта {current_id}"));
        }
        for (dependency_id, dependency_version_id) in self.required_dependencies(&current)? {
            self.visit_install_candidate(&dependency_id, &dependency_version_id, depth + 1, None, core, version, selected)?;
        }
        selected.push((current_id, current));
        Ok(())
    }

    fn required_dependencies(&self, candidate: &Map<String, Value>) -> Result<Vec<(String, String)>, String> {
        let empty = Vec::new();
        let raw = candidate.get("dependencies").and_then(Value::as_array).unwrap_or(&empty);
        if raw.len() > MAX_PLUGIN_DEPENDENCIES_PER_PROJECT {
            return Err("У серверного плагина слишком много зависимостей".to_owned());
        }
        let mut result: Vec<(String, String)> = Vec::new();
        for item in raw {
            let Some(fields) = item.as_object() else { continue };
            if fields.get("optional").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let dependency_type = fields.get("dependency_type").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
            let required = if !dependency_type.is_empty() {
                dependency_type == "required"
            } else if fields.contains_key("required") {
                fields.get("required").and_then(Value::as_bool).unwrap_or(false)
            } else if fields.contains_key("optional") {
                fields.get("optional").and_then(Value::as_bool) == Some(false)
            } else {
                false
            };
            if !required {
                continue;
            }
            let mut dependency_id = fields.get("project_id").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            let pinned_version_id = plugin_version_id(fields.get("version_id").and_then(Value::as_str).unwrap_or(""))?;
            if dependency_id.is_empty() && self.kind == "modrinth" && !pinned_version_id.is_empty() {
                dependency_id = self.modrinth_project_id_for_version(&pinned_version_id)?;
            }
            if dependency_id.is_empty() {
                continue;
            }
            result.push((plugin_project_id(&dependency_id)?, pinned_version_id));
        }
        Ok(result)
    }

    fn modrinth_project_id_for_version(&self, version_id: &str) -> Result<String, String> {
        let url = format!("{}/version/{}", self.base_url, percent_encode(version_id));
        let value = self.http.get_json(&url, None)?;
        let Some(fields) = value.as_object() else {
            return Err(format!("Не удалось определить проект для версии зависимости {version_id}"));
        };
        let returned_version_id = fields
            .get("id")
            .or_else(|| fields.get("version_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !returned_version_id.is_empty() && returned_version_id != version_id {
            return Err("Modrinth вернул другую версию зависимости".to_owned());
        }
        let project_id = fields.get("project_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        if project_id.is_empty() {
            return Err(format!("Не удалось определить проект для версии зависимости {version_id}"));
        }
        plugin_project_id(&project_id)
    }

    fn install_candidate_plan(
        &self,
        candidates: &[Map<String, Value>],
        root_project_id: &str,
        target: &Path,
        core: &str,
        version: &str,
        confirm_unverified: bool,
    ) -> Result<Value, String> {
        let staging = target.join(format!(".trial-plugin-install-{}", super::mods::random_hex(8)));
        std::fs::create_dir(&staging).map_err(|error| format!("Не удалось создать каталог установки: {error}"))?;
        let mut cleanup: Vec<PathBuf> = Vec::new();
        let result = (|| -> Result<Value, String> {
            let mut staged: Vec<(PathBuf, PathBuf, Map<String, Value>)> = Vec::new();
            let mut reserved_names: Vec<String> = Vec::new();
            for candidate in candidates {
                let project_id = candidate.get("project_id").and_then(Value::as_str).unwrap_or("plugin");
                let raw_name = candidate
                    .get("file_name")
                    .and_then(Value::as_str)
                    .map(|value| value.to_owned())
                    .unwrap_or_else(|| format!("{project_id}.jar"));
                let file_name = safe_jar_file_name(&raw_name)?;
                let destination = unique_plugin_file(target, &file_name, &reserved_names)?;
                reserved_names.push(destination.file_name().unwrap_or_default().to_string_lossy().to_string());
                let staged_jar = staging.join(destination.file_name().unwrap_or_default());
                let downloaded = self.download_candidate(candidate, target, &staged_jar, core, version, confirm_unverified, false, None)?;
                cleanup.push(staged_jar.clone());
                staged.push((staged_jar, destination, downloaded));
            }
            let root_position = staged
                .iter()
                .position(|(_, _, result)| result.get("project_id").and_then(Value::as_str) == Some(root_project_id))
                .ok_or_else(|| "Не удалось определить установленный основной плагин".to_owned())?;
            // публикуем JAR и метаданные вместе либо полностью откатываем операцию
            let mut committed: Vec<PathBuf> = Vec::new();
            let publish = (|| -> Result<(), String> {
                plugin_directory(target, false)?;
                for (staged_jar, destination, _) in &staged {
                    let staged_metadata = plugin_metadata_path(staged_jar);
                    let destination_metadata = plugin_metadata_path(destination);
                    for (source, final_path) in [(staged_metadata, destination_metadata), (staged_jar.to_path_buf(), destination.to_path_buf())] {
                        match std::fs::hard_link(&source, &final_path) {
                            Ok(()) => {
                                committed.push(final_path);
                                let _ = std::fs::remove_file(source);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                return Err("Файл плагина был создан во время установки".to_owned());
                            }
                            Err(error) => return Err(format!("Не удалось опубликовать плагин: {error}")),
                        }
                    }
                }
                Ok(())
            })();
            if let Err(error) = publish {
                let mut rollback_errors = 0;
                for path in committed.iter().rev() {
                    if std::fs::symlink_metadata(path).is_ok() && std::fs::remove_file(path).is_err() {
                        rollback_errors += 1;
                    }
                }
                if rollback_errors > 0 {
                    return Err("Не удалось полностью отменить установку серверных плагинов".to_owned());
                }
                return Err(error);
            }
            let (_root_staged_jar, destination, mut root_result) = staged.swap_remove(root_position);
            root_result.insert("path".to_owned(), json!(destination));
            Ok(Value::Object(root_result))
        })();
        for path in cleanup.iter().chain(std::iter::once(&staging)) {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
        result
    }

    pub(crate) fn installed(&self, target: &Path) -> Result<Vec<Map<String, Value>>, String> {
        Ok(installed_plugin_metadata_index(target)?
            .into_iter()
            .filter(|entry| self.matches_catalog(entry))
            .collect())
    }

    fn matches_catalog(&self, entry: &Map<String, Value>) -> bool {
        let catalog_id = self.catalog_id.as_str();
        entry.get("source").and_then(Value::as_str) == Some(self.kind.as_str())
            && entry.get("api_url").and_then(Value::as_str) == Some(self.base_url.as_str())
            && (catalog_id.is_empty() || entry.get("catalog_id").and_then(Value::as_str) == Some(catalog_id))
    }

    pub(crate) fn check_updates(&self, target: &Path, core: &str, version: &str) -> Result<Vec<Value>, String> {
        validate_update_context(core, version)?;
        let mut updates: Vec<Value> = Vec::new();
        for entry in self.installed(target)? {
            let Some(candidate) = self.resolve_candidate(
                entry.get("project_id").and_then(Value::as_str).unwrap_or_default(),
                core,
                version,
                None,
                "",
            )?
            else {
                continue;
            };
            if !candidate.get("compatible").and_then(Value::as_bool).unwrap_or(false) || !is_update(&entry, &candidate) {
                continue;
            }
            updates.push(json!({
                "source": self.kind,
                "catalog_id": entry.get("catalog_id"),
                "project_id": entry.get("project_id"),
                "file_name": entry.get("file_name"),
                "path": entry.get("path"),
                "current_version": entry.get("version").and_then(Value::as_str).unwrap_or_default(),
                "current_version_id": entry.get("version_id").and_then(Value::as_str).unwrap_or_default(),
                "version": candidate.get("version").and_then(Value::as_str).unwrap_or_default(),
                "version_id": candidate.get("version_id").and_then(Value::as_str).unwrap_or_default(),
                "updated_at": candidate.get("updated_at").and_then(Value::as_str).unwrap_or_default(),
                "trusted_hash": candidate.get("trusted_hash").and_then(Value::as_bool).unwrap_or(false),
                "dependencies": candidate.get("dependencies").cloned().unwrap_or_else(|| json!([])),
            }));
        }
        Ok(updates)
    }

    pub(crate) fn update_installed(
        &self,
        target: &Path,
        file_name: &str,
        core: &str,
        version: &str,
        confirm_unverified: bool,
    ) -> Result<Value, String> {
        validate_update_context(core, version)?;
        let file_name = safe_jar_file_name(file_name)?;
        let entries = self.installed(target)?;
        let entry = entries
            .iter()
            .find(|item| item.get("file_name").and_then(Value::as_str) == Some(file_name.as_str()))
            .ok_or_else(|| "Можно обновлять только JAR, установленный из этого каталога".to_owned())?;
        let candidate = self.resolve_candidate(
            entry.get("project_id").and_then(Value::as_str).unwrap_or_default(),
            core,
            version,
            None,
            "",
        )?
        .ok_or_else(|| "Каталог не подтвердил совместимую версию плагина для этого ядра и Minecraft".to_owned())?;
        if !candidate.get("compatible").and_then(Value::as_bool).unwrap_or(false)
            || candidate.get("download_url").and_then(Value::as_str).unwrap_or_default().is_empty()
        {
            return Err("Каталог не подтвердил совместимую версию плагина для этого ядра и Minecraft".to_owned());
        }
        if !is_update(entry, &candidate) {
            let mut value = entry.clone();
            value.insert("updated".to_owned(), json!(false));
            value.insert("current_version".to_owned(), json!(entry.get("version").and_then(Value::as_str).unwrap_or_default()));
            value.insert("latest_version".to_owned(), json!(candidate.get("version").and_then(Value::as_str).unwrap_or_default()));
            return Ok(Value::Object(value));
        }
        let directory = plugin_directory(target, false)?;
        let destination = directory.join(&file_name);
        if destination.symlink_metadata().is_ok_and(|meta| !meta.is_file()) || !destination.is_file() {
            return Err("Установленный JAR отсутствует или является символьной ссылкой".to_owned());
        }
        let mut result = self.download_candidate(&candidate, &directory, &destination, core, version, confirm_unverified, true, Some(entry))?;
        result.insert("updated".to_owned(), json!(true));
        Ok(Value::Object(result))
    }

    #[allow(clippy::too_many_arguments)]
    fn download_candidate(
        &self,
        candidate: &Map<String, Value>,
        target: &Path,
        destination: &Path,
        core: &str,
        version: &str,
        confirm_unverified: bool,
        replace: bool,
        expected_entry: Option<&Map<String, Value>>,
    ) -> Result<Map<String, Value>, String> {
        let hashes = candidate_hashes(candidate);
        let trusted_hash = hashes.iter().any(|value| !value.is_none());
        safe_jar_file_name(
            candidate
                .get("file_name")
                .and_then(Value::as_str)
                .map(|value| value.to_owned())
                .unwrap_or_else(|| destination.file_name().unwrap_or_default().to_string_lossy().to_string())
                .as_str(),
        )?;
        if !trusted_hash && !confirm_unverified {
            return Err(UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE.to_owned());
        }
        let target = plugin_directory(target, true)?;
        if destination.parent() != Some(&target) {
            return Err("Путь JAR плагина находится вне каталога plugins".to_owned());
        }
        if destination.symlink_metadata().is_ok_and(|meta| !meta.is_file()) {
            return Err("Путь JAR плагина небезопасен".to_owned());
        }
        if !replace && destination.symlink_metadata().is_ok() {
            return Err("Файл плагина уже существует".to_owned());
        }
        let url = candidate.get("download_url").and_then(Value::as_str).unwrap_or_default().to_owned();
        if url.is_empty() {
            return Err("Каталог не вернул ссылку для скачивания плагина".to_owned());
        }
        let size = optional_int(candidate.get("size"));
        let temporary = target.join(format!(".trial-plugin-{}.jar", super::mods::random_hex(8)));
        let outcome = (|| -> Result<Map<String, Value>, String> {
            self.http.download(
                &url,
                &temporary,
                hashes[0].as_deref(),
                hashes[1].as_deref(),
                hashes[2].as_deref(),
                size.map(|value| value as u64),
                None,
                None,
                None,
            )?;
            if !temporary.is_file() {
                return Err("Каталог не создал корректный временный JAR".to_owned());
            }
            if !verify_file(&temporary, hashes[0].as_deref(), hashes[1].as_deref(), hashes[2].as_deref(), size.map(|value| value as u64)) {
                return Err("Проверка хеша или размера JAR не пройдена".to_owned());
            }
            let digest = file_hash(&temporary, "sha256")?;
            if replace {
                let current = installed_plugin_metadata_index(&target)?
                    .into_iter()
                    .find(|item| item.get("file_name").and_then(Value::as_str) == destination.file_name().map(|value| value.to_string_lossy().to_string()).as_deref());
                let expected = expected_entry.ok_or_else(|| "Установленный JAR изменился во время загрузки; обновление отменено".to_owned())?;
                let matches = current.as_ref().is_some_and(|current| {
                    current.get("sha256") == expected.get("sha256")
                        && current.get("project_id") == expected.get("project_id")
                        && current.get("catalog_id") == expected.get("catalog_id")
                });
                if !matches {
                    return Err("Установленный JAR изменился во время загрузки; обновление отменено".to_owned());
                }
            } else if destination.symlink_metadata().is_ok() {
                return Err("Файл плагина был создан во время загрузки".to_owned());
            }
            std::fs::rename(&temporary, destination).map_err(|error| format!("Не удалось опубликовать плагин: {error}"))?;
            let mut result = Map::new();
            result.insert("source".to_owned(), json!(self.kind));
            result.insert("catalog_id".to_owned(), json!(self.catalog_id));
            result.insert("project_id".to_owned(), json!(candidate.get("project_id").and_then(Value::as_str).unwrap_or_default()));
            result.insert("file_name".to_owned(), json!(destination.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default()));
            result.insert("version".to_owned(), json!(candidate.get("version").and_then(Value::as_str).unwrap_or_default()));
            result.insert("version_id".to_owned(), json!(candidate.get("version_id").and_then(Value::as_str).unwrap_or_default()));
            result.insert("path".to_owned(), json!(destination));
            result.insert("sha256".to_owned(), json!(digest));
            result.insert("trusted_hash".to_owned(), json!(trusted_hash));
            result.insert("created_at".to_owned(), json!(candidate.get("created_at").and_then(Value::as_str).unwrap_or_default()));
            result.insert("updated_at".to_owned(), json!(candidate.get("updated_at").and_then(Value::as_str).unwrap_or_default()));
            result.insert("dependencies".to_owned(), candidate.get("dependencies").cloned().unwrap_or_else(|| json!([])));
            let mut metadata = Map::new();
            metadata.insert("schema".to_owned(), json!(PLUGIN_METADATA_SCHEMA));
            metadata.insert("managed_by".to_owned(), json!("trial-launcher"));
            metadata.insert("catalog_id".to_owned(), json!(self.catalog_id));
            metadata.insert("source".to_owned(), json!(self.kind));
            metadata.insert("api_url".to_owned(), json!(self.base_url));
            metadata.insert("project_id".to_owned(), result["project_id"].clone());
            metadata.insert("file_name".to_owned(), result["file_name"].clone());
            metadata.insert("version".to_owned(), result["version"].clone());
            metadata.insert("version_id".to_owned(), json!(candidate.get("version_id").and_then(Value::as_str).unwrap_or_default()));
            metadata.insert("core".to_owned(), json!(core));
            metadata.insert("minecraft_version".to_owned(), json!(version));
            metadata.insert("sha256".to_owned(), json!(digest));
            metadata.insert("trusted_hash".to_owned(), json!(trusted_hash));
            metadata.insert("size".to_owned(), json!(destination.metadata().map(|meta| meta.len()).unwrap_or_default()));
            metadata.insert("created_at".to_owned(), result["created_at"].clone());
            metadata.insert("updated_at".to_owned(), result["updated_at"].clone());
            metadata.insert("installed_at".to_owned(), json!(now_iso()));
            metadata.insert("dependencies".to_owned(), Value::Array(normalize_dependencies(candidate.get("dependencies"))));
            write_plugin_metadata(destination, &metadata)?;
            Ok(result)
        })();
        if temporary.symlink_metadata().is_ok() {
            let _ = std::fs::remove_file(&temporary);
        }
        outcome
    }

    fn resolve_candidate(
        &self,
        project_id: &str,
        core: &str,
        version: &str,
        project_details: Option<&Map<String, Value>>,
        pinned_version_id: &str,
    ) -> Result<Option<Map<String, Value>>, String> {
        if self.kind == "modrinth" {
            let details = match project_details {
                Some(value) => value.clone(),
                None => self.modrinth_project_details(project_id)?,
            };
            if !pinned_version_id.is_empty() {
                let url = format!("{}/version/{}", self.base_url, percent_encode(pinned_version_id));
                let value = self.http.get_json(&url, None)?;
                let Some(item) = value.as_object() else {
                    return Ok(None);
                };
                let returned_version_id = item
                    .get("id")
                    .or_else(|| item.get("version_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if returned_version_id != pinned_version_id {
                    return Err("Modrinth вернул другую версию обязательной зависимости".to_owned());
                }
                let returned_project_id = item.get("project_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
                if returned_project_id != project_id {
                    return Err(format!("Modrinth вернул версию другого проекта вместо {project_id}"));
                }
                let mut candidate = modrinth_candidate(project_id, &details, item, core, version);
                candidate.insert("compatible".to_owned(), json!(modrinth_plugin_version_compatible(item, core, version)));
                return Ok(Some(candidate));
            }
            let versions = self.modrinth_versions(project_id, core, version)?;
            if versions.is_empty() {
                return Ok(None);
            }
            let item = latest_item(&versions, &["date_published", "published_at"]);
            return Ok(Some(modrinth_candidate(project_id, &details, &item, core, version)));
        }
        if self.kind == "spigot" {
            let url = format!("{}/resources/{}", self.base_url, percent_encode(project_id));
            let value = self.http.get_json(&url, None)?;
            let fields = value.as_object().cloned().unwrap_or_default();
            return Ok(Some(self.spigot_candidate(project_id, &fields, core, version)));
        }
        if self.kind == "hangar" {
            let project_path = hangar_project_path(project_id)?;
            let url = format!("{}/projects/{}/versions?{}", self.base_url, project_path, form_urlencode(&[("limit", "100".to_owned())]));
            let value = self.http.get_json(&url, None)?;
            let rows = provider_rows(&value, &["result", "versions"]);
            let valid_rows: Vec<Map<String, Value>> = rows.iter().filter_map(|item| item.as_object()).cloned().collect();
            if valid_rows.is_empty() {
                return Ok(None);
            }
            let compatible_rows: Vec<Map<String, Value>> = valid_rows
                .iter()
                .filter(|item| hangar_compatible(item, core, version))
                .cloned()
                .collect();
            let item = if compatible_rows.is_empty() { &valid_rows } else { &compatible_rows };
            let item = latest_item(item, &["createdAt", "datePublished", "created_at"]);
            return Ok(Some(hangar_candidate(&self.base_url, project_id, &item, !compatible_rows.is_empty())));
        }
        let matches = self.search(project_id, core, version, 30, "relevance")?;
        let mut filtered: Vec<CatalogResult> = matches
            .into_iter()
            .filter(|item| item.project_id == project_id)
            .collect();
        sort_results(&mut filtered, "updated");
        let result = filtered.into_iter().next();
        Ok(result.map(|result| candidate_from_result(&result, !core.is_empty() && !version.is_empty())))
    }

    fn modrinth_versions(&self, project_id: &str, core: &str, version: &str) -> Result<Vec<Map<String, Value>>, String> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if !version.is_empty() {
            params.push(("game_versions", format!("[\"{version}\"]")));
        }
        if !core.is_empty() && core != "custom" && core != "bukkit" {
            params.push(("loaders", format!("[\"{core}\"]")));
        }
        let suffix = if params.is_empty() { String::new() } else { format!("?{}", form_urlencode(&params)) };
        let url = format!("{}/project/{}/version{suffix}", self.base_url, percent_encode(project_id));
        let value = self.http.get_json(&url, None)?;
        let rows: Vec<Map<String, Value>> = value
            .as_array()
            .map(|items| items.iter().filter_map(|item| item.as_object().cloned()).collect())
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|item| modrinth_plugin_version_compatible(item, core, version))
            .collect())
    }

    fn modrinth_project_details(&self, project_id: &str) -> Result<Map<String, Value>, String> {
        let url = format!("{}/project/{}", self.base_url, percent_encode(project_id));
        let value = self.http.get_json(&url, None)?;
        let Some(fields) = value.as_object() else {
            return Err("Modrinth не вернул сведения о проекте".to_owned());
        };
        if fields.get("project_type").and_then(Value::as_str).unwrap_or("").trim() != "plugin" {
            let actual = fields
                .get("project_type")
                .and_then(Value::as_str)
                .unwrap_or("неизвестен")
                .trim()
                .to_owned();
            let actual = if actual.is_empty() { "неизвестен".to_owned() } else { actual };
            return Err(format!("Modrinth-проект {project_id} имеет тип {actual}, ожидался plugin"));
        }
        let returned_id = fields
            .get("id")
            .or_else(|| fields.get("project_id"))
            .and_then(Value::as_str)
            .unwrap_or(project_id)
            .trim()
            .to_owned();
        if returned_id != project_id {
            return Err("Modrinth вернул сведения о другом проекте".to_owned());
        }
        Ok(fields.clone())
    }

    fn spigot_candidate(&self, project_id: &str, item: &Map<String, Value>, core: &str, version: &str) -> Map<String, Value> {
        let result = spigot_result(item);
        let update_date = if result.updated_at.is_empty() {
            item.get("updateDate").and_then(Value::as_str).unwrap_or_default().to_owned()
        } else {
            result.updated_at.clone()
        };
        let provider_version = item
            .get("version")
            .and_then(Value::as_str)
            .or_else(|| item.get("tag").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        let hashes = provider_hashes(item);
        let mut candidate = candidate_from_result(&result, spigot_compatible(item, core, version));
        candidate.insert("version".to_owned(), json!(if provider_version.is_empty() { update_date.clone() } else { provider_version }));
        candidate.insert("version_id".to_owned(), json!(update_date));
        candidate.insert(
            "download_url".to_owned(),
            json!(format!("{}/resources/{}/download", self.base_url, percent_encode(project_id))),
        );
        let file_name = safe_file_name(project_id).unwrap_or_else(|_| format!("{project_id}.jar"));
        candidate.insert("file_name".to_owned(), json!(format!("{file_name}.jar")));
        candidate.insert("sha1".to_owned(), json!(hashes.0));
        candidate.insert("sha256".to_owned(), json!(hashes.1));
        candidate.insert("sha512".to_owned(), json!(hashes.2));
        candidate.insert("size".to_owned(), optional_int(item.get("size")).map(|size| json!(size)).unwrap_or(Value::Null));
        candidate.insert(
            "trusted_hash".to_owned(),
            json!(!hashes.0.is_empty() || !hashes.1.is_empty() || !hashes.2.is_empty()),
        );
        candidate
    }
}

// --------------------------------------------------------- результаты ----

fn custom_result(kind: &str, item: &Map<String, Value>) -> CatalogResult {
    CatalogResult {
        source: kind.to_owned(),
        project_id: str_field(item, &["project_id", "id", "slug"]),
        title: non_empty(str_field(item, &["title", "name"]), "Plugin"),
        description: str_field(item, &["description"]),
        author: str_field(item, &["author"]),
        icon_url: str_field(item, &["icon_url", "icon"]),
        downloads: int_value(item.get("downloads")),
        version: str_field(item, &["version"]),
        download_url: str_field(item, &["download_url"]),
        file_name: str_field(item, &["file_name"]),
        sha1: str_field(item, &["sha1"]),
        sha256: str_field(item, &["sha256"]),
        sha512: str_field(item, &["sha512"]),
        size: optional_int(item.get("size")),
        created_at: date_value(first_of(item, &["created_at", "createdAt", "date_created"])),
        updated_at: date_value(first_of(item, &["updated_at", "updatedAt", "date_modified", "updateDate"])),
        dependencies: normalize_dependencies(first_of(item, &["dependencies", "plugin_dependencies"])),
        version_id: str_field(item, &["version_id"]),
        downloads_known: item.contains_key("downloads") && !item.get("downloads").unwrap_or(&Value::Null).is_null(),
    }
}

fn modrinth_result(item: &Map<String, Value>) -> CatalogResult {
    CatalogResult {
        source: "modrinth".to_owned(),
        project_id: str_field(item, &["project_id", "id"]),
        title: non_empty(str_field(item, &["title", "name"]), "Plugin"),
        description: str_field(item, &["description"]),
        author: str_field(item, &["author"]),
        icon_url: str_field(item, &["icon_url", "icon"]),
        downloads: int_value(item.get("downloads")),
        version: str_field(item, &["version"]),
        download_url: str_field(item, &["download_url"]),
        file_name: str_field(item, &["file_name"]),
        sha1: str_field(item, &["sha1"]),
        sha256: str_field(item, &["sha256"]),
        sha512: str_field(item, &["sha512"]),
        size: optional_int(item.get("size")),
        created_at: date_value(first_of(item, &["date_created", "created_at", "createdAt"])),
        updated_at: date_value(first_of(item, &["date_modified", "updated_at", "updatedAt"])),
        dependencies: normalize_dependencies(first_of(item, &["dependencies", "plugin_dependencies"])),
        version_id: str_field(item, &["version_id"]),
        downloads_known: item.contains_key("downloads") && !item.get("downloads").unwrap_or(&Value::Null).is_null(),
    }
}

fn spigot_result(item: &Map<String, Value>) -> CatalogResult {
    let mut author = str_field(item, &["author"]);
    if let Some(fields) = item.get("author").and_then(Value::as_object) {
        author = fields.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
    }
    CatalogResult {
        source: "spigot".to_owned(),
        project_id: str_field(item, &["id"]),
        title: non_empty(str_field(item, &["name"]), "Spigot resource"),
        description: str_field(item, &["tagged", "description"]),
        author,
        icon_url: str_field(item, &["icon_url", "icon"]),
        downloads: int_value(item.get("downloads")),
        version: str_field(item, &["version"]),
        download_url: str_field(item, &["download_url"]),
        file_name: str_field(item, &["file_name"]),
        sha1: provider_hashes(item).0,
        sha256: provider_hashes(item).1,
        sha512: provider_hashes(item).2,
        size: optional_int(item.get("size")),
        created_at: date_value(first_of(item, &["releaseDate", "created_at", "createdAt"])),
        updated_at: date_value(first_of(item, &["updateDate", "updated_at", "updatedAt"])),
        dependencies: normalize_dependencies(first_of(item, &["dependencies", "depend", "softdepend"])),
        version_id: str_field(item, &["version_id"]),
        downloads_known: item.contains_key("downloads") && !item.get("downloads").unwrap_or(&Value::Null).is_null(),
    }
}

fn hangar_result(item: &Map<String, Value>) -> CatalogResult {
    let namespace = item.get("namespace").and_then(Value::as_object).cloned().unwrap_or_default();
    let owner = item.get("owner").and_then(Value::as_object).cloned().unwrap_or_default();
    let stats = item.get("stats").and_then(Value::as_object).cloned().unwrap_or_default();
    let downloads = stats.get("downloads").or_else(|| item.get("downloads"));
    let project_id = namespace
        .get("slug")
        .and_then(Value::as_str)
        .map(|value| value.to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| non_empty_opt(str_field(item, &["slug"])))
        .or_else(|| non_empty_opt(str_field(item, &["project_id"])))
        .unwrap_or_default();
    CatalogResult {
        source: "hangar".to_owned(),
        project_id,
        title: non_empty(str_field(item, &["name"]), "Hangar project"),
        description: str_field(item, &["description"]),
        author: owner.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
        icon_url: str_field(item, &["avatar", "icon_url"]),
        downloads: int_value(downloads),
        version: str_field(item, &["version"]),
        download_url: str_field(item, &["download_url"]),
        file_name: str_field(item, &["file_name"]),
        sha1: provider_hashes(item).0,
        sha256: provider_hashes(item).1,
        sha512: provider_hashes(item).2,
        size: optional_int(item.get("size")),
        created_at: date_value(first_of(item, &["createdAt", "created_at", "dateCreated"])),
        updated_at: date_value(first_of(item, &["lastUpdated", "updatedAt", "updated_at", "dateModified"])),
        dependencies: normalize_dependencies(first_of(item, &["pluginDependencies", "dependencies"])),
        version_id: str_field(item, &["version_id"]),
        downloads_known: (stats.contains_key("downloads") && !stats.get("downloads").unwrap_or(&Value::Null).is_null())
            || (item.contains_key("downloads") && !item.get("downloads").unwrap_or(&Value::Null).is_null()),
    }
}

fn empty_result(source: &str, project_id: &str) -> CatalogResult {
    CatalogResult {
        source: source.to_owned(),
        project_id: project_id.to_owned(),
        title: "Plugin".to_owned(),
        downloads_known: false,
        ..CatalogResult::default()
    }
}

fn non_empty(value: String, fallback: &str) -> String {
    if value.is_empty() { fallback.to_owned() } else { value }
}

fn non_empty_opt(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn str_field(item: &Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = item.get(*key).and_then(Value::as_str) {
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    String::new()
}

fn first_of<'a>(item: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    for key in keys {
        if let Some(value) = item.get(*key) {
            if !value.is_null() && value.as_str().map(|text| !text.is_empty()).unwrap_or(true) {
                return Some(value);
            }
        }
    }
    None
}

// -------------------------------------------------------- кандидаты ----

fn candidate_from_result(result: &CatalogResult, compatible: bool) -> Map<String, Value> {
    let hashes = valid_hashes(Some(&result.sha1), Some(&result.sha256), Some(&result.sha512));
    let mut candidate = Map::new();
    candidate.insert("source".to_owned(), json!(result.source));
    candidate.insert("project_id".to_owned(), json!(result.project_id));
    candidate.insert("title".to_owned(), json!(result.title));
    candidate.insert("description".to_owned(), json!(result.description));
    candidate.insert("author".to_owned(), json!(result.author));
    candidate.insert("icon_url".to_owned(), json!(result.icon_url));
    candidate.insert("downloads".to_owned(), json!(result.downloads));
    candidate.insert("version".to_owned(), json!(result.version));
    candidate.insert("version_id".to_owned(), json!(result.version_id));
    candidate.insert("download_url".to_owned(), json!(result.download_url));
    candidate.insert("file_name".to_owned(), json!(result.file_name));
    candidate.insert("sha1".to_owned(), json!(hashes.0));
    candidate.insert("sha256".to_owned(), json!(hashes.1));
    candidate.insert("sha512".to_owned(), json!(hashes.2));
    candidate.insert("size".to_owned(), result.size.map(|size| json!(size)).unwrap_or(Value::Null));
    candidate.insert("created_at".to_owned(), json!(result.created_at));
    candidate.insert("updated_at".to_owned(), json!(result.updated_at));
    candidate.insert("dependencies".to_owned(), Value::Array(result.dependencies.clone()));
    candidate.insert(
        "trusted_hash".to_owned(),
        json!(!hashes.0.is_empty() || !hashes.1.is_empty() || !hashes.2.is_empty()),
    );
    candidate.insert("compatible".to_owned(), json!(compatible));
    candidate
}

fn modrinth_candidate(
    project_id: &str,
    project: &Map<String, Value>,
    item: &Map<String, Value>,
    core: &str,
    version: &str,
) -> Map<String, Value> {
    let result = modrinth_result(project);
    let files = item.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    let primary = files
        .iter()
        .find(|file| file.get("primary").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| files.first())
        .and_then(|file| file.as_object().cloned())
        .unwrap_or_default();
    let hashes = primary
        .get("hashes")
        .and_then(Value::as_object)
        .map(|fields| {
            valid_hashes(
                fields.get("sha1").and_then(Value::as_str),
                fields.get("sha256").and_then(Value::as_str),
                fields.get("sha512").and_then(Value::as_str),
            )
        })
        .unwrap_or((String::new(), String::new(), String::new()));
    let file_name = safe_jar_file_name(
        &primary
            .get("filename")
            .and_then(Value::as_str)
            .map(|value| value.to_owned())
            .unwrap_or_else(|| format!("{project_id}.jar")),
    )
    .unwrap_or_else(|_| format!("{project_id}.jar"));
    let version_id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let published = date_value(first_of(item, &["date_published", "published_at"]));
    let updated_at = if result.updated_at.is_empty() { published.clone() } else { result.updated_at.clone() };
    let compatible = !version.is_empty() && !core.is_empty() && core != "custom" && core != "bukkit";
    let mut candidate = candidate_from_result(&result, compatible);
    candidate.insert(
        "version".to_owned(),
        json!(item
            .get("version_number")
            .and_then(Value::as_str)
            .or_else(|| item.get("name").and_then(Value::as_str))
            .unwrap_or_default()),
    );
    candidate.insert("version_id".to_owned(), json!(version_id));
    candidate.insert("download_url".to_owned(), json!(primary.get("url").and_then(Value::as_str).unwrap_or_default()));
    candidate.insert("file_name".to_owned(), json!(file_name));
    candidate.insert("sha1".to_owned(), json!(hashes.0));
    candidate.insert("sha256".to_owned(), json!(hashes.1));
    candidate.insert("sha512".to_owned(), json!(hashes.2));
    candidate.insert(
        "size".to_owned(),
        optional_int(primary.get("size")).map(|size| json!(size)).unwrap_or(Value::Null),
    );
    candidate.insert("created_at".to_owned(), json!(result.created_at));
    candidate.insert("updated_at".to_owned(), json!(updated_at));
    candidate.insert("dependencies".to_owned(), Value::Array(normalize_dependencies(item.get("dependencies"))));
    candidate.insert(
        "trusted_hash".to_owned(),
        json!(!hashes.0.is_empty() || !hashes.1.is_empty() || !hashes.2.is_empty()),
    );
    candidate.insert("compatible".to_owned(), json!(compatible));
    candidate
}

fn hangar_candidate(base_url: &str, project_id: &str, item: &Map<String, Value>, compatible: bool) -> Map<String, Value> {
    let project_path = hangar_project_path(project_id).unwrap_or_default();
    let resolved_version = item
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| item.get("version").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let hashes = provider_hashes(item);
    let file_name = safe_jar_file_name(&format!("{}.jar", project_id.replace('/', "-"))).unwrap_or_default();
    let result = CatalogResult {
        source: "hangar".to_owned(),
        project_id: project_id.to_owned(),
        title: non_empty(
            item.get("projectName")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            project_id,
        ),
        downloads: int_value(item.get("downloads")),
        downloads_known: item.contains_key("downloads") && !item.get("downloads").unwrap_or(&Value::Null).is_null(),
        ..CatalogResult::default()
    };
    let created_at = date_value(first_of(item, &["createdAt", "datePublished", "created_at"]));
    let mut candidate = candidate_from_result(&result, compatible);
    candidate.insert("version".to_owned(), json!(resolved_version));
    candidate.insert(
        "version_id".to_owned(),
        json!(item
            .get("id")
            .or_else(|| item.get("versionId"))
            .and_then(Value::as_str)
            .unwrap_or(result.title.as_str())
            .to_owned()),
    );
    candidate.insert(
        "download_url".to_owned(),
        json!(if resolved_version.is_empty() {
            String::new()
        } else {
            format!("{base_url}/projects/{project_path}/versions/{}/download", percent_encode(&resolved_version))
        }),
    );
    candidate.insert("file_name".to_owned(), json!(file_name));
    candidate.insert("sha1".to_owned(), json!(hashes.0));
    candidate.insert("sha256".to_owned(), json!(hashes.1));
    candidate.insert("sha512".to_owned(), json!(hashes.2));
    candidate.insert(
        "size".to_owned(),
        optional_int(item.get("size")).map(|size| json!(size)).unwrap_or(Value::Null),
    );
    candidate.insert("created_at".to_owned(), json!(created_at));
    candidate.insert("updated_at".to_owned(), json!(created_at));
    candidate.insert("dependencies".to_owned(), Value::Array(normalize_dependencies(first_of(item, &["pluginDependencies", "dependencies"]))));
    candidate.insert(
        "trusted_hash".to_owned(),
        json!(!hashes.0.is_empty() || !hashes.1.is_empty() || !hashes.2.is_empty()),
    );
    candidate.insert("compatible".to_owned(), json!(compatible));
    candidate
}

fn apply_candidate(result: &mut CatalogResult, candidate: &Map<String, Value>) {
    let text = |key: &str| candidate.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    result.version = text("version");
    result.version_id = text("version_id");
    result.download_url = text("download_url");
    result.file_name = text("file_name");
    result.sha1 = text("sha1");
    result.sha256 = text("sha256");
    result.sha512 = text("sha512");
    result.size = optional_int(candidate.get("size"));
    if result.created_at.is_empty() {
        result.created_at = text("created_at");
    }
    if result.updated_at.is_empty() {
        result.updated_at = text("updated_at");
    }
    result.dependencies = normalize_dependencies(candidate.get("dependencies"));
}

// ------------------------------------------------- совместимость/сорт ----

fn validate_sort(sort: &str) -> Result<String, String> {
    let mode = if sort.trim().is_empty() { "relevance".to_owned() } else { sort.trim().to_lowercase() };
    if !PLUGIN_SORT_MODES.contains(&mode.as_str()) {
        return Err(format!("Неизвестный порядок сортировки плагинов: {mode}"));
    }
    Ok(mode)
}

fn provider_sort(kind: &str, mode: &str) -> String {
    if kind == "spigot" {
        return match mode {
            "downloads" => "-downloads".to_owned(),
            "newest" => "-releaseDate".to_owned(),
            "updated" => "-updateDate".to_owned(),
            _ => String::new(),
        };
    }
    if kind == "hangar" {
        return match mode {
            "downloads" => "POPULARITY".to_owned(),
            "newest" => "NEWEST".to_owned(),
            "updated" => "UPDATED".to_owned(),
            _ => String::new(),
        };
    }
    String::new()
}

fn sort_results(results: &mut [CatalogResult], mode: &str) {
    if mode == "relevance" {
        return;
    }
    let key_of = |item: &CatalogResult| -> Option<f64> {
        if mode == "downloads" {
            if item.downloads_known {
                Some(item.downloads as f64)
            } else {
                None
            }
        } else if mode == "newest" {
            date_timestamp(&item.created_at)
        } else {
            date_timestamp(&item.updated_at)
        }
    };
    if !results.iter().any(|item| key_of(item).is_some()) {
        return;
    }
    let mut indexed: Vec<(usize, Option<f64>, &CatalogResult)> = results
        .iter()
        .enumerate()
        .map(|(index, item)| (index, key_of(item), item))
        .collect();
    indexed.sort_by(|a, b| {
        let key = match (&a.1, &b.1) {
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => right
                .partial_cmp(left)
                .unwrap_or(std::cmp::Ordering::Equal),
            (None, None) => std::cmp::Ordering::Equal,
        };
        key.then_with(|| a.2.project_id.to_lowercase().cmp(&b.2.project_id.to_lowercase()))
            .then_with(|| a.2.title.to_lowercase().cmp(&b.2.title.to_lowercase()))
            .then_with(|| a.0.cmp(&b.0))
    });
    let sorted: Vec<CatalogResult> = indexed.into_iter().map(|(_, _, item)| item.clone()).collect();
    results.clone_from_slice(&sorted);
}

fn spigot_compatible(item: &Map<String, Value>, core: &str, version: &str) -> bool {
    let compatible_cores = ["bukkit", "spigot", "paper", "purpur", "folia", "leaf", "pufferfish", "patina"];
    if !compatible_cores.contains(&core.trim().to_lowercase().as_str()) || version.is_empty() {
        return false;
    }
    let tested = first_of(item, &["testedVersions", "tested_versions", "game_versions"]);
    let values: Vec<String> = match tested {
        Some(Value::Array(rows)) => rows
            .iter()
            .map(|value| value.as_str().unwrap_or_default().trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect(),
        Some(Value::String(text)) => text
            .split(',')
            .map(|part| part.trim().to_owned())
            .filter(|part| !part.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    versions_match(&values, version)
}

fn modrinth_plugin_version_compatible(item: &Map<String, Value>, core: &str, version: &str) -> bool {
    let normalized_core = core.trim().to_lowercase();
    if normalized_core.is_empty() || normalized_core == "custom" || normalized_core == "bukkit" || version.is_empty() {
        return false;
    }
    let game_versions = item.get("game_versions").and_then(Value::as_array);
    let loaders = item.get("loaders").and_then(Value::as_array);
    game_versions.is_some_and(|rows| rows.iter().any(|value| value.as_str() == Some(version)))
        && loaders.is_some_and(|rows| {
            rows.iter()
                .any(|value| value.as_str().map(|text| text.trim().to_lowercase()).as_deref() == Some(normalized_core.as_str()))
        })
}

fn hangar_compatible(item: &Map<String, Value>, core: &str, version: &str) -> bool {
    let platform = hangar_platform(core);
    if platform.is_empty() || version.is_empty() {
        return false;
    }
    let Some(dependencies) = first_of(item, &["platformDependencies", "platform_dependencies"]).and_then(Value::as_object) else {
        return false;
    };
    let supported = dependencies
        .iter()
        .find(|(key, _)| key.to_uppercase() == platform)
        .map(|(_, value)| value);
    let values: Vec<String> = match supported {
        Some(Value::Array(rows)) => rows
            .iter()
            .map(|value| value.as_str().unwrap_or_default().trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect(),
        Some(Value::String(text)) => vec![text.clone()],
        _ => Vec::new(),
    };
    versions_match(&values, version)
}

fn hangar_platform(core: &str) -> &'static str {
    match core.trim().to_lowercase().as_str() {
        "paper" | "purpur" | "folia" | "leaf" | "pufferfish" | "patina" => "PAPER",
        "velocity" => "VELOCITY",
        "waterfall" => "WATERFALL",
        _ => "",
    }
}

fn versions_match(values: &[String], requested: &str) -> bool {
    values.iter().any(|candidate| {
        candidate == "*"
            || candidate == requested
            || requested.starts_with(&format!("{candidate}."))
            || candidate.starts_with(&format!("{requested}."))
    })
}

fn validate_update_context(core: &str, version: &str) -> Result<(), String> {
    if core.trim().is_empty() || version.trim().is_empty() {
        return Err("Проверка обновлений требует ядро сервера и версию Minecraft".to_owned());
    }
    Ok(())
}

fn is_update(installed: &Map<String, Value>, candidate: &Map<String, Value>) -> bool {
    let current_id = installed.get("version_id").and_then(Value::as_str).unwrap_or_default();
    let latest_id = candidate.get("version_id").and_then(Value::as_str).unwrap_or_default();
    if !current_id.is_empty() && !latest_id.is_empty() {
        return current_id != latest_id;
    }
    let current_updated = installed.get("updated_at").and_then(Value::as_str).unwrap_or_default();
    let latest_updated = candidate.get("updated_at").and_then(Value::as_str).unwrap_or_default();
    if !current_updated.is_empty() && !latest_updated.is_empty() {
        return current_updated != latest_updated;
    }
    let current_version = installed.get("version").and_then(Value::as_str).unwrap_or_default();
    let latest_version = candidate.get("version").and_then(Value::as_str).unwrap_or_default();
    if !current_version.is_empty() || !latest_version.is_empty() {
        return current_version != latest_version;
    }
    let current_hash = installed.get("sha256").and_then(Value::as_str).unwrap_or_default();
    let latest_hash = candidate.get("sha256").and_then(Value::as_str).unwrap_or_default();
    !current_hash.is_empty() && !latest_hash.is_empty() && current_hash.to_lowercase() != latest_hash.to_lowercase()
}

// --------------------------------------------- установленные плагины ----

pub(crate) fn installed_plugin_metadata_index(target: &Path) -> Result<Vec<Map<String, Value>>, String> {
    let root = target;
    if root.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
        return Err("Каталог plugins является символьной ссылкой".to_owned());
    }
    if !root.exists() {
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Err("Путь plugins не является каталогом".to_owned());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root)
        .map_err(|error| format!("Не удалось прочитать каталог plugins: {error}"))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort_by_key(|path| path.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default());
    let mut result: Vec<Map<String, Value>> = Vec::new();
    for jar in entries.into_iter().take(2000) {
        if result.len() >= 1000 {
            break;
        }
        let name = jar.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
        if !name.to_lowercase().ends_with(".jar")
            || jar.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true)
            || !jar.is_file()
        {
            continue;
        }
        if safe_jar_file_name(&name).is_err() {
            continue;
        }
        let sidecar = plugin_metadata_path(&jar);
        if sidecar.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true) || !sidecar.is_file() {
            continue;
        }
        let Ok(metadata) = read_plugin_metadata(&sidecar) else { continue };
        let Some(fields) = metadata.as_object() else { continue };
        if fields.get("schema").and_then(Value::as_i64) != Some(PLUGIN_METADATA_SCHEMA)
            || fields.get("managed_by").and_then(Value::as_str) != Some("trial-launcher")
        {
            continue;
        }
        if fields.get("file_name").and_then(Value::as_str) != Some(name.as_str()) {
            continue;
        }
        if !["modrinth", "spigot", "hangar", "custom"].contains(&fields.get("source").and_then(Value::as_str).unwrap_or_default()) {
            continue;
        }
        let catalog_id = fields.get("catalog_id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let api_url = fields.get("api_url").and_then(Value::as_str).unwrap_or_default().to_owned();
        let project_id = fields.get("project_id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let sha256 = fields.get("sha256").and_then(Value::as_str).unwrap_or_default().to_lowercase();
        if catalog_id.is_empty()
            || !api_url.starts_with("https://")
            || project_id.is_empty()
            || sha256.len() != 64
            || !sha256.chars().all(|character| character.is_ascii_hexdigit())
        {
            continue;
        }
        let Some((actual_hash, actual_size)) = hash_regular_file(&jar) else { continue };
        if Some(actual_size as i64) != optional_int(fields.get("size")) {
            continue;
        }
        if actual_hash != sha256 {
            continue;
        }
        let mut entry = Map::new();
        entry.insert("source".to_owned(), fields.get("source").cloned().unwrap_or(Value::Null));
        entry.insert("catalog_id".to_owned(), json!(catalog_id));
        entry.insert("api_url".to_owned(), json!(api_url));
        entry.insert("project_id".to_owned(), json!(project_id));
        entry.insert("file_name".to_owned(), json!(name));
        entry.insert("path".to_owned(), json!(jar));
        entry.insert("version".to_owned(), json!(fields.get("version").and_then(Value::as_str).unwrap_or_default()));
        entry.insert("version_id".to_owned(), json!(fields.get("version_id").and_then(Value::as_str).unwrap_or_default()));
        entry.insert("core".to_owned(), json!(fields.get("core").and_then(Value::as_str).unwrap_or_default()));
        entry.insert("minecraft_version".to_owned(), json!(fields.get("minecraft_version").and_then(Value::as_str).unwrap_or_default()));
        entry.insert("sha256".to_owned(), json!(sha256));
        entry.insert("trusted_hash".to_owned(), json!(fields.get("trusted_hash").and_then(Value::as_bool).unwrap_or(false)));
        entry.insert("size".to_owned(), json!(actual_size));
        entry.insert("created_at".to_owned(), json!(date_value(fields.get("created_at"))));
        entry.insert("updated_at".to_owned(), json!(date_value(fields.get("updated_at"))));
        entry.insert("installed_at".to_owned(), json!(date_value(fields.get("installed_at"))));
        entry.insert("dependencies".to_owned(), Value::Array(normalize_dependencies(fields.get("dependencies"))));
        entry.insert("managed".to_owned(), json!(true));
        result.push(entry);
    }
    Ok(result)
}

pub(crate) fn remove_installed_plugin(target: &Path, file_name: &str) -> Result<bool, String> {
    let file_name = safe_jar_file_name(file_name)?;
    let entry = installed_plugin_metadata_index(target)?
        .into_iter()
        .find(|item| item.get("file_name").and_then(Value::as_str) == Some(file_name.as_str()));
    if entry.is_none() {
        return Ok(false);
    }
    let directory = plugin_directory(target, false)?;
    let jar = directory.join(&file_name);
    let metadata = plugin_metadata_path(&jar);
    let jar_ok = jar.is_file() && !jar.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true);
    let metadata_ok = metadata.is_file() && !metadata.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true);
    if !jar_ok || !metadata_ok {
        return Err("Установленный плагин или его метаданные изменились".to_owned());
    }
    std::fs::remove_file(&jar)
        .and_then(|_| std::fs::remove_file(&metadata))
        .map_err(|_| "Не удалось удалить плагин, установленный из каталога".to_owned())?;
    Ok(true)
}

fn plugin_directory(path: &Path, create: bool) -> Result<PathBuf, String> {
    let directory = path.to_path_buf();
    if directory.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
        return Err("Каталог plugins является символьной ссылкой".to_owned());
    }
    if create {
        std::fs::create_dir_all(&directory).map_err(|_| "Каталог plugins недоступен".to_owned())?;
    }
    if !directory.is_dir() {
        return Err("Каталог plugins недоступен".to_owned());
    }
    Ok(directory)
}

pub(crate) fn safe_jar_file_name(value: &str) -> Result<String, String> {
    let name = safe_file_name(value)?;
    if name.chars().count() > 180 || !name.to_lowercase().ends_with(".jar") {
        return Err("Файл серверного плагина должен иметь безопасное имя и расширение .jar".to_owned());
    }
    Ok(name)
}

fn plugin_project_id(value: &str) -> Result<String, String> {
    let project_id = value.trim();
    if project_id.is_empty()
        || project_id.len() > 200
        || project_id == "."
        || project_id == ".."
        || project_id.chars().any(|character| (character as u32) < 32 || character as u32 == 127)
    {
        return Err("Некорректный project ID зависимости серверного плагина".to_owned());
    }
    Ok(project_id.to_owned())
}

fn plugin_version_id(value: &str) -> Result<String, String> {
    let version_id = value.trim();
    if !version_id.is_empty()
        && (version_id.len() > 200 || version_id.chars().any(|character| (character as u32) < 32 || character as u32 == 127))
    {
        return Err("Некорректный ID версии зависимости серверного плагина".to_owned());
    }
    Ok(version_id.to_owned())
}

fn candidate_has_trusted_hash(candidate: &Map<String, Value>) -> bool {
    ["sha1", "sha256", "sha512"].iter().any(|key| {
        !candidate
            .get(*key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty()
    })
}

/// возвращает валидные SHA-1, SHA-256 и SHA-512; иначе пустые строки
fn candidate_hashes(candidate: &Map<String, Value>) -> [Option<String>; 3] {
    let valid = valid_hashes(
        candidate.get("sha1").and_then(Value::as_str),
        candidate.get("sha256").and_then(Value::as_str),
        candidate.get("sha512").and_then(Value::as_str),
    );
    [
        (!valid.0.is_empty()).then_some(valid.0),
        (!valid.1.is_empty()).then_some(valid.1),
        (!valid.2.is_empty()).then_some(valid.2),
    ]
}

fn unique_plugin_file(directory: &Path, name: &str, reserved_names: &[String]) -> Result<PathBuf, String> {
    let mut candidate = unique_file(directory, name);
    let candidate_name = candidate.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
    if !reserved_names.contains(&candidate_name) && !plugin_metadata_path(&candidate).exists() {
        return Ok(candidate);
    }
    let stem = candidate
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let suffix = candidate
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    let mut index = 2;
    loop {
        candidate = directory.join(format!("{stem}-{index}{suffix}"));
        let candidate_name = candidate.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
        if !reserved_names.contains(&candidate_name)
            && candidate.symlink_metadata().is_err()
            && !plugin_metadata_path(&candidate).symlink_metadata().is_ok()
        {
            return Ok(candidate);
        }
        index += 1;
    }
}

fn unique_file(directory: &Path, name: &str) -> PathBuf {
    let mut candidate = directory.join(name);
    if candidate.symlink_metadata().is_err() {
        return candidate;
    }
    let stem = candidate
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let suffix = candidate
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    let mut index = 2;
    loop {
        candidate = directory.join(format!("{stem}-{index}{suffix}"));
        if candidate.symlink_metadata().is_err() {
            return candidate;
        }
        index += 1;
    }
}

pub(crate) fn plugin_metadata_path(jar: &Path) -> PathBuf {
    let name = jar.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
    jar.with_file_name(format!(".{name}.trial-plugin.json"))
}

fn write_plugin_metadata(jar: &Path, value: &Map<String, Value>) -> Result<(), String> {
    let sidecar = plugin_metadata_path(jar);
    if jar
        .parent()
        .and_then(|parent| parent.symlink_metadata().ok())
        .map(|meta| meta.is_symlink())
        .unwrap_or(false)
        || sidecar.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false)
    {
        return Err("Метаданные каталога являются символьной ссылкой".to_owned());
    }
    let encoded = serde_json::to_string(value).map_err(|error| error.to_string())?;
    if encoded.len() > PLUGIN_METADATA_MAX_BYTES as usize {
        return Err("Метаданные плагина слишком велики".to_owned());
    }
    let temporary = jar
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".trial-plugin-meta-{}.tmp", super::mods::random_hex(6)));
    std::fs::write(&temporary, encoded.as_bytes()).map_err(|error| format!("Не удалось сохранить метаданные плагина: {error}"))?;
    set_private_permissions(&temporary);
    if sidecar.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
        let _ = std::fs::remove_file(&temporary);
        return Err("Метаданные каталога являются символьной ссылкой".to_owned());
    }
    let outcome = std::fs::rename(&temporary, &sidecar);
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    outcome.map_err(|error| format!("Не удалось сохранить метаданные плагина: {error}"))
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn read_plugin_metadata(path: &Path) -> Result<Value, String> {
    if path.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true) {
        return Err("Метаданные плагина являются символьной ссылкой".to_owned());
    }
    let metadata = path
        .symlink_metadata()
        .map_err(|error| format!("Не удалось прочитать метаданные плагина: {error}"))?;
    if !metadata.is_file() || metadata.len() > PLUGIN_METADATA_MAX_BYTES {
        return Err("Метаданные плагина повреждены".to_owned());
    }
    let content = std::fs::read_to_string(path).map_err(|error| format!("Не удалось прочитать метаданные плагина: {error}"))?;
    serde_json::from_str(&content).map_err(|_| "Метаданные плагина повреждены".to_owned())
}

fn hash_regular_file(path: &Path) -> Option<(String, u64)> {
    if path.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true) {
        return None;
    }
    let Ok(metadata) = path.metadata() else { return None };
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return None;
    }
    let digest = file_hash(path, "sha256").ok()?;
    Some((digest, metadata.len()))
}

fn hangar_project_path(project_id: &str) -> Result<String, String> {
    let parts: Vec<&str> = project_id.trim().split('/').collect();
    if parts.is_empty()
        || parts.len() > 2
        || parts.iter().any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err("Некорректный идентификатор Hangar-проекта".to_owned());
    }
    Ok(parts.iter().map(|part| percent_encode(part)).collect::<Vec<_>>().join("/"))
}

fn provider_rows<'a>(value: &'a Value, keys: &[&str]) -> &'a [Value] {
    if let Some(rows) = value.as_array() {
        return rows;
    }
    if let Some(fields) = value.as_object() {
        for key in keys {
            if let Some(rows) = fields.get(*key).and_then(Value::as_array) {
                return rows;
            }
        }
    }
    &[]
}

fn latest_item(rows: &[Map<String, Value>], date_fields: &[&str]) -> Map<String, Value> {
    if rows.is_empty() {
        return Map::new();
    }
    let mut selected = 0;
    let mut selected_at = first_of(&rows[0], date_fields).and_then(|value| date_timestamp_of(value)).unwrap_or(0.0);
    for (index, item) in rows.iter().enumerate().skip(1) {
        let item_at = first_of(item, date_fields).and_then(|value| date_timestamp_of(value)).unwrap_or(0.0);
        if item_at > selected_at {
            selected = index;
            selected_at = item_at;
        }
    }
    rows[selected].clone()
}

fn provider_hashes(item: &Map<String, Value>) -> (String, String, String) {
    let raw = item.get("hashes").and_then(Value::as_object);
    let checksums = item.get("checksums").and_then(Value::as_object);
    let field = |key: &str| -> String {
        checksums
            .and_then(|fields| fields.get(key).and_then(Value::as_str))
            .or_else(|| raw.and_then(|fields| fields.get(key).and_then(Value::as_str)))
            .or_else(|| item.get(key).and_then(Value::as_str))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    valid_hashes(
        Some(field("sha1")).as_deref(),
        Some(field("sha256")).as_deref(),
        Some(field("sha512")).as_deref(),
    )
}

fn valid_hashes(sha1: Option<&str>, sha256: Option<&str>, sha512: Option<&str>) -> (String, String, String) {
    let normalize = |value: Option<&str>, length: usize| -> String {
        let digest = value.unwrap_or_default().trim().to_lowercase();
        if digest.len() == length && digest.chars().all(|character| character.is_ascii_hexdigit()) {
            digest
        } else {
            String::new()
        }
    };
    (
        normalize(sha1, 40),
        normalize(sha256, 64),
        normalize(sha512, 128),
    )
}

fn normalize_dependencies(value: Option<&Value>) -> Vec<Value> {
    let rows: Vec<Value> = match value {
        Some(Value::Array(items)) => items.iter().take(100).cloned().collect(),
        Some(Value::Object(fields)) => fields
            .iter()
            .take(100)
            .map(|(key, item)| {
                let mut entry = match item {
                    Value::Object(_) => item.as_object().cloned().unwrap_or_default(),
                    _ => {
                        let mut entry = Map::new();
                        entry.insert("version".to_owned(), item.clone());
                        entry
                    }
                };
                entry.insert("id".to_owned(), json!(key));
                Value::Object(entry)
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut result: Vec<Value> = Vec::new();
    for item in rows {
        match item {
            Value::Object(mut fields) => {
                let bounded: Map<String, Value> = fields
                    .iter_mut()
                    .take(30)
                    .map(|(key, child)| (key.chars().take(80).collect::<String>(), bounded_json(child, 0)))
                    .collect();
                result.push(Value::Object(bounded));
            }
            Value::String(text) => result.push(json!({ "id": text.chars().take(200).collect::<String>() })),
            Value::Number(number) => result.push(json!({ "id": number.to_string() })),
            _ => {}
        }
    }
    result
}

fn bounded_json(value: &Value, depth: usize) -> Value {
    if depth >= 4 {
        return Value::Null;
    }
    match value {
        Value::Null | Value::Bool(_) => value.clone(),
        Value::Number(_) => value.clone(),
        Value::String(text) => json!(text.chars().take(500).collect::<String>()),
        Value::Array(items) => Value::Array(items.iter().take(50).map(|item| bounded_json(item, depth + 1)).collect()),
        Value::Object(fields) => {
            let bounded: Map<String, Value> = fields
                .iter()
                .take(30)
                .map(|(key, item)| (key.chars().take(80).collect::<String>(), bounded_json(item, depth + 1)))
                .collect();
            Value::Object(bounded)
        }
    }
}

fn int_value(value: Option<&Value>) -> i64 {
    value.and_then(Value::as_i64).map(|number| number.max(0)).unwrap_or(0)
}

fn optional_int(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).map(|number| number.max(0))
}

// ------------------------------------------------------------- даты ----

fn date_value(value: Option<&Value>) -> String {
    let Some(value) = value else { return String::new() };
    if let Some(number) = value.as_f64() {
        let mut timestamp = number;
        if timestamp > 100_000_000_000.0 {
            timestamp /= 1000.0;
        }
        let Some(moment) = DateTime::from_timestamp(timestamp as i64, 0) else { return String::new() };
        return moment.to_rfc3339_opts(SecondsFormat::Secs, true);
    }
    let text = value.as_str().unwrap_or_default().trim();
    if text.is_empty() {
        return String::new();
    }
    if text.chars().all(|character| character.is_ascii_digit() || character == '.') {
        if let Ok(number) = text.parse::<f64>() {
            return date_value(Some(&json!(number)));
        }
    }
    match DateTime::parse_from_rfc3339(text) {
        Ok(moment) => moment.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true),
        Err(_) => text.chars().take(64).collect::<String>(),
    }
}

fn date_timestamp(value: &str) -> Option<f64> {
    let normalized = date_value(Some(&json!(value)));
    if normalized.is_empty() {
        return None;
    }
    date_timestamp_of(&json!(normalized))
}

fn date_timestamp_of(value: &Value) -> Option<f64> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|moment| moment.timestamp() as f64 + moment.timestamp_subsec_nanos() as f64 / 1e9)
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ---------------------------------------------------------- операции ----

/// выбирает каталог плагинов по идентификатору или активной настройке
pub(crate) fn plugin_catalog_for(config: &ConfigStore, catalog_id: &str) -> Result<Value, String> {
    let settings = config.settings();
    let catalogs = normalize_plugin_catalogs(settings.get("plugin_catalogs").unwrap_or(&Value::Null));
    let active = settings
        .get("active_plugin_catalog_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let requested = catalog_id.trim().to_lowercase();
    let selected = catalogs
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| !requested.is_empty() && item.get("id").and_then(Value::as_str) == Some(requested.as_str()))
                .or_else(|| {
                    items
                        .iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(active))
                })
                .or_else(|| {
                    items
                        .iter()
                        .find(|item| item.get("enabled").and_then(Value::as_bool).unwrap_or(true))
                })
                .cloned()
        })
        .ok_or_else(|| "Каталог плагинов не настроен".to_owned())?;
    if !selected.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
        return Err("Выбранный каталог плагинов выключен".to_owned());
    }
    Ok(selected)
}

pub(crate) fn plugin_catalog_service(request: &Value) -> Result<(ConfigStore, PluginCatalog), String> {
    let config = ConfigStore::load()?;
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    let http = client_from_settings(config.settings(), operation_id)?;
    let catalog = plugin_catalog_for(&config, request.get("catalog_id").and_then(Value::as_str).unwrap_or_default())?;
    Ok((config, PluginCatalog::new(http, &catalog)))
}

pub fn server_plugin_catalogs(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let settings = config.settings();
    Ok(json!({
        "catalogs": normalize_plugin_catalogs(settings.get("plugin_catalogs").unwrap_or(&Value::Null)),
        "active_catalog_id": settings.get("active_plugin_catalog_id").and_then(Value::as_str).unwrap_or_default(),
    }))
}

pub fn save_server_plugin_catalogs(request: &Value) -> Result<Value, String> {
    let mut config = ConfigStore::load()?;
    let catalogs = normalize_plugin_catalogs(request.get("catalogs").unwrap_or(&Value::Null));
    let settings = config.settings();
    let mut active = request
        .get("active_catalog_id")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            settings
                .get("active_plugin_catalog_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_lowercase()
        });
    let items = catalogs.as_array().cloned().unwrap_or_default();
    if !items.iter().any(|item| item.get("id").and_then(Value::as_str) == Some(active.as_str())) {
        active = items
            .iter()
            .find(|item| item.get("enabled").and_then(Value::as_bool).unwrap_or(true))
            .or_else(|| items.first())
            .and_then(|item| item.get("id").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
    }
    config.set_values(&json!({ "plugin_catalogs": catalogs, "active_plugin_catalog_id": active }))?;
    let settings = config.settings();
    Ok(json!({
        "catalogs": normalize_plugin_catalogs(settings.get("plugin_catalogs").unwrap_or(&Value::Null)),
        "active_catalog_id": settings.get("active_plugin_catalog_id").and_then(Value::as_str).unwrap_or_default(),
    }))
}

pub fn server_plugin_search(request: &Value) -> Result<Value, String> {
    let (config, provider) = plugin_catalog_service(request)?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let info = server_instance(&config, name)?;
    let settings = super::server::server_config(&config, &instances_store(&config)?, name)?;
    let core = settings.get("core").and_then(Value::as_str).unwrap_or("custom");
    let version = settings.get("version").and_then(Value::as_str).unwrap_or_default();
    let limit = request.get("limit").and_then(Value::as_i64).unwrap_or(12).clamp(1, 30);
    let results = provider.search(
        request.get("query").and_then(Value::as_str).unwrap_or_default(),
        core,
        version,
        limit,
        request.get("sort_by").and_then(Value::as_str).unwrap_or("relevance"),
    )?;
    let installed: std::collections::HashMap<String, Map<String, Value>> = provider
        .installed(&info.path.join("plugins"))?
        .into_iter()
        .filter_map(|entry| {
            let project_id = entry.get("project_id").and_then(Value::as_str).unwrap_or_default().to_owned();
            if project_id.is_empty() {
                return None;
            }
            Some((project_id, entry))
        })
        .collect();
    let catalog_id = provider_catalog_id(&provider);
    let rows: Vec<Value> = results
        .iter()
        .map(|item| {
            let mut value = item.as_dict();
            let project_id = item.project_id.clone();
            let installed_item = installed.get(&project_id);
            value.insert("installed".to_owned(), json!(installed_item.is_some()));
            if let Some(entry) = installed_item {
                value.insert(
                    "file_name".to_owned(),
                    json!(entry.get("file_name").and_then(Value::as_str).unwrap_or_default()),
                );
            }
            Value::Object(value)
        })
        .collect();
    Ok(json!({
        "results": rows,
        "catalog_id": catalog_id,
        "core": settings.get("core").cloned().unwrap_or(json!("custom")),
        "version": settings.get("version").cloned().unwrap_or(json!("")),
    }))
}

pub fn server_plugin_details(request: &Value) -> Result<Value, String> {
    let (config, provider) = plugin_catalog_service(request)?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let _ = server_instance(&config, name)?;
    let settings = super::server::server_config(&config, &instances_store(&config)?, name)?;
    let details = provider.details(
        request.get("project_id").and_then(Value::as_str).unwrap_or_default(),
        settings.get("core").and_then(Value::as_str).unwrap_or("custom"),
        settings.get("version").and_then(Value::as_str).unwrap_or_default(),
    )?;
    Ok(json!({
        "details": details,
        "catalog_id": provider_catalog_id(&provider),
        "core": settings.get("core").cloned().unwrap_or(json!("custom")),
        "version": settings.get("version").cloned().unwrap_or(json!("")),
    }))
}

pub fn server_plugin_updates(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    let http = client_from_settings(config.settings(), operation_id)?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let info = server_instance(&config, name)?;
    let settings = super::server::server_config(&config, &instances_store(&config)?, name)?;
    let core = settings.get("core").and_then(Value::as_str).unwrap_or("custom").to_owned();
    let version = settings.get("version").and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut updates: Vec<Value> = Vec::new();
    let catalogs = normalize_plugin_catalogs(config.settings().get("plugin_catalogs").unwrap_or(&Value::Null));
    for catalog in catalogs.as_array().cloned().unwrap_or_default() {
        if !catalog.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
            continue;
        }
        let catalog_name = catalog.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let provider = PluginCatalog::new(http.clone(), &catalog);
        let mut values = provider.check_updates(&info.path.join("plugins"), &core, &version)?;
        for value in values.iter_mut() {
            if let Some(fields) = value.as_object_mut() {
                fields.insert("catalog_name".to_owned(), json!(catalog_name));
            }
        }
        updates.extend(values);
    }
    Ok(json!({
        "updates": updates,
        "files": plugins_file_list(name)?,
    }))
}

pub fn server_plugin_update(request: &Value) -> Result<Value, String> {
    let (config, provider) = plugin_catalog_service(request)?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let info = server_instance(&config, name)?;
    let settings = super::server::server_config(&config, &instances_store(&config)?, name)?;
    let core = settings.get("core").and_then(Value::as_str).unwrap_or("custom").to_owned();
    let version = settings.get("version").and_then(Value::as_str).unwrap_or_default().to_owned();
    let confirm_unverified = request.get("confirm_unverified").and_then(Value::as_bool).unwrap_or(false);
    let file_name = request.get("file_name").and_then(Value::as_str).unwrap_or_default();
    let lock = super::server::InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let result = provider.update_installed(
        &info.path.join("plugins"),
        file_name,
        &core,
        &version,
        confirm_unverified,
    );
    drop(lock);
    let result = result?;
    super::events::emit_status_event(
        "server-plugins-changed",
        &format!("Плагин «{}» обновлён.", result.get("file_name").and_then(Value::as_str).unwrap_or_default()),
    );
    Ok(json!({
        "plugin": result,
        "files": plugins_file_list(name)?,
        "catalog_id": provider_catalog_id(&provider),
    }))
}

pub fn server_plugin_install(request: &Value) -> Result<Value, String> {
    let (config, provider) = plugin_catalog_service(request)?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let info = server_instance(&config, name)?;
    let settings = super::server::server_config(&config, &instances_store(&config)?, name)?;
    let core = settings.get("core").and_then(Value::as_str).unwrap_or("custom").to_owned();
    let version = settings.get("version").and_then(Value::as_str).unwrap_or_default().to_owned();
    let confirm_unverified = request.get("confirm_unverified").and_then(Value::as_bool).unwrap_or(false);
    let project_id = request.get("project_id").and_then(Value::as_str).unwrap_or_default();
    let lock = super::server::InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let result = provider.install(project_id, &info.path.join("plugins"), &version, &core, confirm_unverified);
    drop(lock);
    let result = result?;
    super::events::emit_status_event(
        "server-plugins-changed",
        &format!("Плагин «{}» установлен.", result.get("file_name").and_then(Value::as_str).unwrap_or_default()),
    );
    Ok(json!({
        "plugin": result,
        "files": plugins_file_list(name)?,
        "catalog_id": provider_catalog_id(&provider),
    }))
}

pub fn server_plugin_remove(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let file_name = request
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace('\\', "/");
    if std::path::Path::new(&file_name).file_name().map(|value| value.to_string_lossy().to_string()).as_deref() != Some(file_name.as_str())
        || !file_name.to_lowercase().ends_with(".jar")
    {
        return Err("Укажите имя JAR-файла плагина без пути".to_owned());
    }
    let info = server_instance(&config, name)?;
    let lock = super::server::InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let outcome = (|| -> Result<(), String> {
        if server_is_running(&info.path) {
            return Err("Остановите сервер перед удалением плагина".to_owned());
        }
        if !remove_installed_plugin(&info.path.join("plugins"), &file_name)? {
            super::server_files::server_file_list(&json!({ "name": name, "directory": format!("plugins/{file_name}") }))?;
            let target = info.path.join("plugins").join(&file_name);
            if target.symlink_metadata().is_ok() {
                std::fs::remove_file(&target).map_err(|error| format!("Не удалось удалить плагин: {error}"))?;
            }
        }
        Ok(())
    })();
    drop(lock);
    outcome?;
    super::events::emit_status_event("server-plugins-changed", &format!("Плагин «{file_name}» удалён."));
    Ok(json!({ "removed": true, "files": plugins_file_list(name)? }))
}

fn instances_store(config: &ConfigStore) -> Result<super::instances::InstanceStore, String> {
    super::instances::InstanceStore::new(config)
}

fn server_instance(config: &ConfigStore, name: &str) -> Result<super::instances::InstanceInfo, String> {
    let store = instances_store(config)?;
    let info = store.get(name)?;
    if info.kind.trim().eq_ignore_ascii_case("server") {
        Ok(info)
    } else {
        Err(format!("Экземпляр «{name}» не является сервером"))
    }
}

fn server_is_running(root: &Path) -> bool {
    // признак запуска берём из `server_is_running` в `server.rs`
    super::server::server_is_running(root)
}

fn plugins_file_list(name: &str) -> Result<Value, String> {
    super::server_files::server_file_list(&json!({ "name": name, "directory": "plugins" }))
}

fn provider_catalog_id(provider: &PluginCatalog) -> String {
    provider_catalog_id_value(provider)
}

fn provider_catalog_id_value(provider: &PluginCatalog) -> String {
    // идентификатор каталога берётся из настроек и включается в ответ
    provider.catalog_id_for_response()
}

impl PluginCatalog {
    pub(crate) fn catalog_id_for_response(&self) -> String {
        self.catalog_id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_deduplicates_and_drops_bad_entries() {
        let value = json!([
            { "id": "Modrinth-Plugins", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2/" },
            { "id": "modrinth-plugins", "name": "dup", "kind": "modrinth", "api_url": "https://x.example" },
            { "id": "bad", "name": "Bad", "kind": "nope", "api_url": "https://x.example" },
            { "id": "insecure", "name": "Insecure", "kind": "custom", "api_url": "http://x.example" },
        ]);
        let normalized = normalize_plugin_catalogs(&value);
        assert_eq!(normalized.as_array().map(Vec::len), Some(1));
        assert_eq!(normalized[0]["id"], json!("modrinth-plugins"));
        assert_eq!(normalized[0]["api_url"], json!("https://api.modrinth.com/v2"));
    }

    #[test]
    fn non_array_input_falls_back_to_defaults() {
        let normalized = normalize_plugin_catalogs(&json!("nope"));
        assert_eq!(normalized.as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn safe_jar_name_rejects_non_jar_and_long_names() {
        assert!(safe_jar_file_name("plugin.jar").is_ok());
        // пути с каталогами отклоняются общим валидатором имён файлов
        assert_eq!(safe_jar_file_name("../evil.jar").unwrap_err(), "Небезопасное имя файла мода: ../evil.jar");
        assert_eq!(safe_jar_file_name("plugin.txt").unwrap_err(), "Файл серверного плагина должен иметь безопасное имя и расширение .jar");
        assert!(safe_jar_file_name(&format!("{}.jar", "a".repeat(181))).is_err());
    }

    #[test]
    fn hangar_project_path_validates() {
        assert_eq!(hangar_project_path("Owner/Slug").unwrap(), "Owner/Slug");
        assert!(hangar_project_path("a/b/c").is_err());
        assert!(hangar_project_path("").is_err());
    }

    #[test]
    fn versions_match_accepts_wildcard_and_minor_prefix() {
        assert!(versions_match(&["*".to_owned()], "1.20.4"));
        assert!(versions_match(&["1.20".to_owned()], "1.20.4"));
        assert!(versions_match(&["1.20.4".to_owned()], "1.20.4.1"));
        assert!(!versions_match(&["1.20.4".to_owned()], "1.21"));
    }

    #[test]
    fn sort_results_orders_by_downloads_with_known_flag() {
        let mut results = vec![
            CatalogResult { project_id: "a".into(), downloads: 0, downloads_known: false, ..CatalogResult::default() },
            CatalogResult { project_id: "b".into(), downloads: 10, downloads_known: true, ..CatalogResult::default() },
            CatalogResult { project_id: "c".into(), downloads: 100, downloads_known: true, ..CatalogResult::default() },
        ];
        sort_results(&mut results, "downloads");
        assert_eq!(results[0].project_id, "c");
        // неизвестные загрузки уезжают в конец
        assert_eq!(results[2].project_id, "a");
    }

    #[test]
    fn date_value_normalizes_iso_and_numeric() {
        assert_eq!(date_value(Some(&json!("2024-05-06T07:08:09Z"))), "2024-05-06T07:08:09Z");
        assert_eq!(date_value(Some(&json!("2024-05-06T07:08:09+02:00"))), "2024-05-06T05:08:09Z");
        // 1714972089 = 2024-05-06T05:08:09Z (числовые метки всегда UTC)
        assert_eq!(date_value(Some(&json!(1714972089))), "2024-05-06T05:08:09Z");
        // непарсимый текст усечённо возвращается как есть
        assert_eq!(date_value(Some(&json!("not-a-date"))), "not-a-date");
    }

    #[test]
    fn is_update_prefers_version_ids() {
        let mut installed = Map::new();
        installed.insert("version_id".to_owned(), json!("v1"));
        installed.insert("version".to_owned(), json!("1.0"));
        let mut candidate = Map::new();
        candidate.insert("version_id".to_owned(), json!("v2"));
        candidate.insert("version".to_owned(), json!("1.0"));
        assert!(is_update(&installed, &candidate));
        candidate.insert("version_id".to_owned(), json!("v1"));
        assert!(!is_update(&installed, &candidate));
    }

    #[test]
    fn plugin_metadata_sidecar_name_matches_python() {
        let path = plugin_metadata_path(Path::new("/srv/plugins/essentials.jar"));
        assert_eq!(path.file_name().unwrap().to_string_lossy(), ".essentials.jar.trial-plugin.json");
    }
}
