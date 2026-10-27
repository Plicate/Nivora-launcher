use super::*;

pub(super) const BOL_CONTENT_ROOT_PARTS: &[&str] = &[
    "compatdata", "pfx", "drive_c", "users", "steamuser", "AppData", "Roaming",
    "Minecraft Bedrock", "Users",
];

pub fn content_list(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?;
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_content_list(name),
        BedrockPlatform::WindowsOfficial => windows_content_list(name),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

pub(super) fn scope_tail(scope: &str) -> String {
    let chars: Vec<char> = scope.chars().collect();
    if chars.len() > 4 {
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("…{tail}")
    } else {
        scope.to_owned()
    }
}

/// собирает сведения о файлах в каталогах данных Bedrock
fn scan_content(roots: &[(String, PathBuf)]) -> Result<Vec<Value>, String> {
    let mut values: Vec<Value> = Vec::new();
    for (scope, content_root) in roots {
        if is_reparse_point(content_root) || !content_root.is_dir() {
            continue;
        }
        for (category, directories) in CONTENT_DIRECTORIES {
            for directory_name in *directories {
                let directory = content_root.join(directory_name);
                if is_reparse_point(&directory) || !directory.is_dir() {
                    continue;
                }
                let mut entries: Vec<_> = match fs::read_dir(&directory) {
                    Ok(iter) => iter.filter_map(|entry| entry.ok()).collect(),
                    Err(_) => continue,
                };
                entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
                for entry in entries {
                    let path = entry.path();
                    if is_reparse_point(&path) {
                        continue;
                    }
                    let Ok(meta) = entry.metadata() else { continue };
                    let is_dir = meta.is_dir();
                    values.push(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "scope": scope,
                        "scope_label": if scope == "LocalState" {
                            "Данные игры".to_owned()
                        } else {
                            format!("Профиль аккаунта {}", scope_tail(scope))
                        },
                        "category": category,
                        "directory": directory_name,
                        "item_kind": if is_dir { "directory" } else { "file" },
                        "size": if is_dir { 0 } else { meta.len() },
                        "modified_at": meta
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|value| value.as_secs_f64())
                            .unwrap_or(0.0),
                    }));
                    if values.len() >= 5000 {
                        return Ok(values);
                    }
                }
            }
        }
    }
    Ok(values)
}

fn linux_content_list(name: &str) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut roots: Vec<(String, PathBuf)> = Vec::new();
    let users_root = safe_profile_path(&profile, BOL_CONTENT_ROOT_PARTS)?;
    if users_root.is_dir() {
        if let Ok(entries) = fs::read_dir(&users_root) {
            let mut scopes: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| !is_reparse_point(path) && path.is_dir())
                .collect();
            scopes.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
            for scope in scopes {
                let scope_name =
                    scope.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
                if !scope_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    continue;
                }
                let mut parts = BOL_CONTENT_ROOT_PARTS.to_vec();
                parts.extend_from_slice(&[scope_name.as_str(), "games", "com.mojang"]);
                let content_root = safe_profile_path(&profile, &parts)?;
                roots.push((scope_name, content_root));
            }
        }
    }
    Ok(json!({ "items": scan_content(&roots)? }))
}

fn windows_content_list(name: &str) -> Result<Value, String> {
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let (edition, _) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release" } else { edition.as_str() };
    let content_root = win_content_root(edition)?;
    Ok(json!({ "items": scan_content(&[("LocalState".to_owned(), content_root)])? }))
}

