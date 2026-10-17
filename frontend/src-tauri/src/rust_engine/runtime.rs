// Выбор Java для запуска: настроенный бинар, системная java или runtime
// Mojang из piston-meta; проверка прав/владения executable и безопасное
// разворачивание runtime-манифеста в runtimes_dir.
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::config::ConfigStore;
use super::platform_support::canonical_path;
use super::constants::JAVA_RUNTIME_MANIFEST_URL;
use super::net::{atomic_json_write, HttpClient};

pub(crate) struct JavaRuntimeManager {
    runtimes_dir: PathBuf,
    download_cache: PathBuf,
    configured_java: String,
    http: Arc<HttpClient>,
}

impl JavaRuntimeManager {
    pub(crate) fn new(config: &ConfigStore, http: Option<Arc<HttpClient>>) -> Result<Self, String> {
        let http = match http {
            Some(http) => http,
            // Умолчания HttpClient из Python-воркера: 30 секунд, 3 попытки.
            None => Arc::new(HttpClient::new(30, 3, "", None)?),
        };
        Ok(Self {
            runtimes_dir: config.paths.runtimes_dir.clone(),
            download_cache: config.paths.download_cache.clone(),
            configured_java: config.str_setting("java_path"),
            http,
        })
    }

    pub(crate) fn java_for(&self, version_json: &Value, java_path: Option<&str>) -> Result<PathBuf, String> {
        let required_major = version_json
            .get("javaVersion")
            .and_then(|java| java.get("majorVersion"))
            .and_then(Value::as_i64)
            .filter(|major| *major != 0);
        let configured = match java_path {
            Some(value) => value.trim().to_owned(),
            None => self.configured_java.trim().to_owned(),
        };
        if !configured.is_empty() {
            let path = validate_java_path(Path::new(&configured), true)?;
            let actual = java_version(&path);
            if let (Some(required), Some(actual)) = (required_major, actual) {
                if actual > 0 && actual < required {
                    return Err(format!(
                        "Настроенная Java {actual} слишком старая для этой версии; требуется Java {required}"
                    ));
                }
            }
            return Ok(path);
        }

        let mut system_path: Option<PathBuf> = None;
        let mut system_major: Option<i64> = None;
        if let Some(system) = which(java_binary_name()) {
            let path = validate_java_path(&system, false)?;
            let major = java_version(&path);
            let matches_requirement = required_major.map(|required| major == Some(required)).unwrap_or(true);
            if matches_requirement {
                return Ok(path);
            }
            system_path = Some(path);
            system_major = major;
        }

        let component = version_json
            .get("javaVersion")
            .and_then(|java| java.get("component"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let mut runtime_error: Option<String> = None;
        if let Some(component) = component {
            match self.ensure_mojang_runtime(component) {
                Ok(Some(bundled)) => return Ok(bundled),
                Ok(None) => {}
                Err(error) => runtime_error = Some(error),
            }
        }
        if let Some(path) = system_path {
            if let (Some(required), Some(actual)) = (required_major, system_major) {
                if actual > 0 && actual < required {
                    return Err(format!("Java {actual} слишком старая; требуется Java {required}"));
                }
            }
            return Ok(path);
        }
        if let Some(error) = runtime_error {
            return Err(error);
        }
        Err("Java не найден. Установите jre-openjdk или разрешите лаунчеру скачать runtime Mojang.".to_owned())
    }

    /// None — компонент недоступен для нашей платформы (или без манифеста);
    /// ошибки манифеста/сети — Err.
    fn ensure_mojang_runtime(&self, component: &str) -> Result<Option<PathBuf>, String> {
        safe_runtime_component(component, "runtime component")?;
        let platform_key = runtime_platform()?;
        let all_manifest = self.runtime_manifest()?;
        let candidates = all_manifest
            .get(platform_key.as_str())
            .filter(|value| value.is_object())
            .and_then(|data| data.get(component))
            .and_then(Value::as_array)
            .filter(|list| !list.is_empty());
        let Some(candidates) = candidates else {
            return Ok(None);
        };
        let entry = candidates.first().cloned().unwrap_or(Value::Null);
        if !entry.is_object() {
            return Err(format!("Java runtime manifest имеет некорректный entry для {component}"));
        }
        let runtime_version = match entry.get("version").filter(|value| value.is_object()) {
            Some(version) => version.get("name").and_then(Value::as_str).unwrap_or("unknown"),
            None => "unknown",
        }
        .to_owned();
        safe_runtime_component(&runtime_version, "runtime version")?;
        let destination = self.runtimes_dir.join(component).join(&runtime_version);
        ensure_runtime_path(&destination, &self.runtimes_dir)?;
        fs::create_dir_all(&destination).map_err(|error| format!("Не удалось создать каталог Java runtime: {error}"))?;
        let destination_resolved = resolve_path(&destination);
        let java_path = destination.join("bin").join(java_binary_name());
        let marker = destination.join(".trial-runtime-complete");
        if !is_reparse_point(&marker)
            && marker.exists()
            && java_path.exists()
            && within(&resolve_path(&java_path), &destination_resolved)
            && is_executable(&java_path)
        {
            return validate_java_path(&java_path, false).map(Some);
        }

        let Some(manifest_url) = entry
            .get("manifest")
            .filter(|value| value.is_object())
            .and_then(|manifest| manifest.get("url"))
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        else {
            return Ok(None);
        };
        let manifest = self.http.get_json(manifest_url, None)?;
        let files = manifest.get("files").cloned().unwrap_or_else(|| manifest.clone());
        let Some(files_map) = files.as_object() else {
            return Err(format!("Неожиданный Java runtime manifest для {component}"));
        };
        let total_entries = files_map.len();
        let mut processed_files: usize = 0;
        #[cfg(windows)]
        let mut pending_links: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (relative, metadata) in files_map {
            // Слэши Windows в манифесте нормализуем, «.»-компоненты
            // схлопываются — как в pathlib.PurePath.parts.
            let relative_text = relative.replace('\\', "/");
            let parts: Vec<String> = relative_text
                .split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .map(str::to_owned)
                .collect();
            if relative_text.starts_with('/') || parts.iter().any(|part| part == "..") || parts.is_empty() {
                return Err(format!("Java runtime manifest содержит небезопасный путь: {relative}"));
            }
            let target = safe_path(&destination, &parts)?;
            let Some(metadata) = metadata.as_object() else {
                continue;
            };
            let file_type = match metadata.get("type") {
                Some(Value::String(text)) => text.as_str(),
                None => "file",
                Some(_) => {
                    return Err(format!("Java runtime manifest содержит неизвестный тип файла: {relative}"));
                }
            };
            if !matches!(file_type, "file" | "directory" | "link") {
                return Err(format!("Java runtime manifest содержит неизвестный тип файла: {relative}"));
            }
            if file_type == "directory" {
                ensure_runtime_path(&target, &destination)?;
                fs::create_dir_all(&target)
                    .map_err(|error| format!("Не удалось создать каталог Java runtime: {error}"))?;
                processed_files += 1;
                continue;
            }
            if file_type == "link" {
                // Ссылки разрешаем только относительные и внутрь runtime.
                let link_text = metadata.get("target").and_then(Value::as_str).unwrap_or("").replace('\\', "/");
                let link_target = Path::new(&link_text);
                let Some(target_parent) = target.parent() else {
                    return Err(format!("Небезопасная ссылка в Java runtime: {relative}"));
                };
                let resolved_target = resolve_path(&target_parent.join(link_target));
                if link_target.is_absolute() || !within(&resolved_target, &destination_resolved) {
                    return Err(format!("Небезопасная ссылка в Java runtime: {relative}"));
                }
                ensure_runtime_path(target_parent, &destination)?;
                fs::create_dir_all(target_parent)
                    .map_err(|error| format!("Не удалось создать каталог Java runtime: {error}"))?;
                if is_reparse_point(&target) {
                    return Err(format!("Ссылка в Java runtime является reparse point: {relative}"));
                }
                if target.symlink_metadata().is_ok() {
                    fs::remove_file(&target)
                        .map_err(|error| format!("Не удалось удалить файл Java runtime: {error}"))?;
                }
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(link_target, &target)
                        .map_err(|error| format!("Не удалось создать ссылку Java runtime: {error}"))?;
                    processed_files += 1;
                }
                #[cfg(windows)]
                {
                    pending_links.push((target, resolved_target));
                }
                continue;
            }
            let downloads = metadata
                .get("downloads")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let download = downloads.get("raw").cloned().unwrap_or_else(|| downloads.clone());
            let Some(url) = download.get("url").and_then(Value::as_str).filter(|url| !url.is_empty()) else {
                return Err(format!("Java runtime manifest не содержит URL: {relative}"));
            };
            let target_parent = target.parent().unwrap_or_else(|| Path::new("."));
            ensure_runtime_path(target_parent, &destination)?;
            self.http.download(
                url,
                &target,
                download.get("sha1").and_then(Value::as_str),
                None,
                None,
                download.get("size").and_then(Value::as_u64),
                None,
                None,
                None,
            )?;
            #[cfg(unix)]
            if metadata.get("executable").and_then(Value::as_bool).unwrap_or(false)
                || parts.last().map(|part| part.as_str() == "java").unwrap_or(false)
            {
                add_execute_bits(&target)?;
            }
            processed_files += 1;
        }
        #[cfg(windows)]
        if !pending_links.is_empty() {
            // На Windows ссылки заменяем копией после того, как файлы
            // скачаны; цикл нужен для цепочек ссылок.
            use std::sync::atomic::{AtomicU64, Ordering};
            static LINK_COUNTER: AtomicU64 = AtomicU64::new(0);
            let mut remaining = pending_links;
            loop {
                let mut deferred: Vec<(PathBuf, PathBuf)> = Vec::new();
                let mut progress = false;
                for (target, source) in &remaining {
                    if !source.is_file() || is_reparse_point(source) {
                        deferred.push((target.clone(), source.clone()));
                        continue;
                    }
                    let parent = target.parent().unwrap_or_else(|| Path::new("."));
                    ensure_runtime_path(parent, &destination)?;
                    let suffix = LINK_COUNTER.fetch_add(1, Ordering::Relaxed);
                    let temporary = target.with_file_name(format!(
                        ".{}.{suffix}.part",
                        target.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()
                    ));
                    let copied = fs::copy(source, &temporary).and_then(|_| fs::rename(&temporary, &target));
                    let _ = fs::remove_file(&temporary);
                    copied.map_err(|error| format!("Не удалось скопировать ссылку Java runtime: {error}"))?;
                    processed_files += 1;
                    progress = true;
                }
                if deferred.is_empty() {
                    break;
                }
                if !progress {
                    let unresolved = deferred
                        .iter()
                        .take(3)
                        .map(|(target, _)| {
                            target
                                .strip_prefix(&destination_resolved)
                                .map(|path| path.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(format!("Java runtime manifest содержит неразрешимые ссылки: {unresolved}"));
                }
                remaining = deferred;
            }
        }
        if java_path.exists()
            && within(&resolve_path(&java_path), &destination_resolved)
            && processed_files == total_entries
        {
            if is_reparse_point(&marker) || !runtime_path_without_symlink(marker.parent().unwrap_or(Path::new("")), &destination) {
                return Err("Файл состояния Java runtime является символьной ссылкой".to_owned());
            }
            write_runtime_marker(&marker)?;
            return validate_java_path(&java_path, false).map(Some);
        }
        Ok(None)
    }

    /// Манифест runtime с кэшем на 6 часов; при сетевой ошибке отдаём
    /// устаревший кэш, если он есть.
    fn runtime_manifest(&self) -> Result<Value, String> {
        let cache_path = self.download_cache.join("java-runtime-manifest.json");
        let mut cached: Option<Value> = None;
        if let Some((value, fresh)) = read_cached_runtime_manifest(&cache_path) {
            if fresh {
                return Ok(value.get("manifest").cloned().unwrap_or(Value::Null));
            }
            cached = Some(value);
        }
        let manifest = match self.http.get_json(JAVA_RUNTIME_MANIFEST_URL, None) {
            Ok(manifest) => manifest,
            Err(error) => {
                if let Some(cached) = cached {
                    return Ok(cached.get("manifest").cloned().unwrap_or(Value::Null));
                }
                return Err(error);
            }
        };
        if !manifest.is_object() {
            return Err("Java runtime manifest имеет неожиданный формат".to_owned());
        }
        atomic_json_write(
            &cache_path,
            &json!({ "fetched_at": epoch_seconds(), "manifest": manifest.clone() }),
            true,
        )?;
        Ok(manifest)
    }
}

/// Кэш манифеста: (значение, свежесть). Ссылки и файлы больше 8 МБ не
/// читаем; битый fetched_at бракует весь кэш.
fn read_cached_runtime_manifest(path: &Path) -> Option<(Value, bool)> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.is_symlink() || meta.len() > 8 * 1024 * 1024 {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if !value.is_object() || !value.get("manifest").map(Value::is_object).unwrap_or(false) {
        return None;
    }
    let fetched_at = match value.get("fetched_at") {
        None => 0.0,
        Some(raw) => raw.as_f64()?,
    };
    Some((value, epoch_seconds() - fetched_at < 6.0 * 60.0 * 60.0))
}

pub(crate) fn discover_java_paths() -> Vec<PathBuf> {
    let binary = java_binary_name();
    let mut candidates: Vec<String> = Vec::new();
    let java_home = std::env::var("JAVA_HOME").unwrap_or_default().trim().to_owned();
    if !java_home.is_empty() {
        candidates.push(expand_user(&java_home).join("bin").join(binary).to_string_lossy().to_string());
    }
    if let Some(system) = which(binary) {
        candidates.push(system.to_string_lossy().to_string());
    }
    for root in java_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let candidate = entry.path().join("bin").join(binary);
            if candidate.is_file() {
                candidates.push(candidate.to_string_lossy().to_string());
            }
        }
    }
    let mut result: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for candidate in candidates {
        let resolved = resolve_path(&expand_user(&candidate));
        if seen.contains(&resolved) || !resolved.is_file() || !is_executable(&resolved) {
            continue;
        }
        let Ok(validated) = validate_java_path(&resolved, false) else {
            continue;
        };
        seen.insert(resolved);
        result.push(validated);
    }
    result
}

#[cfg(windows)]
fn java_roots() -> Vec<PathBuf> {
    let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:/Program Files".to_owned());
    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join("AppData").join("Local"));
    vec![
        PathBuf::from(&program_files).join("Java"),
        PathBuf::from(&program_files).join("Eclipse Adoptium"),
        local_appdata.join("Programs").join("Eclipse Adoptium"),
    ]
}

#[cfg(not(windows))]
fn java_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/lib/jvm"), PathBuf::from("/opt")]
}

