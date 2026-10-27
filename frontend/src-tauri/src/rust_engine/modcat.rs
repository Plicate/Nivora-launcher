// каталог модов, ресурсов, модпаков и плагинов
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::time::Duration;

use super::config::ConfigStore;
use super::instances::{InstanceInfo, InstanceStore};
use super::instance_workspace::InstanceWorkspace;
use super::modpacks::ModpackProvider;
use super::mods::ModrinthProvider;
use super::net::client_from_settings;
use super::plugin_catalogs::{plugin_catalog_for, PluginCatalog};

const METADATA_SUFFIX: &str = ".trial-project.json";
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MODCAT_TIMEOUT: Duration = Duration::from_secs(180);
const NATIVE_PROJECT_TYPES: &[&str] = &["mod", "resourcepack", "shader", "datapack"];

fn project_directory(project_type: &str) -> Option<&'static str> {
    match project_type {
        "mod" => Some("mods"),
        "resourcepack" => Some("resourcepacks"),
        "shader" => Some("shaderpacks"),
        "datapack" => Some("datapacks"),
        _ => None,
    }
}

pub fn available() -> bool {
    modcat_binary().is_some()
}

fn modcat_binary() -> Option<PathBuf> {
    super::platform::helper_path("nivora-modcat", "NIVORA_MODCAT_BIN").or_else(|| {
        let exe = std::env::current_exe().ok()?;
        let parent = exe.parent()?.parent()?;
        [parent.join("go-modcat").join("nivora-modcat")]
            .into_iter()
            .find(|path| path.is_file())
    })
}

fn modcat_call(payload: &Value) -> Result<Value, String> {
    let binary = modcat_binary().ok_or("Бинарник nivora-modcat недоступен")?;
    let child = super::platform::command(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Не удалось запустить nivora-modcat: {error}"))?;
    let child = Arc::new(Mutex::new(child));
    let (stdin, mut stdout) = {
        let mut guard = child.lock().unwrap_or_else(|poison| poison.into_inner());
        (guard.stdin.take(), guard.stdout.take())
    };
    // ограничиваем время пакетного запроса, чтобы сбой сети не задерживал ответ
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watchdog = child.clone();
    std::thread::spawn(move || {
        if done_rx.recv_timeout(MODCAT_TIMEOUT).is_err() {
            if let Ok(mut guard) = watchdog.lock() {
                let _ = guard.kill();
            }
        }
    });
    let mut output = String::new();
    let mut outcome: Result<(), String> = if let Some(mut stdin) = stdin {
        let body = serde_json::to_string(payload).unwrap_or_default();
        stdin
            .write_all(body.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("Не удалось передать запрос nivora-modcat: {error}"))
    } else {
        Err("Не удалось передать запрос nivora-modcat".to_owned())
    };
    if outcome.is_ok() {
        if let Some(stdout) = stdout.as_mut() {
            outcome = stdout
                .read_to_string(&mut output)
                .map(|_| ())
                .map_err(|error| format!("Не удалось прочитать ответ nivora-modcat: {error}"));
        }
    }
    let _ = done_tx.send(());
    let _ = outcome?;
    let status = {
        let mut guard = child.lock().unwrap_or_else(|poison| poison.into_inner());
        guard.wait()
    }
    .map_err(|error| format!("Не удалось дождаться nivora-modcat: {error}"))?;
    if !status.success() {
        return Err("nivora-modcat завершился с ошибкой".to_owned());
    }
    let value: Value = serde_json::from_str(&output).map_err(|_| "Некорректный ответ nivora-modcat".to_owned())?;
    if value.get("ok") == Some(&Value::Bool(false)) {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("nivora-modcat завершился с ошибкой")
            .to_owned());
    }
    value.get("data").cloned().ok_or_else(|| "Некорректный ответ nivora-modcat".to_owned())
}

