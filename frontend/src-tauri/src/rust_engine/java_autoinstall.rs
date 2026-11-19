use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::config::ConfigStore;
use super::net::client_from_settings;
use super::state::launcher_roots;

fn err(message: &str) -> String {
    message.to_owned()
}

const STAGING_DIR: &str = "_staging";

fn target_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

fn target_arch() -> &'static str {
    if std::env::consts::ARCH == "aarch64" {
        "aarch64"
    } else {
        "x64"
    }
}

fn runtimes_root() -> PathBuf {
    launcher_roots().data_home.join("runtimes").join("java")
}

fn java_executable_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn sanitize_dir_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_lowercase()
}

struct InstalledRuntime {
    path: String,
    version: u32,
    label: String,
}

fn describe(java_path: &Path) -> Result<InstalledRuntime, String> {
    let validated = super::system::java_validate(&json!({ "path": java_path.to_string_lossy() }))?;
    let version = validated
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let canonical = validated
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(InstalledRuntime {
        path: canonical,
        version,
        label: format!("Java {version} (Temurin)"),
    })
}

fn find_java_binary(root: &Path) -> Option<PathBuf> {
    let exe = java_executable_name();
    let mut queue: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut index = 0;
    while index < queue.len() {
        let (dir, depth) = queue[index].clone();
        index += 1;
        let candidate = dir.join("bin").join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
        if depth >= 2 {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    queue.push((path, depth + 1));
                }
            }
        }
    }
    None
}

fn existing_runtime(install_dir: &Path) -> Option<InstalledRuntime> {
    find_java_binary(install_dir).and_then(|java| describe(&java).ok())
}

fn extract_and_validate(archive: &Path, install_dir: &Path) -> Result<InstalledRuntime, String> {
    fs::create_dir_all(install_dir)
        .map_err(|error| err(&format!("Не удалось подготовить каталог: {error}")))?;
    let name = archive
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let extracted = if name.ends_with(".zip") {
        let file = fs::File::open(archive)
            .map_err(|error| err(&format!("Не удалось открыть архив: {error}")))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|error| err(&format!("Архив Java повреждён: {error}")))?;
        zip.extract(install_dir)
            .map_err(|error| err(&format!("Не удалось распаковать Java: {error}")))
    } else {
        let file = fs::File::open(archive)
            .map_err(|error| err(&format!("Не удалось открыть архив: {error}")))?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        tar.unpack(install_dir)
            .map_err(|error| err(&format!("Не удалось распаковать Java: {error}")))
    };
    if let Err(error) = extracted {
        let _ = fs::remove_dir_all(install_dir);
        return Err(error);
    }
    let Some(java) = find_java_binary(install_dir) else {
        let _ = fs::remove_dir_all(install_dir);
        return Err(err("В архиве Java не найден исполняемый файл"));
    };
    match describe(&java) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let _ = fs::remove_dir_all(install_dir);
            Err(err(&format!(
                "Установленная Java не прошла проверку: {error}"
            )))
        }
    }
}

/// скачивает и устанавливает Temurin JRE нужной версии через Adoptium API
pub fn java_autoinstall(request: &Value) -> Result<Value, String> {
    let feature = request.get("feature").and_then(Value::as_u64).unwrap_or(21);
    if !(8..=30).contains(&feature) {
        return Err(err("Поддерживаются версии Java от 8 до 30"));
    }
    let config = ConfigStore::load()?;
    let operation_id = request
        .get("operation_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let http = client_from_settings(config.settings(), operation_id)?;
    let root = runtimes_root();
    fs::create_dir_all(&root)
        .map_err(|error| err(&format!("Не удалось создать каталог рантаймов: {error}")))?;

    let url = format!(
        "https://api.adoptium.net/v3/assets/feature_releases/{feature}/ga?architecture={arch}&heap_size=normal&image_type=jre&jvm_impl=hotspot&os={os}&project=jdk&sort_method=DEFAULT&sort_order=DESC&page_size=1",
        arch = target_arch(),
        os = target_os(),
    );
    let assets = http.get_json(&url, None)?;
    let asset = assets
        .as_array()
        .and_then(|entries| entries.first())
        .ok_or_else(|| err(&format!("Adoptium не вернул сборку JRE для Java {feature}")))?;
    let release_name = asset
        .get("release_name")
        .and_then(Value::as_str)
        .unwrap_or("temurin")
        .to_owned();
    let package = asset
        .get("binaries")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .and_then(|binary| binary.get("package"))
        .ok_or_else(|| err("В ответе Adoptium нет пакета для скачивания"))?;
    let link = package
        .get("link")
        .and_then(Value::as_str)
        .ok_or_else(|| err("В пакете нет ссылки"))?;
    let checksum = package
        .get("checksum")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let size = package.get("size").and_then(Value::as_u64);
    let file_name = package
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("temurin-jre.bin")
        .rsplit('/')
        .next()
        .unwrap_or("temurin-jre.bin")
        .to_owned();

    let install_dir = root.join(format!("temurin-{}", sanitize_dir_name(&release_name)));
    if let Some(existing) = existing_runtime(&install_dir) {
        return Ok(json!({
            "path": existing.path,
            "version": existing.version,
            "label": existing.label,
            "already_installed": true,
            "release_name": release_name,
        }));
    }
    if install_dir.is_dir() {
        fs::remove_dir_all(&install_dir)
            .map_err(|error| err(&format!("Не удалось очистить старый каталог: {error}")))?;
    }

    let staging = root.join(STAGING_DIR);
    fs::create_dir_all(&staging)
        .map_err(|error| err(&format!("Не удалось создать временный каталог: {error}")))?;
    let archive_path = staging.join(&file_name);
    if archive_path.is_file() {
        let _ = fs::remove_file(&archive_path);
    }
    let sha256 = if checksum.is_empty() {
        None
    } else {
        Some(checksum)
    };
    let downloaded = http.download(
        link,
        &archive_path,
        None,
        sha256,
        None,
        size,
        None,
        None,
        None,
    )?;
    if !downloaded {
        return Err(err("Загрузка Java не удалась"));
    }

    let result = extract_and_validate(&archive_path, &install_dir);
    let _ = fs::remove_file(&archive_path);
    let outcome = result?;
    Ok(json!({
        "path": outcome.path,
        "version": outcome.version,
        "label": outcome.label,
        "already_installed": false,
        "release_name": release_name,
    }))
}

#[cfg(test)]
mod tests {
    use super::sanitize_dir_name;

    #[test]
    fn sanitize_keeps_safe_characters_only() {
        assert_eq!(sanitize_dir_name("jdk-21.0.5+11"), "jdk-21.0.5-11");
        assert_eq!(sanitize_dir_name("OpenJDK17U-jre"), "openjdk17u-jre");
    }
}
