// экспорт экземпляра в формат Modrinth (.mrpack). Моды с Modrinth-метаданными
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::instances::InstanceInfo;
use super::mods::{random_hex, read_metadata};
use super::net::HttpClient;

const MRPACK_FORMAT_VERSION: i64 = 1;
const MAX_EXPORT_FILES: usize = 20_000;
const MAX_EXPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const OVERRIDE_ROOTS: &[&str] = &[
    "config", "defaultconfigs", "shaderpacks", "resourcepacks", "scripts", "kubejs", "journeymap",
];
const OVERRIDE_FILES: &[&str] = &["options.txt", "servers.dat", "servers.dat_old"];

fn loader_from_version(version_id: &str, loader: &str) -> String {
    if loader != "vanilla" {
        return loader.to_owned();
    }
    let lowered = version_id.to_lowercase();
    for name in ["neoforge", "forge", "quilt", "fabric"] {
        if lowered.contains(name) {
            return name.to_owned();
        }
    }
    "vanilla".to_owned()
}

fn loader_version(version_id: &str, minecraft_version: &str) -> String {    let mut value = version_id.to_owned();
    if !minecraft_version.is_empty() {
        let suffix = format!("-{minecraft_version}").to_lowercase();
        if let Some(position) = value.len().checked_sub(suffix.len()) {
            if value.is_char_boundary(position) && value[position..].to_lowercase() == suffix {
                value.truncate(position);
            }
        }
    }
    for pattern in [
        r"(?i)fabric-loader-([0-9][\w.+-]*)",
        r"(?i)quilt-loader-([0-9][\w.+-]*)",
        r"(?i)forge-([0-9][\w.+-]*)",
        r"(?i)neoforge-?([0-9][\w.+-]*)",
    ] {
        let Ok(regex) = regex::Regex::new(pattern) else { continue };
        if let Some(captures) = regex.captures(&value) {
            return captures.get(1).map(|found| found.as_str().to_owned()).unwrap_or_default();
        }
    }
    String::new()
}

struct VersionDownload {
    url: String,
    sha1: String,
    size: i64,
    primary_file_name: String,
}

fn version_download(http: &HttpClient, base_url: &str, version_id: &str) -> Option<VersionDownload> {
    let value = http.get_json(&format!("{}/version/{version_id}", base_url.trim_end_matches('/')), None).ok()?;
    let files = value.get("files").and_then(Value::as_array)?;
    let primary = files
        .iter()
        .find(|item| item.is_object() && item.get("primary").map(is_truthy).unwrap_or(false))
        .or_else(|| files.iter().find(|item| item.is_object()))?;
    let url = primary.get("url").and_then(Value::as_str).unwrap_or("").to_owned();
    let hashes = primary.get("hashes").filter(|value| value.is_object());
    let sha1 = hashes
        .and_then(|value| value.get("sha1"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if !url.starts_with("https://") || sha1.is_empty() {
        return None;
    }
    Some(VersionDownload {
        url,
        sha1,
        size: primary
            .get("size")
            .and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|text| text.trim().parse::<i64>().ok()))
            })
            .unwrap_or(0),
        primary_file_name: primary
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

/// локальная метка времени вида 2026.10.13-1442 — как time.strftime.
fn local_timestamp() -> String {
    #[cfg(unix)]
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "1970.01.01-0000".to_owned();
        }
        format!(
            "{:04}.{:02}.{:02}-{:02}{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        )
    }
    #[cfg(not(unix))]
    {
        // localtime_r нет — считаем UTC вручную (дни → гражданская дата).
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or(0);
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        format!(
            "{:04}.{:02}.{:02}-{:02}{:02}",
            year,
            month,
            day,
            rest / 3600,
            (rest % 3600) / 60
        )
    }
}