pub(crate) fn resource_catalog(config: &ConfigStore, catalog_id: &str) -> Result<Value, String> {
    let settings = config.settings();
    let requested = catalog_id.trim().to_lowercase();
    let empty = Vec::new();
    let catalogs = settings
        .get("resource_catalogs")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let active = settings
        .get("active_catalog_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let selected = catalogs
        .iter()
        .find(|item| {
            !requested.is_empty() && item.get("id").and_then(Value::as_str) == Some(requested.as_str())
        })
        .or_else(|| {
            catalogs
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(active))
        });
    selected.cloned().ok_or_else(|| "Каталог ресурсов не настроен".to_owned())
}

pub(crate) fn instance_mod_context(info: &InstanceInfo) -> Result<(String, Option<String>), String> {
    if info.game_edition.trim().eq_ignore_ascii_case("bedrock") {
        return Err("Моды, ресурспаки и модпаки доступны только для Java-экземпляров".to_owned());
    }
    let is_server = info.kind.trim().eq_ignore_ascii_case("server");
    let mut game_version = if is_server {
        info.server_core_version.trim().to_owned()
    } else {
        String::new()
    };
    if game_version.is_empty() {
        game_version = info.minecraft_version.trim().to_owned();
    }
    if game_version.is_empty() {
        game_version = info.version_id.trim().to_owned();
    }
    if game_version.is_empty() {
        return Err(format!("У экземпляра «{}» не определена версия Minecraft", info.name));
    }
    let loader = if is_server {
        "vanilla".to_owned()
    } else {
        info.loader.trim().to_lowercase()
    };
    Ok((game_version, if loader == "vanilla" { None } else { Some(loader) }))
}

struct SidecarEntry {
    project_id: String,
    version_id: String,
    version: String,
    display_name: String,
    name: String,
    target: PathBuf,
}

fn read_sidecar(path: &Path) -> Option<Map<String, Value>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.is_dir() || meta.len() > MAX_METADATA_BYTES {
        return None;
    }
    let raw = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value.as_object().cloned()
}

fn sidecar_text(metadata: &Map<String, Value>, key: &str) -> String {
    metadata.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_owned()
}

// доверенные modrinth-сайдкары: источник, каталог и тип должны совпадать,
// иначе чужие метаданные не попадают в проверку обновлений
fn scan_trusted_sidecars(
    directory: &Path,
    project_type: &str,
    catalog_id: &str,
    catalog_url: &str,
) -> Vec<SidecarEntry> {
    let mut entries: Vec<(PathBuf, Map<String, Value>)> = Vec::new();
    let Ok(items) = fs::read_dir(directory) else {
        return Vec::new();
    };
    for entry in items.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_symlink() || !name.ends_with(METADATA_SUFFIX) || !name.starts_with('.') {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Some(metadata) = read_sidecar(&path) else { continue };
        let target_name = match name.strip_prefix('.').and_then(|value| value.strip_suffix(METADATA_SUFFIX)) {
            Some(value) if !value.is_empty() => value.to_owned(),
            _ => continue,
        };
        let target = path.with_file_name(target_name);
        if target.is_symlink() || !target.is_file() {
            continue;
        }
        entries.push((target, metadata));
    }
    entries.sort_by(|a, b| a.0.to_string_lossy().to_lowercase().cmp(&b.0.to_string_lossy().to_lowercase()));
    let mut result: Vec<SidecarEntry> = Vec::new();
    for (target, metadata) in entries {
        let source_ok = sidecar_text(&metadata, "source").to_lowercase() == "modrinth";
        let catalog_ok = sidecar_text(&metadata, "catalog").to_lowercase() == "modrinth";
        let catalog_id_ok = sidecar_text(&metadata, "catalog_id") == catalog_id;
        let catalog_url_ok = sidecar_text(&metadata, "catalog_url").to_lowercase() == catalog_url.to_lowercase();
        let project_id = sidecar_text(&metadata, "project_id");
        let version_id = sidecar_text(&metadata, "version_id");
        let game_version_is_string = metadata.get("game_version").map(Value::is_string).unwrap_or(false);
        let loader_is_string = metadata.get("loader").map(Value::is_string).unwrap_or(false);
        if !source_ok
            || !catalog_ok
            || !catalog_id_ok
            || !catalog_url_ok
            || sidecar_text(&metadata, "project_type") != project_type
            || project_id.is_empty()
            || version_id.is_empty()
            || !game_version_is_string
            || !loader_is_string
        {
            continue;
        }
        result.push(SidecarEntry {
            project_id,
            version_id,
            version: sidecar_text(&metadata, "version"),
            display_name: sidecar_text(&metadata, "display_name"),
            name: sidecar_text(&metadata, "name"),
            target,
        });
    }
    result
}