pub fn screenshots_list(request: &Value) -> Result<Value, String> {
    const MAX_SCREENSHOTS: usize = 500;
    let name = request_field(request, "name")?.to_owned();
    let mut items: Vec<Value> = Vec::new();

    let scan = |label: String, content_root: &Path, read_prefix: &[String], items: &mut Vec<Value>| -> Result<(), String> {
        let directory = content_root.join("minecraftScreenshots");
        if !directory.is_dir() || is_reparse_point(&directory) {
            return Ok(());
        }
        let entries = fs::read_dir(&directory).map_err(|_| "Не удалось прочитать каталог скриншотов Bedrock".to_owned())?;
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if reserved_screenshot_name(&file_name) || is_reparse_point(&entry.path()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else { continue };
            if !metadata.is_file() {
                continue;
            }
            let lower = file_name.to_lowercase();
            if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
                continue;
            }
            let mut parts = read_prefix.to_vec();
            parts.push("minecraftScreenshots".to_owned());
            parts.push(file_name.clone());
            items.push(json!({
                "name": file_name,
                "read_path": parts.join("/"),
                "scope_label": label,
                "size": metadata.len() as i64,
                "modified_at": metadata.modified().ok().and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok()).map(|value| value.as_secs() as i64).unwrap_or(0),
            }));
            if items.len() >= MAX_SCREENSHOTS {
                break;
            }
        }
        Ok(())
    };

    if cfg!(windows) {
        let store = instance_store()?;
        let info = bedrock_instance(&store, &name)?;
        let (edition, _) = windows_selected(&info.path);
        let edition = if edition.is_empty() { "release" } else { edition.as_str() };
        let content_root = win_content_root(edition)?;
        scan("Скриншоты игры".to_owned(), &content_root, &[], &mut items)?;
    } else {
        let ctx = context()?;
        let store = instance_store()?;
        let info = bedrock_instance(&store, &name)?;
        let profile = linux_profile_path(&ctx, &info)?;
        let users_root = safe_profile_path(&profile, BOL_CONTENT_ROOT_PARTS)?;
        if users_root.is_dir() {
            if let Ok(entries) = fs::read_dir(&users_root) {
                let mut scopes: Vec<PathBuf> = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| !is_reparse_point(path) && path.is_dir())
                    .collect();
                scopes.sort_by_key(|path| path.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default());
                for scope in scopes {
                    let scope_name = scope.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
                    if !scope_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                        continue;
                    }
                    let mut parts = BOL_CONTENT_ROOT_PARTS.to_vec();
                    parts.push(scope_name.as_str());
                    let mut prefix: Vec<String> = parts.iter().map(|value| value.to_string()).collect();
                    prefix.extend_from_slice(&["games".to_owned(), "com.mojang".to_owned()]);
                    let mut content_root = profile.to_path_buf();
                    for part in &prefix {
                        content_root = content_root.join(part);
                    }
                    scan(format!("Профиль аккаунта {}", scope_tail(&scope_name)), &content_root, &prefix, &mut items)?;
                    if items.len() >= MAX_SCREENSHOTS {
                        break;
                    }
                }
            }
        }
    }
    items.sort_by(|left, right| right["modified_at"].as_i64().cmp(&left["modified_at"].as_i64()));
    Ok(json!({ "items": items }))
}

/// служебные и скрытые имена в каталоге скриншотов не показываем
fn reserved_screenshot_name(name: &str) -> bool {
    name.starts_with('.') || name == "Thumbs.db" || name.is_empty()
}

fn safe_profile_path(profile: &Path, parts: &[&str]) -> Result<PathBuf, String> {
    if is_reparse_point(profile) {
        return Err("Путь контента Bedrock содержит символьную ссылку".to_owned());
    }
    let root = profile.canonicalize().unwrap_or_else(|_| profile.to_path_buf());
    let mut path = profile.to_path_buf();
    for part in parts {
        if part.is_empty() || *part == "." || *part == ".." || part.contains('/') || part.contains('\\') || part.contains('\0') {
            return Err("Недопустимый путь контента Bedrock".to_owned());
        }
        path = path.join(part);
        if is_reparse_point(&path) {
            return Err("Путь контента Bedrock содержит символьную ссылку".to_owned());
        }
        if let Ok(resolved) = path.canonicalize() {
            if !resolved.starts_with(&root) {
                return Err("Путь контента Bedrock выходит за пределы профиля".to_owned());
            }
        }
    }
    Ok(path)
}

