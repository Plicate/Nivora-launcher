//! показывает и настраивает версии Minecraft, Java, загрузчика и библиотек LWJGL
use base64::Engine;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

use super::config::ConfigStore;
use super::instances::InstanceStore;
use super::instance_workspace::InstanceWorkspace;

const MAX_VERSION_JSON_BYTES: usize = 4 * 1024 * 1024;
const LWJGL_VERSIONS: [&str; 5] = ["3.3.6", "3.3.5", "3.3.4", "3.3.3", "3.3.2"];

fn read_version_json(path: &Path) -> Value {
    if path.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
        return json!({});
    }
    let Ok(text) = fs::read_to_string(path) else {
        return json!({});
    };
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().is_none_or(Map::is_empty)
}

/// возвращает состав установленных версий для вкладки "Версии"
pub(crate) fn instance_versions(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.game_edition == "bedrock" {
        return Err("Версии доступны только для Java-экземпляров".to_owned());
    }
    if info.kind == "server" {
        return Err("Версии доступны только для клиентских экземпляров".to_owned());
    }
    let versions_dir = info.path.join("versions");
    let mut version_json: Value = json!({});
    let mut version_id = info.version_id.trim().to_owned();
    if !version_id.is_empty() {
        let metadata_path = versions_dir.join(&version_id).join(format!("{version_id}.json"));
        version_json = read_version_json(&metadata_path);
        if version_json.is_object() {
            let parent_id = version_json
                .get("inheritsFrom")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if !parent_id.is_empty() && parent_id != version_id {
                // профили Forge/NeoForge/Fabric наследуют библиотеки ванили
                let parent_path = versions_dir.join(&parent_id).join(format!("{parent_id}.json"));
                let mut parent_json = read_version_json(&parent_path);
                if is_empty_object(&parent_json) {
                    let http = super::net::client_from_settings(config.settings(), "")?;
                    if let Ok(mut repository) = super::metadata::VersionRepository::new(&config, Some(http)) {
                        if let Ok(resolved) = repository.resolve(&parent_id) {
                            parent_json = resolved;
                        }
                    }
                }
                if !is_empty_object(&parent_json) {
                    version_json = super::metadata::merge_version_json(&parent_json, &version_json);
                }
            }
        }
    }
    if is_empty_object(&version_json) && versions_dir.is_dir() {
        // свежий экземпляр без version_id: берём самый свежий */*.json
        let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf, String)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&versions_dir) {
            for entry in entries.flatten() {
                let directory = entry.path();
                if !directory.is_dir() {
                    continue;
                }
                if let Ok(inner) = fs::read_dir(&directory) {
                    for file in inner.flatten() {
                        let path = file.path();
                        if path.is_symlink() || !path.is_file() {
                            continue;
                        }
                        if path.extension().map(|value| value == "json").unwrap_or(false) {
                            if let Ok(modified) = file.metadata().and_then(|meta| meta.modified()) {
                                let stem = path.file_stem().map(|value| value.to_string_lossy().to_string()).unwrap_or_default();
                                candidates.push((modified, path, stem));
                            }
                        }
                    }
                }
            }
        }
        if let Some((_, metadata_path, stem)) = candidates.into_iter().max_by_key(|(modified, _, _)| *modified) {
            version_id = stem;
            version_json = read_version_json(&metadata_path);
        }
    }
    if !version_json.is_object() {
        version_json = json!({});
    }
    let mut components: Vec<Value> = Vec::new();
    let minecraft_version = {
        let stored = info.minecraft_version.trim().to_owned();
        if stored.is_empty() { version_id.clone() } else { stored }
    };
    components.push(json!({
        "id": "minecraft",
        "title": "Minecraft",
        "version": minecraft_version,
        "meta": if version_id.is_empty() { "версия не установлена".to_owned() } else { format!("профиль версии: {version_id}") },
        "changeable": true,
        "removable": false,
    }));
    let java_major = version_json
        .get("javaVersion")
        .filter(|value| value.is_object())
        .and_then(|value| value.get("majorVersion"))
        .cloned()
        .unwrap_or(Value::Null);
    let java_version = match &java_major {
        Value::Null => "не указана в метаданных".to_owned(),
        other => other.to_string().trim_matches('"').to_owned(),
    };
    components.push(json!({
        "id": "java",
        "title": "Java (требуется)",
        "version": java_version,
        "meta": "версия из метаданных Mojang",
        "changeable": false,
        "removable": false,
    }));
    let loader = {
        let value = info.loader.trim().to_lowercase();
        if value.is_empty() { "vanilla".to_owned() } else { value }
    };
    let loader_title: String = match loader.as_str() {
        "vanilla" => "Vanilla".to_owned(),
        "fabric" => "Fabric Loader".to_owned(),
        "quilt" => "Quilt Loader".to_owned(),
        "forge" => "Forge".to_owned(),
        "neoforge" => "NeoForge".to_owned(),
        // незнакомый загрузчик показываем без изменений
        other => other.to_owned(),
    };
    let loader_version = super::instances::loader_version_of(&info.path, &info.version_id, &info.minecraft_version, &loader);
    components.push(json!({
        "id": "loader",
        "title": loader_title,
        "version": if !loader_version.is_empty() { loader_version.clone() } else if loader == "vanilla" { "—".to_owned() } else { "не определена".to_owned() },
        "meta": "модлоадер экземпляра",
        "changeable": true,
        "removable": loader != "vanilla",
    }));
    let workspace = InstanceWorkspace::new().ok();
    let settings = workspace
        .as_ref()
        .and_then(|workspace| workspace.settings(&info.name).ok())
        .unwrap_or_else(|| json!({}));
    let overrides = settings
        .get("library_overrides")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut lwjgl_version = String::new();
    if let Some(libraries) = version_json.get("libraries").and_then(Value::as_array) {
        for library in libraries {
            let Some(name) = library.get("name").and_then(Value::as_str) else { continue };
            let parts: Vec<&str> = name.split(':').collect();
            if parts.len() >= 3 && parts[0] == "org.lwjgl" && parts[1] == "lwjgl" {
                lwjgl_version = parts[2].to_owned();
                break;
            }
        }
    }
    if lwjgl_version.is_empty() {
        lwjgl_version = "3.3.x (в версии не указана)".to_owned();
    }
    let overridden = overrides
        .get("org.lwjgl:lwjgl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    components.push(json!({
        "id": "lwjgl",
        "title": "LWJGL 3",
        "version": lwjgl_version,
        "meta": if overridden.is_empty() { "графика/окно/ввод, из version JSON".to_owned() } else { format!("переопределено на {overridden} · Maven Central") },
        "changeable": true,
        "removable": !overridden.is_empty(),
        "overridden": overridden,
    }));
    let asset_index = version_json
        .get("assets")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            version_json
                .get("assetIndex")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    components.push(json!({
        "id": "assets",
        "title": "Индекс ассетов",
        "version": if asset_index.is_empty() { "—".to_owned() } else { asset_index },
        "meta": "звуки, текстуры и шрифты игры",
        "changeable": false,
        "removable": false,
    }));
    Ok(json!({
        "components": components,
        "minecraft_version": minecraft_version,
        "loader": loader,
        "loader_version": loader_version,
        "lwjgl_versions": LWJGL_VERSIONS,
        "library_overrides": overrides,
    }))
}