fn catalog_field(catalog: &Value, key: &str, fallback: &str) -> String {
    catalog
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .trim()
        .to_owned()
}

fn request_str(request: &Value, key: &str) -> String {
    request.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_owned()
}

fn modrinth_category_supported(project_type: &str, catalog_kind: &str) -> bool {
    project_type == "mod" && catalog_kind == "modrinth"
}

fn parse_bounded_int(request: &Value, key: &str, default: i64, min: i64, max: i64) -> Result<i64, String> {
    let raw = request.get(key).cloned().unwrap_or(json!(default));
    match raw.as_i64().or_else(|| raw.as_str().and_then(|value| value.parse::<i64>().ok())) {
        Some(value) if !raw.is_null() => Ok(value.clamp(min, max)),
        Some(_) => Ok(default),
        None => Err("Номер страницы каталога должен быть числом".to_owned()),
    }
}

pub fn mods_search(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let mut instance_name = request_str(request, "instance_name");
    let project_type = request_str(request, "project_type").to_lowercase();
    if project_type.is_empty() {
        return Err("Неизвестный тип Modrinth-проекта".to_owned());
    }
    // нумерация страниц начинается с нуля
    let page = parse_bounded_int(request, "page", 0, 0, 500)?;
    let page_size = parse_bounded_int(request, "limit", 12, 1, 30)?;
    let offset = page * page_size;
    // у каталогов без смещения забираем начало выдачи и режем локально
    let local_window = 30.min(offset + page_size);

    let store = InstanceStore::new(&config)?;
    let mut info: Option<InstanceInfo> = None;
    if !instance_name.is_empty() {
        let item = store.get(&instance_name)?;
        if item.game_edition.trim().eq_ignore_ascii_case("bedrock") {
            // bedrock-профиль не может принимать Java-контент: ищем по явной версии
            instance_name = String::new();
        } else {
            info = Some(item);
        }
    }
    let (game_version, loader) = match &info {
        Some(item) => {
            let kind = item.kind.trim().to_lowercase();
            if project_type == "plugin" && kind != "server" {
                return Err("Каталог плагинов доступен только для серверных экземпляров".to_owned());
            }
            if project_type != "plugin" && kind == "server" {
                return Err("Каталог ресурсов доступен только для клиентских экземпляров".to_owned());
            }
            instance_mod_context(item)?
        }
        None => explicit_mod_context(request)?,
    };

    let query = request_str(request, "query");
    let requested_category = request_str(request, "category");
    let sort_by = request_str(request, "sort_by");
    let catalog = if project_type == "plugin" {
        plugin_catalog_for(&config, &request_str(request, "catalog_id"))?
    } else {
        resource_catalog(&config, &request_str(request, "catalog_id"))?
    };
    let catalog_id = catalog_field(&catalog, "id", "modrinth");
    let category = if modrinth_category_supported(&project_type, &catalog_field(&catalog, "kind", "modrinth")) {
        requested_category
    } else {
        String::new()
    };
    let requested_core = request_str(request, "server_core");
    let core = if !requested_core.is_empty() {
        requested_core.to_lowercase()
    } else {
        info.as_ref()
            .map(|item| item.server_core.trim().to_lowercase())
            .unwrap_or_default()
    };
    let core = if core.is_empty() { "paper".to_owned() } else { core };
    let http = client_from_settings(
        config.settings(),
        request.get("operation_id").and_then(Value::as_str).unwrap_or_default(),
    )?;

    let mut values: Vec<Value>;
    let mut total: Option<i64> = None;
    let mut local_sliced = false;
    if project_type == "plugin" {
        let provider = PluginCatalog::new(http.clone(), &catalog);
        let fetch_limit = if page != 0 { local_window } else { page_size };
        values = provider
            .search(&query, &core, &game_version, fetch_limit, &sort_by)?
            .iter()
            .map(|item| {
                let mut dict = item.as_dict();
                dict.insert("project_type".to_owned(), json!("plugin"));
                Value::Object(dict)
            })
            .collect();
        local_sliced = true;
    } else if project_type == "modpack" {
        let provider = ModpackProvider::new(http.clone(), &catalog)?;
        values = provider
            .search(&query, &game_version, page_size, offset, &sort_by)?
            .iter()
            .map(|item| Value::Object(item.as_dict()))
            .collect();
        total = provider.last_total();
    } else {
        let kind = catalog_field(&catalog, "kind", "modrinth");
        if kind == "ftb" || kind == "technic" {
            return Err("Каталоги FTB и Technic предназначены для поиска модпаков".to_owned());
        }
        if available() && NATIVE_PROJECT_TYPES.contains(&project_type.as_str()) && category.is_empty() {
            let mut payload = Map::new();
            payload.insert("cmd".to_owned(), json!("search"));
            payload.insert("query".to_owned(), json!(query));
            payload.insert("game_version".to_owned(), json!(game_version));
            payload.insert("loader".to_owned(), json!(loader.clone().unwrap_or_default()));
            payload.insert("limit".to_owned(), json!(page_size));
            payload.insert("project_type".to_owned(), json!(project_type));
            payload.insert("sort_by".to_owned(), json!(sort_by));
            if offset > 0 {
                payload.insert("offset".to_owned(), json!(offset));
            }
            payload.insert("api_url".to_owned(), json!(catalog_field(&catalog, "api_url", "")));
            let data = modcat_call(&Value::Object(payload))?;
            values = data.get("hits").and_then(Value::as_array).cloned().unwrap_or_default();
            total = data.get("total").and_then(Value::as_i64);
        } else {
            let provider = ModrinthProvider::new(http.clone(), &catalog_field(&catalog, "api_url", ""), &catalog_id);
            values = provider.search(
                &query,
                Some(game_version.as_str()),
                loader.as_deref(),
                page_size,
                if offset != 0 { offset } else { 0 },
                &project_type,
                if category.is_empty() { None } else { Some(category.as_str()) },
                &sort_by,
            )?;
            total = provider.last_total();
        }
    }

    let has_more = if local_sliced {
        let fetched = values.len() as i64;
        if page != 0 {
            let start = (offset as usize).min(values.len());
            let end = ((offset + page_size) as usize).min(values.len());
            values = values[start..end].to_vec();
        }
        fetched >= local_window && offset + page_size < 30
    } else if let Some(total) = total {
        offset + (values.len() as i64) < total
    } else {
        values.len() as i64 >= page_size
    };

    let workspace = InstanceWorkspace::new()?;
    let installed: Vec<String> = if !instance_name.is_empty() && project_type != "plugin" {
        workspace.installed_projects(&instance_name, &project_type, Some(&catalog_id))?
    } else {
        Vec::new()
    };
    let mut installed_plugins: Map<String, Value> = Map::new();
    if !instance_name.is_empty() && project_type == "plugin" {
        if let Some(item) = &info {
            for entry in PluginCatalog::new(http.clone(), &catalog)
                .installed(&item.path.join("plugins"))
                .unwrap_or_default()
            {
                let entry_id = entry.get("project_id").and_then(Value::as_str).unwrap_or_default().to_owned();
                if !entry_id.is_empty() {
                    installed_plugins.insert(entry_id, Value::Object(entry));
                }
            }
        }
    }
    let raw_server_core = if !requested_core.is_empty() {
        requested_core
    } else {
        info.as_ref()
            .map(|item| item.server_core.trim().to_owned())
            .unwrap_or_default()
    };
    let server_core = if raw_server_core.is_empty() { "paper".to_owned() } else { raw_server_core };
    let results: Vec<Value> = values
        .into_iter()
        .filter(|value| value.is_object())
        .map(|mut value| {
            let object = value.as_object_mut().unwrap();
            let project_id = object
                .get("project_id")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let icon_url = ["icon_url", "icon", "logo_url"]
                .iter()
                .find_map(|key| {
                    object
                        .get(*key)
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                })
                .unwrap_or_default()
                .to_owned();
            let installed_flag = if project_type == "plugin" {
                installed_plugins.contains_key(&project_id)
            } else {
                installed.contains(&project_id)
            };
            let file_name = installed_plugins
                .get(&project_id)
                .and_then(|item| item.get("file_name").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    object
                        .get("file_name")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            object.insert("icon_url".to_owned(), json!(icon_url));
            object.insert("installed".to_owned(), json!(installed_flag));
            object.insert("file_name".to_owned(), json!(file_name));
            object.insert("catalog_id".to_owned(), json!(catalog_id));
            value
        })
        .collect();
    Ok(json!({
        "results": results,
        "page": page,
        "page_size": page_size,
        "total": total,
        "has_more": has_more,
        "instance_name": instance_name,
        "game_version": game_version,
        "loader": loader.unwrap_or_else(|| "vanilla".to_owned()),
        "catalog_id": catalog_id,
        "server_core": server_core,
    }))
}

fn explicit_mod_context(request: &Value) -> Result<(String, Option<String>), String> {
    let game_version = request_str(request, "game_version");
    if game_version.is_empty() {
        return Err("Укажите версию Minecraft для поиска".to_owned());
    }
    let loader = request_str(request, "loader").to_lowercase();
    Ok((game_version, if loader.is_empty() || loader == "vanilla" { None } else { Some(loader) }))
}

pub fn mods_details(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let project_type = request
        .get("project_type")
        .and_then(Value::as_str)
        .unwrap_or("mod")
        .trim()
        .to_lowercase();
    let store = InstanceStore::new(&config)?;
    let instance_name = request_str(request, "instance_name");
    let mut info: Option<InstanceInfo> = None;
    let (game_version, loader) = if !instance_name.is_empty() {
        let item = store.get(&instance_name)?;
        let context = instance_mod_context(&item)?;
        let kind = item.kind.trim().to_lowercase();
        if project_type == "plugin" && kind != "server" {
            return Err("Каталог плагинов доступен только для серверных экземпляров".to_owned());
        }
        if project_type != "plugin" && kind == "server" {
            return Err("Каталог ресурсов доступен только для клиентских экземпляров".to_owned());
        }
        info = Some(item);
        context
    } else {
        let game_version = request_str(request, "game_version");
        let loader = request_str(request, "loader").to_lowercase();
        (game_version, if loader.is_empty() || loader == "vanilla" { None } else { Some(loader) })
    };
    let project_id = request_str(request, "project_id");
    let http = client_from_settings(
        config.settings(),
        request.get("operation_id").and_then(Value::as_str).unwrap_or_default(),
    )?;
    if project_type == "plugin" {
        let catalog = plugin_catalog_for(&config, &request_str(request, "catalog_id"))?;
        let requested_core = request_str(request, "server_core");
        let core = if !requested_core.is_empty() {
            requested_core.to_lowercase()
        } else {
            info.as_ref()
                .map(|item| item.server_core.trim().to_lowercase())
                .unwrap_or_default()
        };
        let core = if core.is_empty() { "paper".to_owned() } else { core };
        let details = PluginCatalog::new(http, &catalog).details(&project_id, &core, &game_version)?;
        let mut payload = match details {
            Value::Object(map) => map,
            other => {
                let mut map = Map::new();
                map.insert("details".to_owned(), other);
                map
            }
        };
        payload.insert("project_type".to_owned(), json!("plugin"));
        return Ok(json!({
            "details": Value::Object(payload),
            "compatible_versions": [],
            "catalog_id": catalog_field(&catalog, "id", "modrinth"),
        }));
    }
    let catalog = resource_catalog(&config, &request_str(request, "catalog_id"))?;
    let catalog_id = catalog_field(&catalog, "id", "modrinth");
    if project_type == "modpack" {
        let details = ModpackProvider::new(http, &catalog)?.project_details(&project_id)?;
        return Ok(json!({
            "details": details,
            "catalog_id": catalog_id,
            "compatible_versions": [],
        }));
    }
    let kind = catalog_field(&catalog, "kind", "modrinth");
    if kind == "ftb" || kind == "technic" {
        return Err("Этот каталог поддерживает только подробности модпаков".to_owned());
    }
    if available() && NATIVE_PROJECT_TYPES.contains(&project_type.as_str()) {
        let mut payload = Map::new();
        payload.insert("cmd".to_owned(), json!("details"));
        payload.insert("project_id".to_owned(), json!(project_id));
        payload.insert("game_version".to_owned(), json!(game_version));
        payload.insert("loader".to_owned(), json!(loader.clone().unwrap_or_default()));
        payload.insert("project_type".to_owned(), json!(project_type));
        payload.insert("api_url".to_owned(), json!(catalog_field(&catalog, "api_url", "")));
        let data = modcat_call(&Value::Object(payload))?;
        let versions = data
            .get("versions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let compatible_versions: Vec<Value> = versions
            .iter()
            .map(|version| {
                json!({
                    "id": version.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "version_number": version.get("version_number").and_then(Value::as_str).unwrap_or_default(),
                    "name": version.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "date_published": version.get("date_published").and_then(Value::as_str).unwrap_or_default(),
                    "game_versions": version.get("game_versions").cloned().unwrap_or(json!([])),
                    "loaders": version.get("loaders").cloned().unwrap_or(json!([])),
                })
            })
            .collect();
        return Ok(json!({
            "details": data.get("details").cloned().unwrap_or(Value::Null),
            "compatible_versions": compatible_versions,
            "catalog_id": catalog_id,
        }));
    }
    let provider = ModrinthProvider::new(http, &catalog_field(&catalog, "api_url", ""), &catalog_id);
    let details = provider.project_details(&project_id, &project_type)?;
    let versions = provider.compatible_versions(&project_id, &game_version, loader.as_deref(), &project_type)?;
    let compatible_versions: Vec<Value> = versions
        .iter()
        .map(|version| {
            json!({
                "id": version.get("id").and_then(Value::as_str).unwrap_or_default(),
                "version_number": version.get("version_number").and_then(Value::as_str).unwrap_or_default(),
                "name": version.get("name").and_then(Value::as_str).unwrap_or_default(),
                "date_published": version.get("date_published").and_then(Value::as_str).unwrap_or_default(),
                "game_versions": version.get("game_versions").cloned().unwrap_or(json!([])),
                "loaders": version.get("loaders").cloned().unwrap_or(json!([])),
            })
        })
        .collect();
    Ok(json!({
        "details": details,
        "compatible_versions": compatible_versions,
        "catalog_id": catalog_id,
    }))
}

pub fn mods_check_updates(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request_str(request, "instance_name");
    let project_type = request_str(request, "project_type").to_lowercase();
    if !["mod", "resourcepack", "shader", "datapack"].contains(&project_type.as_str()) {
        return Err("Проверка обновлений доступна для модов, ресурспаков, шейдеров и датапаков".to_owned());
    }
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.kind.trim().eq_ignore_ascii_case("server") {
        return Err("Выберите клиентский экземпляр для проверки ресурсов".to_owned());
    }
    let (game_version, loader) = instance_mod_context(&info)?;
    let requested_catalog_id = request_str(request, "catalog_id");
    let settings = config.settings();
    let empty = Vec::new();
    let all_catalogs = settings
        .get("resource_catalogs")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .clone();
    let catalogs: Vec<Value> = if !requested_catalog_id.is_empty() {
        let catalog = resource_catalog(&config, &requested_catalog_id)?;
        vec![catalog]
    } else {
        all_catalogs
            .iter()
            .filter(|item| item.get("kind").and_then(Value::as_str).unwrap_or("modrinth") == "modrinth")
            .cloned()
            .collect()
    };
    if catalogs.is_empty() || catalogs.iter().any(|item| item.get("kind").and_then(Value::as_str) != Some("modrinth")) {
        return Err("Проверка обновлений доступна только для установок Modrinth".to_owned());
    }
    let mut updates: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let http = client_from_settings(
        config.settings(),
        request.get("operation_id").and_then(Value::as_str).unwrap_or_default(),
    )?;
    if available() && project_type != "datapack" {
        let directory = info.path.join(project_directory(&project_type).unwrap_or_default());
        for catalog in &catalogs {
            let catalog_id = catalog_field(catalog, "id", "modrinth");
            let catalog_url = catalog_field(catalog, "api_url", "");
            let catalog_name = catalog_field(catalog, "name", &catalog_id);
            let sidecars = scan_trusted_sidecars(&directory, &project_type, &catalog_id, &catalog_url);
            if sidecars.is_empty() {
                continue;
            }
            let mut payload = Map::new();
            payload.insert("cmd".to_owned(), json!("check"));
            payload.insert("game_version".to_owned(), json!(game_version));
            payload.insert("loader".to_owned(), json!(loader.clone().unwrap_or_default()));
            payload.insert("project_type".to_owned(), json!(project_type));
            payload.insert("api_url".to_owned(), json!(catalog_url));
            let projects: Vec<Value> = sidecars
                .iter()
                .map(|sidecar| json!({ "project_id": sidecar.project_id, "version_id": sidecar.version_id }))
                .collect();
            payload.insert("projects".to_owned(), Value::Array(projects));
            let data = match modcat_call(&Value::Object(payload)) {
                Ok(data) => data,
                Err(error) => {
                    errors.push(format!("{catalog_name}: {error}"));
                    continue;
                }
            };
            let outcomes = data.as_array().cloned().unwrap_or_default();
            for (sidecar, outcome) in sidecars.iter().zip(outcomes.iter()) {
                if !outcome.get("type_ok").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
                if let Some(error) = outcome.get("error").and_then(Value::as_str) {
                    if !error.is_empty() {
                        continue;
                    }
                }
                let Some(latest) = outcome.get("latest").filter(|value| value.is_object()) else {
                    continue;
                };
                let latest_version_id = latest
                    .get("id")
                    .or_else(|| latest.get("version_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if latest_version_id.is_empty() || latest_version_id == sidecar.version_id {
                    continue;
                }
                let name_field = if !sidecar.display_name.is_empty() {
                    sidecar.display_name.clone()
                } else if !sidecar.name.is_empty() {
                    sidecar.name.clone()
                } else {
                    sidecar.project_id.clone()
                };
                updates.push(json!({
                    "source": "Modrinth",
                    "catalog": "modrinth",
                    "catalog_id": catalog_id,
                    "project_id": sidecar.project_id,
                    "project_type": project_type,
                    "name": name_field,
                    "current_version": sidecar.version,
                    "current_version_id": sidecar.version_id,
                    "version": latest.get("version_number").and_then(Value::as_str).unwrap_or_default(),
                    "version_id": latest_version_id,
                    "game_version": game_version,
                    "loader": loader.clone().unwrap_or_else(|| "vanilla".to_owned()),
                    "path": sidecar.target.to_string_lossy(),
                }));
            }
        }
    } else {
        let workspace = InstanceWorkspace::new()?;
        let worlds = if project_type == "datapack" {
            workspace.list_worlds(&name)?
        } else {
            Vec::new()
        };
        for catalog in &catalogs {
            let catalog_id = catalog_field(catalog, "id", "modrinth");
            let catalog_url = catalog_field(catalog, "api_url", "");
            let catalog_name = catalog_field(catalog, "name", &catalog_id);
            let provider = ModrinthProvider::new(http.clone(), &catalog_url, &catalog_id);
            let outcome: Result<Vec<Value>, String> = if project_type == "datapack" {
                let mut collected: Vec<Value> = Vec::new();
                for world in &worlds {
                    let world_id = world.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
                    if world_id.is_empty() {
                        continue;
                    }
                    let target = workspace.world_directory(&name, &world_id)?;
                    let items =
                        provider.check_updates(&target, &game_version, loader.as_deref(), "datapack", Some(&target))?;
                    let world_title = world
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or(&world_id)
                        .to_owned();
                    for mut item in items {
                        if let Some(object) = item.as_object_mut() {
                            object.insert("world_name".to_owned(), json!(world_id));
                            object.insert("world_title".to_owned(), json!(world_title));
                        }
                        collected.push(item);
                    }
                }
                Ok(collected)
            } else {
                provider.check_updates(&info.path, &game_version, loader.as_deref(), &project_type, None)
            };
            match outcome {
                Ok(items) => updates.extend(items),
                Err(error) => errors.push(format!("{catalog_name}: {error}")),
            }
        }
    }
    let active_catalog_id = settings
        .get("active_catalog_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let result_catalog_id = if !requested_catalog_id.is_empty() {
        catalog_field(&catalogs[0], "id", "modrinth")
    } else {
        match catalogs.iter().find(|item| item.get("id").and_then(Value::as_str) == Some(active_catalog_id.as_str())) {
            Some(item) => catalog_field(item, "id", "modrinth"),
            None => catalog_field(&catalogs[0], "id", "modrinth"),
        }
    };
    Ok(json!({
        "updates": updates,
        "errors": errors,
        "project_type": project_type,
        "instance_name": name,
        "catalog_id": result_catalog_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-modcat-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn catalog_page_and_limit_request_fields_keep_zero_based_pagination() {
        let request = json!({ "page": "4", "limit": 30 });
        let page = parse_bounded_int(&request, "page", 0, 0, 500).unwrap();
        let page_size = parse_bounded_int(&request, "limit", 12, 1, 30).unwrap();
        assert_eq!((page, page_size, page * page_size), (4, 30, 120));
        assert_eq!(
            parse_bounded_int(&json!({ "page": 999 }), "page", 0, 0, 500).unwrap(),
            500
        );
    }

    #[test]
    fn sidecar_scan_filters_untrusted_metadata() {
        let dir = tmpdir("scan");
        let target = dir.join("sodium.jar");
        fs::write(&target, b"jar").unwrap();
        let sidecar = dir.join(".sodium.jar.trial-project.json");
        fs::write(
            &sidecar,
            r#"{"source":"modrinth","catalog":"modrinth","catalog_id":"modrinth","catalog_url":"https://api.modrinth.com/v2","project_type":"mod","project_id":"AANobbMI","version_id":"v1","version":"0.5","display_name":"Sodium","game_version":"1.21.1","loader":"fabric"}"#,
        )
        .unwrap();
        // чужой источник
        fs::write(dir.join(".other.jar.trial-project.json"), r#"{"source":"curseforge","catalog":"modrinth","catalog_id":"modrinth","project_id":"X","version_id":"v9","project_type":"mod","game_version":"1.21.1","loader":"fabric"}"#).unwrap();
        let entries = scan_trusted_sidecars(&dir, "mod", "modrinth", "https://api.modrinth.com/v2");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project_id, "AANobbMI");
        assert_eq!(entries[0].version_id, "v1");
        assert_eq!(entries[0].target, target);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn category_is_only_supported_for_modrinth_mod_projects() {
        assert!(modrinth_category_supported("mod", "modrinth"));
        for project_type in ["resourcepack", "shader", "datapack", "plugin", "modpack"] {
            assert!(!modrinth_category_supported(project_type, "modrinth"));
        }
        assert!(!modrinth_category_supported("mod", "custom"));
    }

    #[test]
    fn catalog_url_match_is_case_insensitive() {
        let dir = tmpdir("case");
        fs::write(dir.join("x.jar"), b"x").unwrap();
        fs::write(
            dir.join(".x.jar.trial-project.json"),
            r#"{"source":"Modrinth","catalog":"Modrinth","catalog_id":"modrinth","catalog_url":"https://API.modrinth.com/v2","project_type":"mod","project_id":"P","version_id":"v","game_version":"1.21.1","loader":"fabric"}"#,
        )
        .unwrap();
        let entries = scan_trusted_sidecars(&dir, "mod", "modrinth", "https://api.modrinth.com/v2");
        assert_eq!(entries.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