pub fn content_import(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let file_name = request_field(request, "file_name")?.to_owned();
    let data = request_field(request, "data").unwrap_or_default();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| "Файл Bedrock не является корректными base64-данными".to_owned())?;
    if decoded.is_empty() || decoded.len() > MAX_CONTENT_UPLOAD_BYTES {
        return Err("Файл Bedrock пуст или превышает лимит 128 MiB".to_owned());
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_content_import(&name, &file_name, decoded),
        BedrockPlatform::WindowsOfficial => windows_content_import(&name, &file_name, decoded),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

pub(super) fn import_extension(file_name: &str) -> Result<String, String> {
    let suffix = Path::new(file_name)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if IMPORT_EXTENSIONS.contains(&suffix.as_str()) {
        Ok(suffix)
    } else {
        Err("Bedrock поддерживает .mcpack, .mcaddon, .mcworld, .mctemplate и .mcskin".to_owned())
    }
}

fn linux_content_import(name: &str, file_name: &str, content: Vec<u8>) -> Result<Value, String> {
    let suffix = import_extension(file_name)?;
    validate_content_archive(&content)?;
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    bol_require_runtime(&profile)?;
    let temp_root = ctx.cache_home.join("bedrock-imports");
    if is_reparse_point(&temp_root) {
        return Err("Временный каталог Bedrock является символьной ссылкой".to_owned());
    }
    fs::create_dir_all(&temp_root).map_err(|_| "Временный каталог Bedrock недоступен".to_owned())?;
    if is_symlink_or_missing_dir(&temp_root) {
        return Err("Временный каталог Bedrock недоступен".to_owned());
    }
    let staged = temp_root.join(format!(".nivora-bedrock-{}{suffix}", random_hex(6)));
    {
        let mut file = super::platform::create_private(&staged)
            .map_err(|_| "Не удалось подготовить файл импорта".to_owned())?;
        file.write_all(&content)
            .map_err(|_| "Не удалось подготовить файл импорта".to_owned())?;
    }
    let result = bol_run(
        &["import".to_owned(), staged.to_string_lossy().into_owned()],
        &profile,
        "import",
        None,
        None,
        Duration::from_secs(600),
        500,
        false,
        None,
        None,
        None,
    );
    let _ = fs::remove_file(&staged);
    let (code, logs) = result.map_err(|error| format!("Не удалось импортировать контент Bedrock: {error}"))?;
    if code != 0 {
        let detail = logs.last().cloned().unwrap_or_else(|| format!("код {code}"));
        return Err(format!("Не удалось импортировать контент Bedrock: {detail}"));
    }
    Ok(json!({
        "items": linux_content_list(name)?.get("items").cloned().unwrap_or_default(),
        "logs": logs,
    }))
}

pub(super) fn classify_archive(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Result<Vec<(String, String)>, String> {
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| archive.by_index(index).ok().map(|entry| entry.name().map(|v| v.replace('\\', "/")).unwrap_or_default()))
        .collect();
    let has_manifest = |prefix: &str| {
        names.iter().any(|name| {
            name.as_str() == format!("{prefix}manifest.json") || name.as_str() == format!("{prefix}/manifest.json")
        })
    };
    let has_world_files = |prefix: &str| {
        names.iter().any(|name| {
            name.as_str() == format!("{prefix}levelname.txt")
                || name.as_str() == format!("{prefix}level.dat")
                || name.as_str() == format!("{prefix}/levelname.txt")
                || name.as_str() == format!("{prefix}/level.dat")
        })
    };
    if has_manifest("") || has_world_files("") {
        let mut roots = Vec::new();
        classify_single_root(archive, "", &mut roots)?;
        return Ok(roots);
    }
    // .mcaddon: несколько паков в подкаталогах
    let mut directories: Vec<String> = Vec::new();
    for name in &names {
        if let Some(index) = name.find('/') {
            let directory = name[..index].to_owned();
            if !directory.is_empty() && !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    directories.sort();
    let mut roots: Vec<(String, String)> = Vec::new();
    let mut found_any = false;
    for directory in &directories {
        if has_manifest(directory) || has_world_files(directory) {
            found_any = true;
            classify_single_root(archive, directory, &mut roots)?;
        }
    }
    if !found_any {
        return Err("В архиве не найден manifest.json ни данные мира (levelname.txt/level.dat)".to_owned());
    }
    Ok(roots)
}

fn classify_single_root(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    prefix: &str,
    roots: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let prefix_slash = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
    if let Some(manifest) = read_manifest_value(archive, &prefix_slash) {
        let mut target = "resource_packs";
        if let Some(modules) = manifest.get("modules").and_then(Value::as_array) {
            for module in modules {
                let kind = module.get("type").and_then(Value::as_str).unwrap_or("");
                if kind == "data" {
                    target = "behavior_packs";
                    break;
                }
                if kind == "skin_pack" {
                    target = "skin_packs";
                    break;
                }
                if kind == "resources" {
                    target = "resource_packs";
                }
            }
        }
        roots.push((target.to_owned(), prefix.to_owned()));
        return Ok(());
    }
    // мир без манифеста: levelname.txt / level.dat
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| archive.by_index(index).ok().map(|entry| entry.name().map(|v| v.replace('\\', "/")).unwrap_or_default()))
        .collect();
    if names
        .iter()
        .any(|name| name.as_str() == format!("{prefix_slash}levelname.txt") || name.as_str() == format!("{prefix_slash}level.dat"))
    {
        roots.push(("minecraftWorlds".to_owned(), prefix.to_owned()));
        return Ok(());
    }
    Err("В архиве не найден manifest.json ни данные мира".to_owned())
}

fn read_manifest_value(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, prefix_slash: &str) -> Option<Value> {
    let candidates = [
        format!("{prefix_slash}manifest.json"),
        format!("{}.manifest.json", prefix_slash.trim_end_matches('/')),
    ];
    for candidate in candidates {
        if let Ok(mut entry) = archive.by_name(&candidate) {
            let mut text = String::new();
            if entry.read_to_string(&mut text).is_ok() {
                if let Ok(value) = serde_json::from_str(&text) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn windows_content_import(name: &str, file_name: &str, content: Vec<u8>) -> Result<Value, String> {
    let suffix = import_extension(file_name)?;
    let mut archive = validate_content_archive(&content)?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let (edition, _) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release" } else { edition.as_str() };
    let content_root = win_content_root(edition)?;
    // сначала проверяем расширение, затем при необходимости читаем манифест
    let targets: Vec<(String, String)> = match suffix.as_str() {
        ".mcworld" | ".mctemplate" => vec![("minecraftWorlds".to_owned(), String::new())],
        ".mcskin" => vec![("skin_packs".to_owned(), String::new())],
        _ => classify_archive(&mut archive)?,
    };
    let pack_name = Path::new(file_name)
        .file_stem()
        .map(|value| sanitize_content_name(&value.to_string_lossy()))
        .unwrap_or_else(|| format!("pack-{}", random_hex(3)));
    let mut imported: Vec<String> = Vec::new();
    for (target_directory, archive_prefix) in &targets {
        let destination = content_root.join(target_directory).join(&pack_name);
        if is_reparse_point(&destination) {
            continue;
        }
        if destination.is_dir() {
            fs::remove_dir_all(&destination).map_err(|error| format!("Не удалось заменить контент: {error}"))?;
        } else if destination.is_file() {
            let _ = fs::remove_file(&destination);
        }
        fs::create_dir_all(&destination).map_err(|error| format!("Не удалось создать каталог контента: {error}"))?;
        super::platform::restrict_dir(&destination);
        let prefix_slash = if archive_prefix.is_empty() { String::new() } else { format!("{archive_prefix}/") };
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| format!("Не удалось прочитать архив: {error}"))?;
            if entry.is_dir() {
                continue;
            }
            let entry_name = entry.name().map(|v| v.replace('\\', "/")).unwrap_or_default();
            let Some(relative) = entry_name.strip_prefix(&prefix_slash) else { continue };
            if relative.is_empty() || relative.split('/').any(|part| part == ".." || part.is_empty()) {
                continue;
            }
            let target_path = destination.join(relative);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог контента: {error}"))?;
            }
            if is_reparse_point(&target_path) {
                continue;
            }
            let mut file = super::platform::create_private(&target_path)
                .map_err(|error| format!("Не удалось записать контент: {error}"))?;
            std::io::copy(&mut entry, &mut file).map_err(|error| format!("Не удалось записать контент: {error}"))?;
        }
        imported.push(format!("{target_directory}/{pack_name}"));
    }
    Ok(json!({
        "items": windows_content_list(name)?.get("items").cloned().unwrap_or_default(),
        "logs": [format!("Импортировано в официальную версию: {}", imported.join(", "))],
    }))
}

fn sanitize_content_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '_' || c == '-' || c == '.' { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let result: String = trimmed.chars().take(96).collect();
    if result.is_empty() {
        format!("pack-{}", random_hex(3))
    } else {
        result
    }
}

pub fn content_remove(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let scope = request_field(request, "scope").unwrap_or_default().to_owned();
    let directory_name = request_field(request, "directory")?.to_owned();
    let item_name = request_field(request, "item_name")?.to_owned();
    let known: Vec<&str> = CONTENT_DIRECTORIES.iter().flat_map(|(_, dirs)| dirs.iter().copied()).collect();
    if !known.contains(&directory_name.as_str()) {
        return Err("Неизвестный каталог Bedrock".to_owned());
    }
    for value in [&item_name, &scope] {
        let invalid = value.is_empty()
            || *value == "."
            || *value == ".."
            || value.contains('/')
            || value.contains('\\')
            || value.contains('\0')
            || value.chars().any(|c| (c as u32) < 32)
            || value.len() > 255;
        if invalid {
            return Err("Недопустимое имя ресурса Bedrock".to_owned());
        }
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_content_remove(&name, &scope, &directory_name, &item_name),
        BedrockPlatform::WindowsOfficial => windows_content_remove(&name, &directory_name, &item_name),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn linux_content_remove(name: &str, scope: &str, directory_name: &str, item_name: &str) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut parts = BOL_CONTENT_ROOT_PARTS.to_vec();
    parts.extend_from_slice(&[scope, "games", "com.mojang", directory_name, item_name]);
    let target = safe_profile_path(&profile, &parts)?;
    if is_reparse_point(&target) || !target.exists() {
        return Err("Ресурс Bedrock не найден".to_owned());
    }
    if target.is_dir() {
        fs::remove_dir_all(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    } else if target.is_file() {
        fs::remove_file(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    } else {
        return Err("Можно удалить только файл или папку Bedrock".to_owned());
    }
    Ok(json!({ "items": linux_content_list(name)?.get("items").cloned().unwrap_or_default() }))
}

fn windows_content_remove(name: &str, directory_name: &str, item_name: &str) -> Result<Value, String> {
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let (edition, _) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release" } else { edition.as_str() };
    let content_root = win_content_root(edition)?;
    let target = content_root.join(directory_name).join(item_name);
    if is_reparse_point(&target) || !target.exists() {
        return Err("Ресурс Bedrock не найден".to_owned());
    }
    if target.is_dir() {
        fs::remove_dir_all(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    } else if target.is_file() {
        fs::remove_file(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    }
    Ok(json!({ "items": windows_content_list(name)?.get("items").cloned().unwrap_or_default() }))
}
