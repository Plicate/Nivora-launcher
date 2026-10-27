use super::*;

const FIX_ACTIONS: [&str; 12] = [
    "disable_mods",
    "install_dependencies",
    "set_memory",
    "set_java",
    "reset_jvm_arguments",
    "repair_version",
    "backup_configs",
    "set_graphics_mode",
    "update_loader",
    "set_account",
    "login_microsoft",
    "set_security",
];

/// выбирает стабильную версию загрузчика не ниже заданного минимума
pub(super) fn pick_loader_version(choices: &[String], minimum: &str) -> String {
    let mut values: Vec<String> = Vec::new();
    for item in choices {
        let text = item.trim();
        if !text.is_empty() && !values.iter().any(|value| value == text) {
            values.push(text.to_owned());
        }
    }
    if values.is_empty() {
        return String::new();
    }
    let minimum_key = if minimum.trim().is_empty() { Vec::new() } else { version_key(minimum).0 };
    let eligible: Vec<&String> = values
        .iter()
        .filter(|item| minimum_key.is_empty() || version_key(item).0 >= minimum_key)
        .collect();
    if eligible.is_empty() {
        return String::new();
    }
    let stable: Vec<&String> = eligible.iter().copied().filter(|item| !is_prerelease(item)).collect();
    let pool: Vec<&String> = if stable.is_empty() { eligible } else { stable };
    // при равных ключах сохраняем первый вариант
    let mut best: Option<&String> = None;
    for item in pool {
        if best.is_none_or(|current| version_key(item) > version_key(current)) {
            best = Some(item);
        }
    }
    best.map(|value| value.clone()).unwrap_or_default()
}

pub(super) fn string_list(value: Option<&Value>, limit: usize) -> Result<Vec<String>, String> {
    let Some(list) = value.filter(|item| item.is_array()) else {
        return Err("Параметры починки имеют неожиданный формат".to_owned());
    };
    let empty: Vec<Value> = Vec::new();
    Ok(list
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .take(limit)
        .filter_map(|item| {
            let text = item.as_str()?.trim().to_owned();
            if text.is_empty() { None } else { Some(text) }
        })
        .collect())
}

/// возвращает версию игры и загрузчик; для vanilla загрузчик равен `None`
fn mod_context(info: &super::instances::InstanceInfo) -> Result<(String, Option<String>), String> {
    let game_version = if info.minecraft_version.trim().is_empty() {
        info.version_id.trim().to_owned()
    } else {
        info.minecraft_version.trim().to_owned()
    };
    if game_version.is_empty() {
        return Err(format!("У экземпляра «{}» не определена версия Minecraft", info.name));
    }
    let loader = info.loader.trim().to_lowercase();
    let loader = if loader.is_empty() || loader == "vanilla" { None } else { Some(loader) };
    Ok((game_version, loader))
}

/// устанавливает зависимость из первого настроенного каталога Modrinth
fn install_mod(
    config: &super::config::ConfigStore,
    http: &std::sync::Arc<super::net::HttpClient>,
    info: &super::instances::InstanceInfo,
    project_id: &str,
) -> Result<(), String> {
    let settings = config.settings();
    let empty: Vec<Value> = Vec::new();
    let catalogs = settings.get("resource_catalogs").and_then(Value::as_array).unwrap_or(&empty);
    let catalog = catalogs
        .iter()
        .find(|item| item.get("kind").and_then(Value::as_str) == Some("modrinth"))
        .ok_or_else(|| "Каталог Modrinth не настроен".to_owned())?;
    let api_url = catalog.get("api_url").and_then(Value::as_str).unwrap_or_default();
    let catalog_id = catalog.get("id").and_then(Value::as_str).unwrap_or_default();
    let (game_version, loader) = mod_context(info)?;
    let provider = super::mods::ModrinthProvider::new(http.clone(), api_url, catalog_id);
    provider
        .install(project_id, &info.path, &game_version, loader.as_deref(), "mod", None, false, None, false)
        .map(|_| ())
}

/// восстанавливает версию и загрузчик, не изменяя `mods` и `config`
fn repair_instance_version(
    config: &super::config::ConfigStore,
    http: &std::sync::Arc<super::net::HttpClient>,
    info: &super::instances::InstanceInfo,
) -> Result<(), String> {
    let loader = {
        let value = info.loader.trim().to_lowercase();
        if value.is_empty() { "vanilla".to_owned() } else { value }
    };
    let minecraft_version = info.minecraft_version.trim();
    if minecraft_version.is_empty() {
        return Err(format!("У экземпляра «{}» не определена версия Minecraft", info.name));
    }
    let loader_version = {
        let resolved = super::instances::loader_version_of(&info.path, &info.version_id, &info.minecraft_version, &loader);
        if resolved.is_empty() { None } else { Some(resolved) }
    };
    let version_id = {
        let value = info.version_id.trim();
        if value.is_empty() { None } else { Some(value.to_owned()) }
    };
    let mut installer = super::installer::MinecraftInstaller::new(config, http.clone())?;
    installer
        .repair_version(&info.path, minecraft_version, &loader, loader_version.as_deref(), version_id.as_deref())
        .map(|_| ())
}