#[cfg(windows)]
fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

fn runtime_platform() -> Result<String, String> {
    let machine = std::env::consts::ARCH;
    match std::env::consts::OS {
        "windows" => match machine {
            "x86_64" | "amd64" => Ok("windows-x64".to_owned()),
            "arm64" | "aarch64" => Ok("windows-arm64".to_owned()),
            other => Err(format!("Для Windows beta поддерживается x86_64 или ARM64, обнаружено: {other}")),
        },
        "linux" => match machine {
            "x86_64" | "amd64" => Ok("linux".to_owned()),
            other => Err(format!("Для Linux beta поддерживается x86_64, обнаружено: {other}")),
        },
        other => Err(format!("Nivora Launcher beta не поддерживает платформу: {other}")),
    }
}

fn within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn safe_runtime_component(value: &str, label: &str) -> Result<(), String> {
    static COMPONENT: OnceLock<Regex> = OnceLock::new();
    let regex = COMPONENT.get_or_init(|| Regex::new(r"^[A-Za-z0-9._+-]+\z").unwrap());
    if value.is_empty() || !regex.is_match(value) {
        return Err(format!("Небезопасный {label}: {value}"));
    }
    Ok(())
}

/// Путь и все его предки до root не должны быть ссылками.
fn runtime_path_without_symlink(path: &Path, root: &Path) -> bool {
    let mut current = path.to_path_buf();
    loop {
        if is_reparse_point(&current) {
            return false;
        }
        if current == root {
            return true;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        if parent == current || !within(&current, root) {
            return false;
        }
        current = parent.to_path_buf();
    }
}

fn ensure_runtime_path(path: &Path, root: &Path) -> Result<(), String> {
    if !runtime_path_without_symlink(path, root) {
        return Err(format!("Небезопасный путь Java runtime: {}", path.display()));
    }
    Ok(())
}

fn is_reparse_point(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn resolve_path(path: &Path) -> PathBuf {
    canonical_path(path).unwrap_or_else(|_| path.to_path_buf())
}

fn expand_user(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    } else if value == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(value)
}

/// Аналог shutil.which: обход PATH, на unix — ещё проверка бита x.
fn which(name: &str) -> Option<PathBuf> {
    let path_value = std::env::var("PATH").ok()?;
    for directory in std::env::split_paths(&path_value) {
        let candidate = directory.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn java_binary_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

#[cfg(unix)]
fn add_execute_bits(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata =
        fs::metadata(path).map_err(|error| format!("Не удалось изменить права файла Java runtime: {error}"))?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Не удалось изменить права файла Java runtime: {error}"))
}

/// Маркер завершённости runtime: без перехода по ссылке (O_NOFOLLOW),
/// права 0600.
fn write_runtime_marker(marker: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(marker)
            .map_err(|error| format!("Не удалось записать файл состояния Java runtime: {error}"))?;
        file.write_all(b"complete\n")
            .map_err(|error| format!("Не удалось записать файл состояния Java runtime: {error}"))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(marker, b"complete\n")
            .map_err(|error| format!("Не удалось записать файл состояния Java runtime: {error}"))?;
    }
    Ok(())
}

/// Аналог portable_fs.safe_path: относительный путь внутри корня без
/// ссылок и выхода наружу.
fn safe_path(root: &Path, parts: &[String]) -> Result<PathBuf, String> {
    if is_reparse_point(root) {
        return Err("Корневой каталог является ссылкой".to_owned());
    }
    let resolved_root = fs::canonicalize(root).map_err(|_| "Корневой каталог недоступен".to_owned())?;
    let mut current = resolved_root.clone();
    for part in parts {
        validate_path_component(part)?;
        current = current.join(part);
        if is_reparse_point(&current) {
            return Err("Путь содержит символьную ссылку или junction".to_owned());
        }
        if current.symlink_metadata().is_ok() {
            let resolved = fs::canonicalize(&current)
                .map_err(|_| "Путь выходит за пределы корневого каталога".to_owned())?;
            if !resolved.starts_with(&resolved_root) {
                return Err("Путь выходит за пределы корневого каталога".to_owned());
            }
        }
    }
    Ok(current)
}

fn validate_path_component(part: &str) -> Result<(), String> {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part.contains('/')
        || part.contains('\\')
        || part.ends_with(' ')
        || part.ends_with('.')
        || part.chars().any(|character| {
            matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (character as u32) < 32
        })
    {
        return Err("Путь содержит недопустимый компонент".to_owned());
    }
    let device = part.split('.').next().unwrap_or("").to_uppercase();
    // COM1..COM9/LPT1..LPT9 — ровно 4 символа с цифрой 1-9 в конце.
    let bytes = device.as_bytes();
    let port_reserved = (device.starts_with("COM") || device.starts_with("LPT"))
        && bytes.len() == 4
        && (b'1'..=b'9').contains(&bytes[3]);
    if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL") || port_reserved {
        return Err("Путь содержит зарезервированное имя Windows".to_owned());
    }
    Ok(())
}

/// Главная версия Java из вывода «<executable> -version».
pub(crate) fn java_version(java_path: &Path) -> Option<i64> {
    // Замена javaw нужна только на Windows; на unix переменная не меняется.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut executable = java_path.to_path_buf();
    #[cfg(windows)]
    if java_path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase() == "javaw.exe")
        .unwrap_or(false)
    {
        let console_java = java_path.with_file_name("java.exe");
        if console_java.is_file() {
            executable = console_java;
        }
    }
    let (stdout, stderr) = run_version_command(&executable)?;
    let output = format!("{stderr}{stdout}");
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let regex = VERSION.get_or_init(|| Regex::new(r#"version\s+"([^"]+)"#).unwrap());
    let captures = regex.captures(&output)?;
    let mut value = captures[1].to_string();
    if value.starts_with("1.") {
        value = value.split('.').nth(1).unwrap_or_default().to_owned();
    }
    value.split('.').next()?.parse::<i64>().ok()
}

/// Запуск с таймаутом 10 секунд, как subprocess.run(timeout=10).
fn run_version_command(executable: &Path) -> Option<(String, String)> {
    let mut child = Command::new(executable)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    Some((stdout, stderr))
}

/// Проверка Java executable без запуска: путь из настроек — недоверенный
/// ввод, принимаем только java/javaw в bin каталога JDK/JRE, запись для
/// группы/всех и чужой владение отвергаем.
pub(crate) fn validate_java_path(value: &Path, allow_directory: bool) -> Result<PathBuf, String> {
    let mut candidate = expand_user(&value.to_string_lossy());
    if allow_directory && candidate.is_dir() {
        candidate = candidate.join("bin").join(java_binary_name());
    }
    // Без verbatim-префикса \\?\: JVM, запущенная по такому пути, получает
    // java.home вида \\?\C:\… и ломает загрузку модулей у Forge/NeoForge.
    let resolved = canonical_path(&candidate)
        .map_err(|_| format!("Java executable не найден: {}", candidate.display()))?;
    let allowed: [&str; 2] = if cfg!(windows) { ["java.exe", "javaw.exe"] } else { ["java", "javaw"] };
    let name_ok = resolved
        .file_name()
        .map(|name| allowed.contains(&name.to_string_lossy().to_lowercase().as_str()))
        .unwrap_or(false);
    let bin_ok = resolved
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_lowercase() == "bin")
        .unwrap_or(false);
    if !name_ok || !bin_ok || !resolved.is_file() || !is_executable(&resolved) {
        return Err("Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let root = resolved.parent().and_then(|bin| bin.parent());
        let (Some(bin_path), Some(root_path)) = (resolved.parent(), root) else {
            return Err("Не удалось проверить права Java executable".to_owned());
        };
        let executable_meta =
            fs::metadata(&resolved).map_err(|_| "Не удалось проверить права Java executable".to_owned())?;
        let bin_meta = fs::metadata(bin_path).map_err(|_| "Не удалось проверить права Java executable".to_owned())?;
        let root_meta = fs::metadata(root_path).map_err(|_| "Не удалось проверить права Java executable".to_owned())?;
        let euid = unsafe { libc::geteuid() };
        let owned = |uid: u32| uid == euid || uid == 0;
        if !owned(executable_meta.uid()) || !owned(bin_meta.uid()) || !owned(root_meta.uid()) {
            return Err("Java executable и каталог JDK/JRE должны принадлежать текущему пользователю или root".to_owned());
        }
        let group_or_world_writable = |mode: u32| mode & 0o022 != 0;
        if group_or_world_writable(executable_meta.mode())
            || group_or_world_writable(bin_meta.mode())
            || group_or_world_writable(root_meta.mode())
        {
            return Err("Java executable и каталог JDK/JRE не должны быть доступны для записи группе или всем".to_owned());
        }
    }
    Ok(resolved)
}

pub(crate) fn safe_process_environment(compatibility: bool) -> HashMap<String, String> {
    let raw: Vec<(String, String)> = std::env::vars().collect();
    if compatibility {
        // Режим совместимости: всё, кроме секретоподобных переменных.
        return raw
            .into_iter()
            .filter(|(key, _)| {
                let upper = key.to_uppercase();
                !(upper.contains("PASSWORD")
                    || upper.contains("SECRET")
                    || upper.contains("TOKEN")
                    || upper.contains("PRIVATE_KEY"))
            })
            .collect();
    }
    #[cfg(windows)]
    let passthrough: &[&str] = &[
        "PATH", "SYSTEMROOT", "WINDIR", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "TEMP",
        "TMP", "PATHEXT", "COMSPEC", "PROCESSOR_ARCHITECTURE", "NUMBER_OF_PROCESSORS", "JAVA_HOME", "JDK_HOME",
    ];
    #[cfg(not(windows))]
    let passthrough: &[&str] = &[
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TMPDIR", "TMP", "TEMP", "DISPLAY",
        "WAYLAND_DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS", "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP", "XDG_SESSION_TYPE", "XDG_RUNTIME_DIR", "PULSE_SERVER", "PIPEWIRE_REMOTE",
        "PIPEWIRE_LATENCY", "DESKTOP_SESSION", "GDK_BACKEND", "QT_QPA_PLATFORM", "QT_SCALE_FACTOR",
        "LD_LIBRARY_PATH", "JAVA_HOME", "JDK_HOME", "LIBGL_ALWAYS_SOFTWARE", "LIBGL_ALWAYS_INDIRECT",
        "MESA_LOADER_DRIVER_OVERRIDE", "GALLIUM_DRIVER", "__GLX_VENDOR_LIBRARY_NAME",
        "__NV_PRIME_RENDER_OFFLOAD", "__VK_LAYER_NV_optimus",
    ];
    let mut result = HashMap::new();
    for (key, value) in raw {
        // На Windows сравнение регистронезависимое, на unix — точное
        // плюс префикс LC_.
        let keep = if cfg!(windows) {
            passthrough.contains(&key.to_uppercase().as_str())
        } else {
            passthrough.contains(&key.as_str()) || key.starts_with("LC_")
        };
        if keep {
            result.insert(key, value);
        }
    }
    result
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

// ----------------------------------------------------------- Тесты ----

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-runtime-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn make_java(root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let jdk = root.join("jdk");
        let bin = jdk.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let java = bin.join("java");
        fs::write(&java, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&jdk, fs::Permissions::from_mode(0o755)).unwrap();
        java
    }

    #[cfg(unix)]
    #[test]
    fn validate_java_path_accepts_owned_executable() {
        let root = temp_root("valid");
        let java = make_java(&root);
        let resolved = validate_java_path(&java, false).unwrap();
        assert_eq!(resolved, canonical_path(&java).unwrap());
        // Каталог JDK с allow_directory подставляет bin/java.
        assert_eq!(validate_java_path(&root.join("jdk"), true).unwrap(), resolved);
    }

    #[cfg(unix)]
    #[test]
    fn validate_java_path_rejects_wrong_shape_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("invalid");
        let bin = root.join("jre").join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("javac"), b"x").unwrap();
        let error = validate_java_path(&bin.join("javac"), false).unwrap_err();
        assert_eq!(
            error,
            "Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin"
        );
        let missing = validate_java_path(&root.join("nope").join("bin").join("java"), false).unwrap_err();
        assert!(missing.contains("Java executable не найден"));
        // Доступность записи для группы — отказ.
        let java = make_java(&root);
        fs::set_permissions(&java, fs::Permissions::from_mode(0o775)).unwrap();
        let error = validate_java_path(&java, false).unwrap_err();
        assert!(error.contains("не должны быть доступны для записи"));
    }

    #[test]
    fn java_version_of_missing_binary_is_none() {
        assert!(java_version(Path::new("/nonexistent-java-binary")).is_none());
    }

    #[test]
    fn runtime_platform_is_supported_or_russian_error() {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") | ("linux", "amd64") => assert_eq!(runtime_platform().unwrap(), "linux"),
            ("windows", "x86_64") | ("windows", "amd64") => assert_eq!(runtime_platform().unwrap(), "windows-x64"),
            _ => assert!(runtime_platform().is_err()),
        }
    }

    #[test]
    fn runtime_components_are_validated() {
        assert!(safe_runtime_component("java-runtime-delta", "runtime component").is_ok());
        assert_eq!(
            safe_runtime_component("a/b", "runtime component").unwrap_err(),
            "Небезопасный runtime component: a/b"
        );
        assert_eq!(
            safe_runtime_component("", "runtime version").unwrap_err(),
            "Небезопасный runtime version: "
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_paths_reject_symlinked_directories() {
        let root = temp_root("symlink");
        let inner = root.join("runtime");
        fs::create_dir_all(&inner).unwrap();
        std::os::unix::fs::symlink(&inner, root.join("link")).unwrap();
        assert!(runtime_path_without_symlink(&inner, &root));
        assert!(!runtime_path_without_symlink(&root.join("link"), &root));
        assert!(ensure_runtime_path(&root.join("link").join("bin"), &root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn safe_path_confines_to_root() {
        let root = temp_root("safepath");
        let runtime = root.join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        assert_eq!(
            safe_path(&runtime, &["bin".to_owned()]).unwrap(),
            fs::canonicalize(&runtime).unwrap().join("bin")
        );
        assert_eq!(
            safe_path(&runtime, &["..".to_owned()]).unwrap_err(),
            "Путь содержит недопустимый компонент"
        );
        let outside = root.join("outside.txt");
        fs::write(&outside, b"x").unwrap();
        std::os::unix::fs::symlink(&outside, runtime.join("escape")).unwrap();
        assert_eq!(
            safe_path(&runtime, &["escape".to_owned()]).unwrap_err(),
            "Путь содержит символьную ссылку или junction"
        );
    }

    #[test]
    fn process_environment_passthrough() {
        std::env::set_var("NIVORA_TEST_SECRET", "hunter2");
        std::env::set_var("LC_MESSAGES", "ru_RU.UTF-8");
        let compatibility = safe_process_environment(true);
        assert!(!compatibility.contains_key("NIVORA_TEST_SECRET"));
        let strict = safe_process_environment(false);
        assert!(!strict.contains_key("NIVORA_TEST_SECRET"));
        assert!(strict.contains_key("LC_MESSAGES"));
    }

    #[test]
    fn discovered_java_paths_are_files() {
        for path in discover_java_paths() {
            assert!(path.is_file(), "не файл: {}", path.display());
        }
    }
}