/// сохраняет переопределения библиотек экземпляра, включая LWJGL
pub(crate) fn instance_version_override_save(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.game_edition == "bedrock" || info.kind == "server" {
        return Err("Версии библиотек доступны только для клиентских Java-экземпляров".to_owned());
    }
    if info.running {
        return Err("Остановите экземпляр перед сменой версии библиотеки".to_owned());
    }
    let Some(raw_overrides) = request.get("overrides") else {
        return Err("Не переданы оверрайды библиотек".to_owned());
    };
    let workspace = InstanceWorkspace::new()?;
    let mut values = workspace.settings(&name)?;
    if let Some(object) = values.as_object_mut() {
        object.insert("library_overrides".into(), raw_overrides.clone());
    }
    workspace.save_settings(&name, &values)?;
    let instances: Vec<Value> = store.list().iter().map(|item| item.public()).collect();
    Ok(json!({ "settings": workspace.settings(&name)?, "instances": instances }))
}

/// импортирует пользовательский JSON версии Minecraft
pub(crate) fn instance_version_import(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let file_name = request.get("file_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let content = request.get("data").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if file_name.is_empty() || content.is_empty() {
        return Err("Не передан файл версии".to_owned());
    }
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.game_edition == "bedrock" || info.kind == "server" {
        return Err("Свои версии доступны только для клиентских Java-экземпляров".to_owned());
    }
    if info.running {
        return Err("Остановите экземпляр перед установкой версии".to_owned());
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(content.as_bytes())
        .map_err(|error| format!("Файл версии повреждён: {error}"))?;
    if data.len() > MAX_VERSION_JSON_BYTES {
        return Err("Файл версии слишком большой (лимит 4 МиБ)".to_owned());
    }
    let version_json: Value = serde_json::from_slice(&data)
        .map_err(|error| format!("Не удалось прочитать JSON версии: {error}"))?;
    if !version_json.is_object()
        || version_json
            .get("mainClass")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        return Err("В JSON версии нет mainClass — это не файл версии Minecraft".to_owned());
    }
    let raw_id = version_json
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let custom_id = if raw_id.is_empty() {
        file_name.strip_suffix(".json").unwrap_or(&file_name).to_owned()
    } else {
        raw_id
    };
    let id_valid = !custom_id.is_empty()
        && !custom_id.contains("..")
        && custom_id
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '.' | '_' | '-'));
    if !id_valid {
        return Err("Некорректный идентификатор версии в JSON (поле id)".to_owned());
    }
    if let Some(inherits) = version_json.get("inheritsFrom").and_then(Value::as_str) {
        if inherits.trim() == custom_id {
            return Err("JSON версии наследуется сам от себя".to_owned());
        }
    }
    super::installer::safe_version_id(&custom_id)?;
    let version_dir = info.path.join("versions").join(&custom_id);
    if version_dir.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
        return Err("Каталог версии является символьной ссылкой".to_owned());
    }
    let _lock = super::server::InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    fs::create_dir_all(&version_dir).map_err(|_| "Не удалось создать каталог версии".to_owned())?;
    let mut text = serde_json::to_string_pretty(&version_json).unwrap_or_default();
    text.push('\n');
    fs::write(version_dir.join(format!("{custom_id}.json")), text)
        .map_err(|_| "Не удалось записать файл версии".to_owned())?;
    let http = super::net::client_from_settings(config.settings(), "")?;
    let mut installer = super::installer::MinecraftInstaller::new(&config, http)?;
    let result = installer.load_existing(&info.path, Some(&custom_id))?;
    Ok(json!({
        "version_id": result.version_id,
        "instance": store.get(&info.name)?.public(),
        "instances": store.list().iter().map(|item| item.public()).collect::<Vec<Value>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lwjgl_versions_match_frontend_preset() {
        assert_eq!(LWJGL_VERSIONS[0], "3.3.6");
        assert_eq!(LWJGL_VERSIONS.len(), 5);
    }

    #[test]
    fn versions_payload_requires_selection_and_edition() {
        let error = instance_versions(&json!({ "name": "" })).unwrap_err();
        assert_eq!(error, "Сначала выберите экземпляр");
    }

    #[test]
    fn version_import_validates_payload() {
        assert_eq!(
            instance_version_import(&json!({ "name": "x", "file_name": "", "data": "" })).unwrap_err(),
            "Не передан файл версии"
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"{\"id\":\"custom-1\"}");
        assert_eq!(
            instance_version_import(&json!({ "name": "", "file_name": "v.json", "data": encoded })).unwrap_err(),
            "Сначала выберите экземпляр"
        );
    }
}