/// обновляет загрузчик до стабильной версии не ниже минимума
fn update_instance_loader(
    config: &super::config::ConfigStore,
    http: &std::sync::Arc<super::net::HttpClient>,
    info: &super::instances::InstanceInfo,
    minimum: &str,
) -> Result<String, String> {
    let loader = info.loader.trim().to_lowercase();
    if !matches!(loader.as_str(), "fabric" | "quilt" | "forge" | "neoforge") {
        return Err("У экземпляра нет модлоадера, который можно обновить".to_owned());
    }
    let minecraft_version = info.minecraft_version.trim();
    if minecraft_version.is_empty() {
        return Err(format!("У экземпляра «{}» не определена версия Minecraft", info.name));
    }
    let installer = super::installer::MinecraftInstaller::new(config, http.clone())?;
    let choices: Vec<String> = installer
        .loader_choices(&loader, minecraft_version)?
        .iter()
        .filter_map(|choice| choice.get("version").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let target = pick_loader_version(&choices, minimum);
    if target.is_empty() {
        let suffix = if minimum.is_empty() {
            "в каталоге загрузчика".to_owned()
        } else {
            format!("{minimum} или новее")
        };
        return Err(format!("Для Minecraft {minecraft_version} нет версии {loader} {suffix}"));
    }
    let old_version_id = info.version_id.trim().to_owned();
    let mut installer = installer;
    let result = installer.install(minecraft_version, &loader, Some(&target), Some(&info.name), false, false)?;
    let old_pattern = Regex::new(r"^[A-Za-z0-9._+-]+$").unwrap();
    if !old_version_id.is_empty()
        && old_version_id != result.version_id
        && old_version_id != minecraft_version
        && old_pattern.is_match(&old_version_id)
    {
        let old_dir = info.path.join("versions").join(&old_version_id);
        let _ = fs::remove_dir_all(&old_dir);
    }
    Ok(target)
}

fn apply_one(
    config: &super::config::ConfigStore,
    http: &std::sync::Arc<super::net::HttpClient>,
    info: &super::instances::InstanceInfo,
    action: &str,
    params: &Value,
) -> Result<String, String> {
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    let save = |values: Value| -> Result<(), String> {
        workspace.save_settings(&info.name, &values).map(|_| ())
    };
    match action {
        "disable_mods" => {
            let files = string_list(params.get("files"), MAX_FILES_PER_FIX)?;
            let mut done: Vec<String> = Vec::new();
            let mut skipped: Vec<String> = Vec::new();
            for name in files {
                // берём только файлы непосредственно в `mods`, без подкаталогов
                let plain = Path::new(&name).file_name().map(|value| value.to_string_lossy().to_string());
                if plain.as_deref() != Some(name.as_str()) || name.to_lowercase().ends_with(".disabled") {
                    skipped.push(name);
                    continue;
                }
                match workspace.toggle_mod(&info.name, &name, false) {
                    Ok(_) => done.push(name),
                    Err(_) => skipped.push(name),
                }
            }
            if done.is_empty() {
                let suffix = if skipped.is_empty() {
                    String::new()
                } else {
                    format!(": {}", skipped.iter().take(4).cloned().collect::<Vec<_>>().join(", "))
                };
                return Err(format!("Не удалось отключить ни одного мода{suffix}"));
            }
            let extra = if skipped.is_empty() {
                String::new()
            } else {
                format!("; пропущено: {}", skipped.iter().take(4).cloned().collect::<Vec<_>>().join(", "))
            };
            Ok(format!("Отключено модов: {}{extra}", done.len()))
        }
        "install_dependencies" => {
            let pattern = Regex::new(r"^[\w.\-]{2,64}$").unwrap();
            let ids: Vec<String> = string_list(params.get("ids"), MAX_FILES_PER_FIX)?
                .into_iter()
                .filter(|item| pattern.is_match(item))
                .collect();
            let mut installed: Vec<String> = Vec::new();
            let mut failed: Vec<String> = Vec::new();
            for project_id in ids {
                match install_mod(config, http, info, &project_id) {
                    Ok(()) => installed.push(project_id),
                    Err(_) => failed.push(project_id),
                }
            }
            if installed.is_empty() {
                return Err(format!(
                    "Не удалось найти зависимости в каталоге: {}. Установите их вручную на вкладке «Моды».",
                    failed.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            let extra = if failed.is_empty() {
                String::new()
            } else {
                format!("; не найдено: {}", failed.join(", "))
            };
            Ok(format!("Установлено: {}{extra}", installed.join(", ")))
        }
        "set_memory" => {
            let memory = params.get("memory").and_then(Value::as_str).unwrap_or_default().trim();
            let kib = super::config::parse_memory_kib(memory)?;
            let total = super::config::system_memory_kib();
            if kib < 512 * 1024 || (total > 0 && kib > total) {
                return Err("Недопустимый объём памяти".to_owned());
            }
            save(json!({ "memory": super::config::format_memory_kib(kib) }))?;
            Ok(format!("Память экземпляра: {}", human_memory(kib)))
        }
        "set_java" => {
            let java_path = params.get("java_path").and_then(Value::as_str).unwrap_or_default().trim();
            let java_path = if java_path.is_empty() {
                String::new()
            } else {
                super::runtime::validate_java_path(Path::new(java_path), true)?
                    .to_string_lossy()
                    .to_string()
            };
            save(json!({ "java_path": java_path }))?;
            Ok(if java_path.is_empty() {
                "Java выбирается автоматически".to_owned()
            } else {
                format!("Java экземпляра: {java_path}")
            })
        }
        "reset_jvm_arguments" => {
            save(json!({ "jvm_arguments": [], "jvm_preset": "none" }))?;
            Ok("Аргументы JVM и пресет сброшены".to_owned())
        }
        "backup_configs" => {
            let files = string_list(params.get("files"), MAX_FILES_PER_FIX)?;
            let root = info.path.join("config");
            let backup_root = info.path.join(BACKUP_DIRECTORY).join(super::backups::timestamp());
            let mut moved: Vec<String> = Vec::new();
            for name in files {
                let relative = name.replace('\\', "/");
                let relative = relative.strip_prefix("config/").unwrap_or(&relative).to_owned();
                let Some(source) = safe_child(&root, &relative) else { continue };
                if !source.is_file() {
                    continue;
                }
                let target = backup_root.join("config").join(&relative);
                if target.exists() {
                    continue;
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| format!("Ошибка файловой системы: {error}"))?;
                }
                fs::rename(&source, &target).map_err(|error| format!("Ошибка файловой системы: {error}"))?;
                moved.push(format!("config/{relative}"));
            }
            if moved.is_empty() {
                return Err("Конфиги не найдены — возможно, их уже пересоздали".to_owned());
            }
            Ok(format!(
                "Перенесено в {BACKUP_DIRECTORY}/: {}",
                moved.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
            ))
        }
        "repair_version" => {
            repair_instance_version(config, http, info)?;
            Ok("Версия и загрузчик восстановлены".to_owned())
        }
        "update_loader" => {
            let minimum = params.get("minimum").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
            if !minimum.is_empty() && !Regex::new(r"^[\w.+\-]{1,64}$").unwrap().is_match(&minimum) {
                return Err("Некорректная версия загрузчика".to_owned());
            }
            let installed = update_instance_loader(config, http, info, &minimum)?;
            Ok(if installed.is_empty() {
                "Загрузчик переустановлен".to_owned()
            } else {
                format!("Загрузчик установлен: {installed}")
            })
        }
        "set_account" => {
            let account_id = params.get("account_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
            if account_id.is_empty()
                || account_id.len() > 200
                || !Regex::new(r"^[\w.\-]+$").unwrap().is_match(&account_id)
            {
                return Err("Некорректный аккаунт".to_owned());
            }
            let response = super::accounts::set_default_account_in(
                &config.paths.config_home,
                &json!({ "account_id": account_id }),
            )?;
            let account = response.get("account").cloned().unwrap_or_else(|| json!({}));
            let alias = account.get("alias").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
            let username = account.get("username").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
            let label = if !alias.is_empty() {
                alias
            } else if !username.is_empty() {
                username
            } else {
                account_id.clone()
            };
            Ok(format!("Аккаунт по умолчанию: {label}"))
        }
        "login_microsoft" => {
            // вход по коду устройства запускается интерфейсом после применения починок (см; autoRepairDialog)
            Ok("Откроется вход Microsoft — подтвердите его в браузере".to_owned())
        }
        "set_security" => {
            let current = workspace.settings(&info.name)?;
            let profile = params
                .get("profile")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| current.get("security_profile").and_then(Value::as_str))
                .unwrap_or("strict")
                .trim()
                .to_lowercase();
            if !super::security::SECURITY_PROFILES.contains(&profile.as_str()) {
                return Err(format!("Неизвестный профиль безопасности: {profile}"));
            }
            let add_permissions: Vec<String> = match params.get("add_permissions").filter(|value| !value.is_null()) {
                Some(value) => string_list(Some(value), MAX_FILES_PER_FIX)?,
                None => Vec::new(),
            }
            .into_iter()
            .filter(|item| super::security::SECURITY_PERMISSIONS.contains(&item.as_str()))
            .collect();
            let remove_permissions: BTreeSet<String> = match params.get("remove_permissions").filter(|value| !value.is_null()) {
                Some(value) => string_list(Some(value), MAX_FILES_PER_FIX)?,
                None => Vec::new(),
            }
            .into_iter()
            .filter(|item| super::security::SECURITY_PERMISSIONS.contains(&item.as_str()))
            .collect();
            let mut permissions: Vec<String> = match current.get("security_permissions").filter(|value| value.is_array()) {
                Some(raw) => raw
                    .as_array()
                    .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default(),
                None => GAME_PERMISSIONS.iter().map(|item| item.to_string()).collect(),
            };
            permissions.retain(|item| !remove_permissions.contains(item));
            for item in &add_permissions {
                if !permissions.contains(item) {
                    permissions.push(item.clone());
                }
            }
            if SANDBOXED_PROFILES.contains(&profile.as_str()) {
                // песочница не может выдать эти разрешения, поэтому запуск снова не удался бы
                permissions.retain(|item| !super::security::UNENFORCED_CELL_PERMISSIONS.contains(&item.as_str()));
            }
            save(json!({ "security_profile": profile, "security_permissions": permissions }))?;
            let mut changes = vec![format!("профиль {profile}")];
            if !add_permissions.is_empty() {
                changes.push("выдано: ".to_owned() + &add_permissions.join(", "));
            }
            if !remove_permissions.is_empty() {
                changes.push("снято: ".to_owned() + &remove_permissions.iter().cloned().collect::<Vec<_>>().join(", "));
            }
            Ok("Безопасность экземпляра: ".to_owned() + &changes.join("; "))
        }
        other => Err(format!("Неизвестная починка: {other}")),
    }
}

/// сначала применяет остальные починки, затем восстанавливает версию и обновляет загрузчик
fn apply_fixes(
    config: &super::config::ConfigStore,
    http: &std::sync::Arc<super::net::HttpClient>,
    info: &super::instances::InstanceInfo,
    fixes: &[Value],
) -> Result<Vec<Value>, String> {
    if fixes.is_empty() {
        return Err("Не выбрано ни одной починки".to_owned());
    }
    if fixes.len() > 20 {
        return Err("Слишком много починок за раз".to_owned());
    }
    let mut ordered: Vec<&Value> = fixes.iter().collect();
    ordered.sort_by_key(|fix| {
        let action = fix.get("action").and_then(Value::as_str).unwrap_or_default();
        matches!(action, "repair_version" | "update_loader")
    });
    let mut results: Vec<Value> = Vec::new();
    for fix in ordered {
        let Some(object) = fix.as_object() else { continue };
        let action = object.get("action").and_then(Value::as_str).unwrap_or_default().to_owned();
        let params = object
            .get("params")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let fix_id: String = object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(&action)
            .chars()
            .take(120)
            .collect();
        if !FIX_ACTIONS.contains(&action.as_str()) {
            results.push(json!({"id": fix_id, "ok": false, "message": format!("Неизвестная починка: {action}")}));
            continue;
        }
        match apply_one(config, http, info, &action, &params) {
            Ok(message) => results.push(json!({"id": fix_id, "ok": true, "message": message})),
            Err(message) => results.push(json!({"id": fix_id, "ok": false, "message": message})),
        }
    }
    Ok(results)
}

/// применяет починки только к остановленному Java-клиенту
pub(super) fn auto_repair_apply(request: &Value) -> Result<Value, String> {
    let name = request.get("instance_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let config = super::config::ConfigStore::load()?;
    let store = super::instances::InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.kind.trim().to_lowercase() != "client" || info.game_edition.trim().to_lowercase() != "java" {
        return Err("Авто-починка доступна только для клиентских Java-экземпляров".to_owned());
    }
    if info.running {
        return Err("Остановите экземпляр перед починкой".to_owned());
    }
    let Some(fixes) = request.get("fixes").and_then(Value::as_array) else {
        return Err("Не выбрано ни одной починки".to_owned());
    };
    let http = super::net::client_from_settings(config.settings(), "")?;
    let _lock = super::server::InstanceLock::acquire(&config.paths.data_home, &info.name)?;
    let results = apply_fixes(&config, &http, &info, fixes)?;
    super::events::emit_status_event("workspace-changed", "Авто-починка применена.");
    let list: Vec<Value> = store.list().iter().map(|item| item.public()).collect();
    Ok(json!({
        "instance_name": info.name,
        "results": results,
        "instances": list,
    }))
}
