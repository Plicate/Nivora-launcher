//! Встроенный проводник: список каталогов и чтение файлов для окна «Файлы».
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;

const CHUNK_BYTES: u64 = 6 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

fn err(message: &str) -> String {
    format!("Встроенный проводник: {message}")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn allowed_roots() -> Vec<(String, PathBuf)> {
    let mut roots = Vec::new();
    if let Some(home) = home_dir() {
        if let Ok(canonical) = home.canonicalize() {
            roots.push(("Дом".to_owned(), canonical));
        }
    }
    let engine_roots = super::state::launcher_roots();
    let game_dir = super::state::settings_game_dir(&engine_roots.config_home, &engine_roots.data_home);
    for (label, path) in [
        ("Папка игры".to_owned(), game_dir),
        ("Данные лаунчера".to_owned(), engine_roots.data_home.clone()),
        ("Настройки лаунчера".to_owned(), engine_roots.config_home.clone()),
    ] {
        if let Ok(canonical) = path.canonicalize() {
            roots.push((label, canonical));
        }
    }
    roots
}

pub(crate) fn canonical_within_allowed(raw: &str) -> Result<PathBuf, String> {
    let requested = Path::new(raw);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        // относительный путь трактуем от первого корня (обычно — дом).
        allowed_roots()
            .first()
            .map(|(_, root)| root.join(requested))
            .ok_or_else(|| err("не найден домашний каталог"))?
    };
    let canonical = candidate.canonicalize().map_err(|_| err(&format!("каталог недоступен: {raw}")))?;
    let allowed = allowed_roots();
    if !allowed.iter().any(|(_, root)| canonical.starts_with(root)) {
        return Err(err(&format!("путь вне обозреваемых каталогов: {raw}")));
    }
    Ok(canonical)
}

fn entry_size(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().filter(|metadata| metadata.is_file()).map(|metadata| metadata.len())
}

fn entry_modified_ms(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as f64)
        .unwrap_or(0.0)
}

/// список каталога: папки сначала, сортировка без учёта регистра.
/// `show_hidden: true` показывает dot-файлы (по умолчанию скрыты,
/// `.minecraft` виден всегда — туда ставятся сторонние лаунчеры).
/// читает каталог: папки сначала, сортировка без учёта регистра.
/// Отделено от `fs_browse`, чтобы тестироваться без env/корней.
fn read_entries(path: &Path, show_hidden: bool) -> Result<(Vec<(String, PathBuf)>, Vec<(String, PathBuf)>), String> {
    let mut directories: Vec<(String, PathBuf)> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut read = fs::read_dir(path).map_err(|error| err(&format!("не удалось открыть каталог: {error}")))?;
    while let Some(entry) = read.next() {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') && name != ".minecraft" {
            // скрытые служебные точки не показываем, но .minecraft оставим:
            // именно туда ставятся сторонние лаунчеры.
            continue;
        }
        let entry_path = entry.path();
        if entry_path.is_dir() {
            directories.push((name, entry_path));
        } else {
            files.push((name, entry_path));
        }
        if directories.len() + files.len() >= MAX_ENTRIES {
            break;
        }
    }
    let collator = |a: &(String, PathBuf), b: &(String, PathBuf)| a.0.to_lowercase().cmp(&b.0.to_lowercase());
    directories.sort_by(collator);
    files.sort_by(collator);
    Ok((directories, files))
}