#[cfg(not(unix))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn collect_override_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if is_symlink(&path) {
            continue;
        }
        if path.is_dir() {
            collect_override_files(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

pub(crate) fn export_instance_as_mrpack(
    http: &Arc<HttpClient>,
    instance: &InstanceInfo,
    destination_dir: &Path,
    display_name: &str,
    version_label: &str,
) -> Result<Value, String> {
    if instance.game_edition != "java" {
        return Err("Экспорт модпака поддерживается только для Java-экземпляров".to_owned());
    }
    let loader = loader_from_version(&instance.version_id, &instance.loader);
    if loader == "vanilla" {
        return Err("Ванильный экземпляр нечего экспортировать как модпак — установите загрузчик или моды".to_owned());
    }
    let minecraft_version = instance.minecraft_version.clone();
    if minecraft_version.is_empty() {
        return Err("Не удалось определить версию Minecraft экземпляра".to_owned());
    }

    let instance_root = &instance.path;
    let mods_dir = instance_root.join("mods");
    let mut indexed: Vec<Value> = Vec::new();
    let mut override_files: Vec<PathBuf> = Vec::new();
    if mods_dir.is_dir() && !is_symlink(&mods_dir) {
        let mut entries: Vec<PathBuf> = fs::read_dir(&mods_dir)
            .map(|entries| entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
        for path in entries {
            let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            if !path.is_file() || name.ends_with(".disabled") || name.starts_with('.') {
                continue;
            }
            if !(name.ends_with(".jar") || name.ends_with(".zip")) {
                continue;
            }
            let metadata = read_metadata(&path.with_file_name(format!(".{name}.trial-project.json")));
            let catalog_url = metadata
                .get("catalog_url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("https://api.modrinth.com")
                .to_owned();
            let version_id = metadata.get("version_id").and_then(Value::as_str).unwrap_or("").to_owned();
            let resolved = if version_id.is_empty() {
                None
            } else {
                version_download(http, &catalog_url, &version_id)
            };
            if resolved.as_ref().map(|item| item.primary_file_name == name).unwrap_or(false) {
                let resolved = resolved.unwrap();
                indexed.push(json!({
                    "path": format!("mods/{name}"),
                    "hashes": { "sha1": resolved.sha1 },
                    "env": { "client": "required", "server": "unsupported" },
                    "downloads": [resolved.url],
                    "file_size": resolved.size,
                }));
            } else {
                override_files.push(path);
            }
        }
    }

    for root_name in OVERRIDE_ROOTS {
        let root = instance_root.join(root_name);
        if root.is_dir() && !is_symlink(&root) {
            collect_override_files(&root, &mut override_files);
        }
    }
    for file_name in OVERRIDE_FILES {
        let path = instance_root.join(file_name);
        if path.is_file() && !is_symlink(&path) {
            override_files.push(path);
        }
    }

    if indexed.is_empty() && override_files.is_empty() {
        return Err("В экземпляре нет модов и настроек для экспорта".to_owned());
    }

    let mut dependencies = json!({ "minecraft": minecraft_version });
    let resolved_loader_version = loader_version(&instance.version_id, &minecraft_version);
    let loader_key = match loader.as_str() {
        "fabric" => Some("fabric-loader"),
        "quilt" => Some("quilt-loader"),
        "forge" => Some("forge"),
        "neoforge" => Some("neoforge"),
        _ => None,
    };
    if let Some(loader_key) = loader_key.filter(|_| !resolved_loader_version.is_empty()) {
        dependencies[loader_key] = json!(resolved_loader_version);
    }

    // как в Python: выбор по истинности (пустая строка — ложь), без trim.
    let name = if !display_name.is_empty() {
        display_name.to_owned()
    } else if !instance.display_name.is_empty() {
        instance.display_name.clone()
    } else {
        instance.name.clone()
    };
    let version_id = if version_label.is_empty() {
        local_timestamp()
    } else {
        version_label.to_owned()
    };
    let index = json!({
        "formatVersion": MRPACK_FORMAT_VERSION,
        "game": "minecraft",
        "versionId": version_id,
        "name": name,
        "summary": instance.description.chars().take(256).collect::<String>(),
        "files": indexed,
        "dependencies": dependencies,
    });

    create_dir_all_700(destination_dir)?;
    let archive_name = archive_slug(&name, &instance.name);
    let safe_name = format!("{archive_name}-{version_id}.mrpack");
    let destination = destination_dir.join(safe_name);
    let mut total: u64 = 0;
    let mut files_count: usize = 1; // modrinth.index.json
    let temporary = destination_dir.join(format!(".export-{random}.part", random = random_hex(8)));
    let result = write_mrpack(&temporary, &index, &override_files, instance_root, &mut total, &mut files_count);
    if let Err(message) = result {
        let _ = fs::remove_file(&temporary);
        return Err(message);
    }
    if fs::rename(&temporary, &destination).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("Не удалось записать файл экспорта".to_owned());
    }
    let size = fs::metadata(&destination).map(|meta| meta.len()).unwrap_or(0);
    Ok(json!({
        "path": destination.to_string_lossy(),
        "file_name": destination.file_name().map(|value| value.to_string_lossy()).unwrap_or_default(),
        "size_bytes": size,
        "indexed_files": indexed.len(),
        "override_files": override_files.len(),
    }))
}

fn archive_slug(name: &str, fallback: &str) -> String {
    let regex = regex::Regex::new(r"[^A-Za-z0-9._-]+").unwrap();
    let slug: String = regex.replace_all(name, "-").to_string();
    let slug = slug.trim_matches(['.', '-']).to_owned();
    let slug: String = slug.chars().take(64).collect();
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}

fn create_dir_all_700(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("Не удалось создать каталог {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn write_mrpack(
    temporary: &Path,
    index: &Value,
    override_files: &[PathBuf],
    instance_root: &Path,
    total: &mut u64,
    files_count: &mut usize,
) -> Result<(), String> {
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(temporary)
        .map_err(|error| format!("Не удалось записать файл экспорта: {error}"))?;
    let options = zip::write::SimpleFileOptions::default();
    let mut archive = zip::ZipWriter::new(file);
    let index_text = serde_json::to_string_pretty(index).map_err(|error| error.to_string())?;
    archive
        .start_file("modrinth.index.json", options)
        .map_err(|error| format!("Не удалось записать файл экспорта: {error}"))?;
    archive
        .write_all(index_text.as_bytes())
        .map_err(|error| format!("Не удалось записать файл экспорта: {error}"))?;
    for path in override_files {
        if *files_count >= MAX_EXPORT_FILES || *total > MAX_EXPORT_BYTES {
            return Err("Экземпляр слишком велик для экспорта (лимит 4 ГиБ)".to_owned());
        }
        let size = fs::metadata(path).map_err(|error| error.to_string())?.len();
        *total += size;
        *files_count += 1;
        let relative = path.strip_prefix(instance_root).map_err(|_| "Небезопасный путь файла экспорта".to_owned())?;
        let archive_path = relative.to_string_lossy().replace('\\', "/");
        archive
            .start_file(format!("overrides/{archive_path}"), options)
            .map_err(|error| format!("Не удалось записать файл экспорта: {error}"))?;
        let mut input = File::open(path).map_err(|error| error.to_string())?;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            archive.write_all(&buffer[..count]).map_err(|error| format!("Не удалось записать файл экспорта: {error}"))?;
        }
    }
    archive.finish().map_err(|error| format!("Не удалось записать файл экспорта: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_names_follow_version_hints() {
        assert_eq!(loader_from_version("1.20.4", "fabric"), "fabric");
        assert_eq!(loader_from_version("1.20.4-neoforge-47", "vanilla"), "neoforge");
        assert_eq!(loader_from_version("forge-47.1.3-1.20.4", "vanilla"), "forge");
        assert_eq!(loader_from_version("plain-1.20.4", "vanilla"), "vanilla");
    }

    #[test]
    fn loader_versions_strip_game_suffix_first() {
        assert_eq!(
            loader_version("fabric-loader-0.92.2-1.20.4", "1.20.4"),
            "0.92.2"
        );
        // без версии игры паттерн жадный и забирает всё до конца, как в Python.
        assert_eq!(loader_version("neoforge-47.1.3-1.20.4", ""), "47.1.3-1.20.4");
        assert_eq!(loader_version("vanilla", ""), "");
    }

    #[test]
    fn archive_slug_is_safe_and_bounded() {
        // кириллица не попадает в [A-Za-z0-9._-] — всё схлопывается в fallback.
        assert_eq!(archive_slug("Мой крутой пак!", "fallback"), "fallback");
        assert_eq!(archive_slug("Cool Pack v2", "fallback"), "Cool-Pack-v2");
        assert_eq!(archive_slug("...---", "fallback"), "fallback");
        assert_eq!(archive_slug(&"x".repeat(100), "fallback").chars().count(), 64);
    }

    #[test]
    fn timestamp_shape_matches_strftime() {
        let value = local_timestamp();
        // %Y.%m.%d-%H%M — три точки-части и одна «HHMM» после дефиса.
        let parts: Vec<&str> = value.split(['.', '-']).collect();
        assert_eq!(parts.len(), 4, "формат {value}");
        let year: i64 = parts[0].parse().unwrap();
        assert!(year >= 2024 && year <= 2100);
        let month: u32 = parts[1].parse().unwrap();
        assert!((1..=12).contains(&month));
        let (day, minute) = (parts[2].parse::<u32>().unwrap(), parts[3].parse::<u32>().unwrap());
        assert!((1..=31).contains(&day));
        assert!(minute < 2400, "HHMM вне диапазона: {minute}");
    }
}
