//! установка и обновление игровых ресурсов

use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

use super::config::ConfigStore;
use super::events::emit_status_event;
use super::instance_workspace::InstanceWorkspace;
use super::instances::{InstanceInfo, InstanceStore};
use super::modcat::{instance_mod_context, resource_catalog};
use super::modpacks::ModpackProvider;
use super::mods::ModrinthProvider;
use super::net::{client_from_settings, HttpClient};
use super::plugin_catalogs::{plugin_catalog_for, PluginCatalog};
use super::safe_update;
use super::server::InstanceLock;
use super::snapshots;

fn request_str<'a>(request: &'a Value, key: &str) -> &'a str {
    request.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn flag(request: &Value, key: &str) -> bool {
    request.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn http_client(config: &ConfigStore, request: &Value) -> Result<Arc<HttpClient>, String> {
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    client_from_settings(config.settings(), operation_id)
}

fn catalog_field(catalog: &Value, key: &str, fallback: &str) -> String {
    catalog
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn instance_loader_version(info: &InstanceInfo) -> String {
    let loader = info.loader.trim().to_lowercase();
    if !matches!(loader.as_str(), "fabric" | "forge" | "neoforge" | "quilt") {
        return String::new();
    }
    super::instances::loader_version_of(&info.path, &info.version_id, &info.minecraft_version, &info.loader)
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn snapshot_map(snapshots: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (category, id) in snapshots {
        map.insert(category.clone(), json!(id));
    }
    Value::Object(map)
}

// ── mods_update: обновление одного ресурса Modrinth ─────────────────

pub fn mods_update(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request_str(request, "instance_name").trim().to_owned();
    let project_type = request_str(request, "project_type").trim().to_lowercase();
    if !matches!(project_type.as_str(), "mod" | "resourcepack" | "shader" | "datapack") {
        return Err("Обновление доступно только для ресурсов Modrinth".to_owned());
    }
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.kind.trim().eq_ignore_ascii_case("server") {
        return Err("Выберите клиентский экземпляр для обновления ресурса".to_owned());
    }
    let (game_version, loader) = instance_mod_context(&info)?;
    let catalog = resource_catalog(&config, request_str(request, "catalog_id"))?;
    if catalog.get("kind").and_then(Value::as_str) != Some("modrinth") {
        return Err("Обновление доступно только для установок Modrinth".to_owned());
    }
    let world_id = request_str(request, "world_name").trim().to_owned();
    let workspace = InstanceWorkspace::new()?;
    let target: Option<PathBuf> = if project_type == "datapack" {
        Some(workspace.world_directory(&name, &world_id)?)
    } else {
        None
    };
    let http = http_client(&config, request)?;
    let provider = ModrinthProvider::new(
        http,
        &catalog_field(&catalog, "api_url", ""),
        &catalog_field(&catalog, "id", "modrinth"),
    );
    let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let result = provider.install(
        request_str(request, "project_id"),
        &info.path,
        &game_version,
        loader.as_deref(),
        &project_type,
        target.as_deref(),
        flag(request, "confirm_unverified"),
        non_empty(request_str(request, "version_id")),
        true,
    )?;
    emit_status_event(
        "workspace-changed",
        &format!("Ресурс «{}» обновлён.", result.name),
    );
    Ok(json!({
        "project_id": result.project_id,
        "name": result.name,
        "version": result.version,
        "version_id": result.version_id,
        "file_name": result.file_name,
        "dependencies": result.dependencies,
        "details": workspace.snapshot(&name)?,
        "instance_name": name,
    }))
}

// ── безопасное обновление: снапшот → обновление → откат ─────────────

pub fn mods_update_all_safe(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request_str(request, "instance_name").trim().to_owned();
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.kind.trim().eq_ignore_ascii_case("server") {
        return Err("Выберите клиентский экземпляр для обновления ресурсов".to_owned());
    }
    if info.running {
        return Err("Остановите экземпляр перед обновлением".to_owned());
    }
    let (game_version, loader) = instance_mod_context(&info)?;
    let project_types: Vec<String> = match request.get("project_types") {
        Some(Value::Array(rows)) => rows
            .iter()
            .filter_map(Value::as_str)
            .map(|item| item.trim().to_lowercase())
            .filter(|item| safe_update::PROJECT_TYPES.contains(&item.as_str()))
            .collect(),
        _ => vec!["mod".to_owned()],
    };
    if project_types.is_empty() {
        return Err("Выберите хотя бы один тип ресурсов: моды, ресурспаки или шейдеры".to_owned());
    }
    let catalogs: Vec<Value> = {
        let settings = config.settings();
        let empty = Vec::new();
        settings
            .get("resource_catalogs")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
            .iter()
            .filter(|item| item.get("kind").and_then(Value::as_str) == Some("modrinth"))
            .cloned()
            .collect()
    };
    if catalogs.is_empty() {
        return Err("Безопасное обновление доступно только для установок Modrinth".to_owned());
    }
    let http = http_client(&config, request)?;
    let mut updates: Vec<Map<String, Value>> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut last_catalog_id = String::new();
    for catalog in &catalogs {
        let catalog_id = catalog_field(catalog, "id", "modrinth");
        let catalog_name = catalog_field(catalog, "name", &catalog_id);
        last_catalog_id = catalog_id.clone();
        let provider = ModrinthProvider::new(
            http.clone(),
            &catalog_field(catalog, "api_url", ""),
            &catalog_id,
        );
        for project_type in &project_types {
            match provider.check_updates(&info.path, &game_version, loader.as_deref(), project_type, None) {
                Ok(rows) => updates.extend(rows.into_iter().filter_map(|item| match item {
                    Value::Object(map) => Some(map),
                    _ => None,
                })),
                Err(error) => errors.push(format!("{catalog_name}: {error}")),
            }
        }
    }
    let pending: Vec<Map<String, Value>> = updates
        .into_iter()
        .filter(|item| item.get("project_id").and_then(Value::as_str).unwrap_or_default().trim().len() > 0)
        .map(|item| {
            let project_id = item.get("project_id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let fallback_name = if project_id.is_empty() { String::new() } else { project_id.clone() };
            let name_value = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or(fallback_name);
            let project_type = item
                .get("project_type")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("mod")
                .to_owned();
            let catalog_id = item
                .get("catalog_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| last_catalog_id.clone());
            json!({
                "project_id": project_id,
                "version_id": item.get("version_id").and_then(Value::as_str).unwrap_or_default(),
                "name": name_value,
                "project_type": project_type,
                "catalog_id": catalog_id,
            })
            .as_object()
            .cloned()
            .unwrap_or_default()
        })
        .collect();

    let workspace = InstanceWorkspace::new()?;
    let apply_one = |update: &Map<String, Value>| -> Result<Map<String, Value>, String> {
        let catalog =
            resource_catalog(&config, update.get("catalog_id").and_then(Value::as_str).unwrap_or_default())?;
        if catalog.get("kind").and_then(Value::as_str) != Some("modrinth") {
            return Err("Обновление доступно только для установок Modrinth".to_owned());
        }
        let provider = ModrinthProvider::new(
            http.clone(),
            &catalog_field(&catalog, "api_url", ""),
            &catalog_field(&catalog, "id", "modrinth"),
        );
        let result = provider.install(
            update.get("project_id").and_then(Value::as_str).unwrap_or_default(),
            &info.path,
            &game_version,
            loader.as_deref(),
            update.get("project_type").and_then(Value::as_str).unwrap_or("mod"),
            None,
            false,
            non_empty(update.get("version_id").and_then(Value::as_str).unwrap_or_default()),
            true,
        )?;
        Ok(json!({
            "project_id": result.project_id,
            "name": result.name,
            "version": result.version,
            "version_id": result.version_id,
        })
        .as_object()
        .cloned()
        .unwrap_or_default())
    };
    let create_snapshot = |category: &str| -> Result<Map<String, Value>, String> {
        snapshots::store_for(&name)?.create(category, 168, 20)
    };
    let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let outcome = safe_update::run_safe_update(
        &info.path,
        &pending,
        apply_one,
        create_snapshot,
        |event, message| emit_status_event(event, message),
    )?;
    emit_status_event(
        "workspace-changed",
        &format!(
            "Безопасное обновление: применено {}, ошибок {}.",
            outcome.applied.len(),
            outcome.failed.len()
        ),
    );
    Ok(json!({
        "session": outcome.session,
        "applied": outcome.applied,
        "failed": outcome.failed,
        "snapshots": snapshot_map(&outcome.snapshots),
        "errors": errors,
        "sessions": safe_update::list_sessions(&info.path),
        "details": workspace.snapshot(&name)?,
        "instance_name": name,
    }))
}

pub fn mods_update_rollback(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request_str(request, "instance_name").trim().to_owned();
    let session_id = request_str(request, "session_id").trim().to_owned();
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.running {
        return Err("Остановите экземпляр перед откатом".to_owned());
    }
    let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let outcome = safe_update::rollback(&info.path, &session_id, |category, snapshot_id| {
        snapshots::store_for(&name)?.restore(category, snapshot_id)
    })?;
    emit_status_event("workspace-changed", "Обновления откачены до снапшота.");
    let workspace = InstanceWorkspace::new()?;
    Ok(json!({
        "session_id": outcome.session_id,
        "restored": outcome.restored,
        "errors": outcome.errors,
        "sessions": safe_update::list_sessions(&info.path),
        "details": workspace.snapshot(&name)?,
    }))
}

pub fn mods_update_sessions(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request_str(request, "instance_name").trim().to_owned();
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    Ok(json!({
        "sessions": safe_update::list_sessions(&info.path),
        "instance_name": name,
    }))
}

// ── mods_install: установка ресурса/модпака/плагина ─────────────────

pub fn mods_install(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let name = request_str(request, "instance_name").trim().to_owned();
    let project_type = request_str(request, "project_type").trim().to_lowercase();
    let http = http_client(&config, request)?;
    let store = InstanceStore::new(&config)?;

    // предпросмотр зависимостей модпака без установки
    if project_type == "modpack" && flag(request, "preflight_only") {
        let catalog = resource_catalog(&config, request_str(request, "catalog_id"))?;
        let provider = ModpackProvider::new(http, &catalog)?;
        let dependencies = provider.preflight(
            request_str(request, "project_id"),
            request_str(request, "game_version").trim(),
            request_str(request, "version"),
            &request_str(request, "loader").trim().to_lowercase(),
            non_empty(request_str(request, "loader_version")),
            flag(request, "confirm_unverified"),
        )?;
        return Ok(json!({ "preflight": true, "dependencies": dependencies }));
    }

    let info = store.get(&name)?;
    let workspace = InstanceWorkspace::new()?;

    if project_type == "plugin" {
        if !info.kind.trim().eq_ignore_ascii_case("server") {
            return Err("Плагины можно устанавливать только в серверные экземпляры".to_owned());
        }
        let (game_version, _) = instance_mod_context(&info)?;
        let catalog = plugin_catalog_for(&config, request_str(request, "catalog_id"))?;
        let core = if info.server_core.trim().is_empty() {
            "paper".to_owned()
        } else {
            info.server_core.trim().to_owned()
        };
        let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
        let result = PluginCatalog::new(http, &catalog).install(
            request_str(request, "project_id"),
            &info.path.join("plugins"),
            &game_version,
            &core,
            flag(request, "confirm_unverified"),
        )?;
        emit_status_event("workspace-changed", "Плагин установлен в сервер.");
        let mut payload = result;
        let object = payload
            .as_object_mut()
            .ok_or_else(|| "Каталог вернул некорректный результат установки".to_owned())?;
        object.insert("catalog_id".to_owned(), json!(catalog_field(&catalog, "id", "")));
        object.insert("instance_name".to_owned(), json!(name));
        return Ok(payload);
    }

    if info.kind.trim().eq_ignore_ascii_case("server") {
        return Err("Ресурсы можно устанавливать только в клиентские экземпляры".to_owned());
    }
    let (game_version, loader) = instance_mod_context(&info)?;
    let catalog = resource_catalog(&config, request_str(request, "catalog_id"))?;
    let catalog_id = catalog_field(&catalog, "id", "modrinth");

    if project_type == "modpack" {
        let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
        let provider = ModpackProvider::new(http, &catalog)?;
        let result = provider.install(
            request_str(request, "project_id"),
            &info.path,
            &game_version,
            request_str(request, "version"),
            request_str(request, "display_name"),
            loader.as_deref().unwrap_or("vanilla"),
            non_empty(&instance_loader_version(&info)),
            flag(request, "confirm_unverified"),
        )?;
        emit_status_event("workspace-changed", "Ресурс установлен.");
        let mut payload = Value::Object(result);
        let object = payload.as_object_mut().expect("install возвращает объект");
        object.insert("catalog_id".to_owned(), json!(catalog_id));
        object.insert("details".to_owned(), workspace.snapshot(&name)?);
        return Ok(payload);
    }

    if matches!(
        catalog.get("kind").and_then(Value::as_str).unwrap_or("modrinth"),
        "ftb" | "technic"
    ) {
        return Err("Каталоги FTB и Technic предназначены для установки модпаков".to_owned());
    }
    let provider = ModrinthProvider::new(http, &catalog_field(&catalog, "api_url", ""), &catalog_id);
    let _lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let target_dir: Option<PathBuf> = if project_type == "datapack" {
        Some(workspace.world_directory(&name, request_str(request, "world_name").trim())?)
    } else {
        None
    };
    let result = provider.install(
        request_str(request, "project_id"),
        &info.path,
        &game_version,
        loader.as_deref(),
        &project_type,
        target_dir.as_deref(),
        flag(request, "confirm_unverified"),
        non_empty(request_str(request, "version_id")),
        false,
    )?;
    emit_status_event("workspace-changed", "Ресурс установлен.");
    Ok(json!({
        "source": result.source,
        "project_id": result.project_id,
        "name": result.name,
        "version": result.version,
        "version_id": result.version_id,
        "file_name": result.file_name,
        "dependencies": result.dependencies,
        "path": result
            .path
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default(),
        "catalog_id": catalog_id,
        "details": workspace.snapshot(&name)?,
    }))
}