pub fn fs_browse(request: &Value) -> Result<Value, String> {
    let raw_path = request.get("path").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
    let show_hidden = request.get("show_hidden").and_then(Value::as_bool).unwrap_or(false);
    let path = match raw_path {
        Some(value) => canonical_within_allowed(value)?,
        None => allowed_roots()
            .first()
            .map(|(_, root)| root.clone())
            .ok_or_else(|| err("не найден стартовый каталог"))?,
    };
    let allowed = allowed_roots();
    let root_label = allowed
        .iter()
        .find(|(_, root)| path.starts_with(root))
        .map(|(label, root)| json!({ "label": label, "path": root.to_string_lossy() }));
    // выше корня подниматься нельзя: родитель за пределами всех корней — null.
    let parent = path
        .parent()
        .filter(|parent| allowed.iter().any(|(_, root)| parent.starts_with(root)))
        .map(|parent| parent.to_string_lossy().into_owned());

    let (directories, files) = read_entries(&path, show_hidden)?;

    let entries: Vec<Value> = directories
        .into_iter()
        .map(|(name, path)| {
            json!({
                "name": name,
                "kind": "dir",
                "size": null,
                "modified_at": entry_modified_ms(&path),
            })
        })
        .chain(files.into_iter().map(|(name, path)| {
            json!({
                "name": name,
                "kind": "file",
                "size": entry_size(&path),
                "modified_at": entry_modified_ms(&path),
            })
        }))
        .collect();

    Ok(json!({
        "path": path.to_string_lossy(),
        "parent": parent,
        "root": root_label,
        "roots": allowed
            .iter()
            .map(|(label, root)| json!({ "label": label, "path": root.to_string_lossy() }))
            .collect::<Vec<_>>(),
        "entries": entries,
    }))
}

pub fn fs_read_file(request: &Value) -> Result<Value, String> {
    let raw_path = request
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| err("не указан путь файла"))?;
    let path = canonical_within_allowed(raw_path)?;
    let metadata = fs::metadata(&path).map_err(|error| err(&format!("файл недоступен: {error}")))?;
    if !metadata.is_file() {
        return Err(err("это не файл"));
    }
    let size = metadata.len();
    if size > MAX_FILE_BYTES {
        return Err(err(&format!("файл больше лимита {} MiB", MAX_FILE_BYTES / 1024 / 1024)));
    }
    let offset = request.get("offset").and_then(Value::as_u64).unwrap_or(0).min(size);
    let length = request.get("length").and_then(Value::as_u64).unwrap_or(CHUNK_BYTES).min(CHUNK_BYTES);
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(&path).map_err(|error| err(&format!("не удалось открыть файл: {error}")))?;
    file.seek(SeekFrom::Start(offset)).map_err(|error| err(&format!("не удалось прочитать файл: {error}")))?;
    let mut buffer = vec![0u8; length.min(size - offset) as usize];
    file.read_exact(&mut buffer).map_err(|error| err(&format!("не удалось прочитать файл: {error}")))?;
    let data = base64::engine::general_purpose::STANDARD.encode(&buffer);
    let read = buffer.len() as u64;
    Ok(json!({
        "size": size,
        "offset": offset,
        "length": read,
        "eof": offset + read >= size,
        "data": data,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("nivora-fsbrowser-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(entries: &[(String, PathBuf)]) -> Vec<String> {
        entries.iter().map(|(name, _)| name.clone()).collect()
    }

    #[test]
    fn hidden_files_are_filtered_unless_requested_and_dot_minecraft_stays() {
        let sandbox = Sandbox::new("hidden");
        fs::write(sandbox.0.join("visible.txt"), b"x").unwrap();
        fs::write(sandbox.0.join(".hidden"), b"x").unwrap();
        fs::create_dir_all(sandbox.0.join(".config")).unwrap();
        fs::create_dir_all(sandbox.0.join(".minecraft")).unwrap();
        fs::create_dir_all(sandbox.0.join("mods")).unwrap();

        let (dirs, files) = read_entries(&sandbox.0, false).unwrap();
        assert_eq!(names(&dirs), vec![".minecraft".to_owned(), "mods".to_owned()]);
        assert_eq!(names(&files), vec!["visible.txt".to_owned()]);

        let (dirs, files) = read_entries(&sandbox.0, true).unwrap();
        assert_eq!(names(&dirs), vec![".config".to_owned(), ".minecraft".to_owned(), "mods".to_owned()]);
        assert_eq!(names(&files), vec![".hidden".to_owned(), "visible.txt".to_owned()]);
    }

    #[test]
    fn missing_directory_is_a_readable_error() {
        let sandbox = Sandbox::new("missing");
        let error = read_entries(&sandbox.0.join("nope"), false).unwrap_err();
        assert!(error.contains("Встроенный проводник"), "{error}");
        assert!(error.contains("не удалось открыть каталог"), "{error}");
    }
}
