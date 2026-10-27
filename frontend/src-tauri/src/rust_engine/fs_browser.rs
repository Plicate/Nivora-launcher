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
        // относительный путь считаем от первого корня, обычно домашнего каталога
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

/// список каталога: сначала папки, сортировка без учёта регистра
/// скрытые файлы показываются по флагу; `.minecraft` виден всегда
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
    // не поднимаемся выше корня: для внешнего родителя возвращаем `null`
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

fn within_roots(path: &Path, roots: &[(String, PathBuf)]) -> bool {
    roots.iter().any(|(_, root)| path.starts_with(root))
}

fn safe_entry_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name == "." || name == ".." || name.chars().any(|ch| ch == '/' || ch == '\\' || ch == '\0') || name.len() > 255 {
        return Err(err("некорректное имя файла или папки"));
    }
    Ok(name)
}

fn canonical_directory(raw: &str, roots: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let requested = Path::new(raw);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        roots.first().map(|(_, root)| root.join(requested)).ok_or_else(|| err("не найден домашний каталог"))?
    };
    let canonical = candidate.canonicalize().map_err(|_| err("каталог недоступен"))?;
    if !canonical.is_dir() || !within_roots(&canonical, roots) {
        return Err(err("каталог вне обозреваемых папок"));
    }
    Ok(canonical)
}

fn resolve_existing_entry(raw: &str, roots: &[(String, PathBuf)]) -> Result<(PathBuf, fs::Metadata), String> {
    let requested = Path::new(raw);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        roots.first().map(|(_, root)| root.join(requested)).ok_or_else(|| err("не найден домашний каталог"))?
    };
    let name = candidate.file_name().ok_or_else(|| err("некорректный путь"))?;
    let parent = candidate.parent().ok_or_else(|| err("некорректный путь"))?.canonicalize().map_err(|_| err("родительская папка недоступна"))?;
    if !within_roots(&parent, roots) {
        return Err(err("путь вне обозреваемых папок"));
    }
    let target = parent.join(name);
    let metadata = fs::symlink_metadata(&target).map_err(|_| err("файл или папка больше недоступны"))?;
    if metadata.file_type().is_symlink() {
        return Err(err("операция с символическими ссылками запрещена"));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(err("поддерживаются только обычные файлы и папки"));
    }
    Ok((target, metadata))
}

fn create_file_in_roots(directory: &str, name: &str, roots: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let name = safe_entry_name(name)?;
    let parent = canonical_directory(directory, roots)?;
    let target = parent.join(name);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| err(&format!("не удалось создать файл: {error}")))?;
    Ok(target)
}

fn create_directory_in_roots(directory: &str, name: &str, roots: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let name = safe_entry_name(name)?;
    let parent = canonical_directory(directory, roots)?;
    let target = parent.join(name);
    fs::create_dir(&target).map_err(|error| err(&format!("не удалось создать папку: {error}")))?;
    Ok(target)
}

fn rename_entry_in_roots(raw: &str, new_name: &str, roots: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let new_name = safe_entry_name(new_name)?;
    let (source, _) = resolve_existing_entry(raw, roots)?;
    let target = source.parent().ok_or_else(|| err("некорректный путь"))?.join(new_name);
    match fs::symlink_metadata(&target) {
        Ok(_) => return Err(err("файл или папка с таким именем уже существует")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(err(&format!("не удалось проверить новое имя: {error}"))),
    }
    fs::rename(&source, &target).map_err(|error| err(&format!("не удалось переименовать: {error}")))?;
    Ok(target)
}

fn delete_entry_in_roots(raw: &str, roots: &[(String, PathBuf)]) -> Result<bool, String> {
    let (target, metadata) = resolve_existing_entry(raw, roots)?;
    if roots.iter().any(|(_, root)| *root == target) {
        return Err(err("нельзя удалить корневую папку"));
    }
    if metadata.is_dir() {
        fs::remove_dir_all(&target).map_err(|error| err(&format!("не удалось удалить папку: {error}")))?;
        Ok(true)
    } else {
        fs::remove_file(&target).map_err(|error| err(&format!("не удалось удалить файл: {error}")))?;
        Ok(false)
    }
}

pub fn fs_create_file(request: &Value) -> Result<Value, String> {
    let directory = request.get("directory").and_then(Value::as_str).ok_or_else(|| err("не указана папка"))?;
    let name = request.get("name").and_then(Value::as_str).ok_or_else(|| err("не указано имя файла"))?;
    let path = create_file_in_roots(directory, name, &allowed_roots())?;
    Ok(json!({ "created": true, "path": path.to_string_lossy() }))
}

pub fn fs_create_directory(request: &Value) -> Result<Value, String> {
    let directory = request.get("directory").and_then(Value::as_str).ok_or_else(|| err("не указана папка"))?;
    let name = request.get("name").and_then(Value::as_str).ok_or_else(|| err("не указано имя папки"))?;
    let path = create_directory_in_roots(directory, name, &allowed_roots())?;
    Ok(json!({ "created": true, "path": path.to_string_lossy() }))
}

pub fn fs_rename(request: &Value) -> Result<Value, String> {
    let path = request.get("path").and_then(Value::as_str).ok_or_else(|| err("не указан путь"))?;
    let new_name = request.get("new_name").and_then(Value::as_str).ok_or_else(|| err("не указано новое имя"))?;
    let path = rename_entry_in_roots(path, new_name, &allowed_roots())?;
    Ok(json!({ "renamed": true, "path": path.to_string_lossy() }))
}

pub fn fs_delete(request: &Value) -> Result<Value, String> {
    let path = request.get("path").and_then(Value::as_str).ok_or_else(|| err("не указан путь"))?;
    let directory = delete_entry_in_roots(path, &allowed_roots())?;
    Ok(json!({ "deleted": true, "directory": directory }))
}

/// открывает файл или папку в системном проводнике в пределах разрешённых каталогов
pub fn fs_open_file(request: &Value) -> Result<Value, String> {
    let raw_path = request
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| err("не указан путь файла"))?;
    let path = canonical_within_allowed(raw_path)?;
    if !path.is_file() {
        return Err(err("это не файл"));
    }
    super::platform_support::open_path(&path).map_err(|error| err(&format!("не удалось открыть файл: {error}")))?;
    Ok(json!({ "opened": true, "path": path.to_string_lossy() }))
}

pub fn fs_reveal(request: &Value) -> Result<Value, String> {
    let raw_path = request
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let path = match raw_path {
        Some(value) => canonical_within_allowed(value)?,
        None => allowed_roots()
            .first()
            .map(|(_, root)| root.clone())
            .ok_or_else(|| err("не найден стартовый каталог"))?,
    };
    #[cfg_attr(not(windows), allow(unused_variables))]
    let (folder, selected_file) = if path.is_file() {
        (path.parent().map(Path::to_path_buf).unwrap_or(path.clone()), Some(path.clone()))
    } else {
        (path, None)
    };
    #[cfg(windows)]
    {
        let mut command = super::platform::command("explorer");
        match &selected_file {
            Some(file) => {
                command.arg(format!("/select,{}", file.to_string_lossy()));
            }
            None => {
                command.arg(folder.as_os_str());
            }
        }
        command
            .spawn()
            .map_err(|error| err(&format!("не удалось открыть проводник: {error}")))?;
        return Ok(json!({ "opened": true, "path": folder.to_string_lossy() }));
    }
    #[cfg(not(windows))]
    {
        super::platform_support::open_path(&folder).map_err(|error| err(&format!("не удалось открыть папку: {error}")))?;
        Ok(json!({ "opened": true, "path": folder.to_string_lossy() }))
    }
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

/// записывает файл в разрешённый каталог; существующую символьную ссылку не перезаписывает
pub fn fs_write_file(request: &Value) -> Result<Value, String> {
    let raw_path = request
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| err("не указан путь файла"))?;
    let data = request.get("data").and_then(Value::as_str).unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| err("данные файла повреждены (base64)"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(err(&format!("файл больше лимита {} MiB", MAX_FILE_BYTES / 1024 / 1024)));
    }
    let requested = Path::new(raw_path);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        allowed_roots()
            .first()
            .map(|(_, root)| root.join(requested))
            .ok_or_else(|| err("не найден домашний каталог"))?
    };
    let file_name = candidate
        .file_name()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or_else(|| err("некорректное имя файла"))?;
    let parent = candidate.parent().ok_or_else(|| err("некорректный путь файла"))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| err(&format!("каталог недоступен: {}", parent.to_string_lossy())))?;
    let allowed = allowed_roots();
    if !allowed.iter().any(|(_, root)| canonical_parent.starts_with(root)) {
        return Err(err(&format!("путь вне обозреваемых каталогов: {raw_path}")));
    }
    let target = canonical_parent.join(file_name);
    if fs::symlink_metadata(&target).map(|meta| meta.file_type().is_symlink()).unwrap_or(false) {
        return Err(err("символьные ссылки не перезаписываются"));
    }
    fs::write(&target, &bytes).map_err(|error| err(&format!("не удалось записать файл: {error}")))?;
    Ok(json!({
        "written": true,
        "path": target.to_string_lossy(),
        "size": bytes.len(),
    }))
}

const MAX_TEXT_EDITOR_BYTES: usize = 2 * 1024 * 1024;

fn text_editor_target(raw_path: &str, allowed: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let requested = Path::new(raw_path);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        allowed.first().map(|(_, root)| root.join(requested)).ok_or_else(|| err("не найден домашний каталог"))?
    };
    let file_name = candidate
        .file_name()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or_else(|| err("некорректное имя файла"))?;
    let parent = candidate.parent().ok_or_else(|| err("некорректный путь файла"))?;
    let canonical_parent = parent.canonicalize().map_err(|_| err("каталог файла недоступен"))?;
    if !allowed.iter().any(|(_, root)| canonical_parent.starts_with(root)) {
        return Err(err("путь вне обозреваемых каталогов"));
    }
    let target = canonical_parent.join(file_name);
    let metadata = fs::symlink_metadata(&target).map_err(|_| err("файл больше недоступен"))?;
    if metadata.file_type().is_symlink() {
        return Err(err("редактор не перезаписывает символьные ссылки"));
    }
    if !metadata.is_file() {
        return Err(err("это не обычный файл"));
    }
    Ok(target)
}

fn open_text_file_no_follow(path: &Path) -> Result<fs::File, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| err(&format!("не удалось безопасно открыть файл: {error}")))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| err(&format!("не удалось безопасно открыть файл: {error}")))?;
        let metadata = file.metadata().map_err(|error| err(&format!("не удалось проверить файл: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(err("редактор не перезаписывает символьные ссылки"));
        }
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let metadata = fs::symlink_metadata(path).map_err(|error| err(&format!("файл больше недоступен: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(err("редактор не перезаписывает символьные ссылки"));
        }
        fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|error| err(&format!("не удалось безопасно открыть файл: {error}")))
    }
}

fn read_text_file_snapshot(path: &Path) -> Result<(fs::Metadata, Vec<u8>), String> {
    use std::io::Read;
    let file = open_text_file_no_follow(path)?;
    let metadata = file.metadata().map_err(|error| err(&format!("не удалось проверить файл: {error}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(err("это не обычный файл"));
    }
    if metadata.len() > MAX_TEXT_EDITOR_BYTES as u64 {
        return Err(err("редактор поддерживает файлы до 2 MiB"));
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_TEXT_EDITOR_BYTES + 1) as u64)
        .read_to_end(&mut contents)
        .map_err(|error| err(&format!("не удалось проверить файл перед сохранением: {error}")))?;
    if contents.len() > MAX_TEXT_EDITOR_BYTES {
        return Err(err("редактор поддерживает файлы до 2 MiB"));
    }
    Ok((metadata, contents))
}

fn same_file_snapshot(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    {
        before.len() == after.len() && before.modified().ok() == after.modified().ok()
    }
}

fn same_file_permissions(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.mode() == after.mode()
    }
    #[cfg(not(unix))]
    {
        before.permissions().readonly() == after.permissions().readonly()
    }
}

fn save_text_file_in_roots(raw_path: &str, expected: &[u8], contents: &[u8], allowed: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    if expected.len() > MAX_TEXT_EDITOR_BYTES || contents.len() > MAX_TEXT_EDITOR_BYTES {
        return Err(err("редактор поддерживает файлы до 2 MiB"));
    }
    let target = text_editor_target(raw_path, allowed)?;
    let (metadata, original_contents) = read_text_file_snapshot(&target)?;
    if metadata.permissions().readonly() {
        return Err(err("файл доступен только для чтения"));
    }
    if original_contents != expected {
        return Err(err("файл изменился вне лаунчера. Закройте его и откройте заново, чтобы не затереть чужие изменения."));
    }
    let parent = target.parent().ok_or_else(|| err("некорректный путь файла"))?;
    let name = target.file_name().ok_or_else(|| err("некорректное имя файла"))?.to_string_lossy();
    let (temporary, mut file) = (0..8)
        .find_map(|attempt| {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            let path = parent.join(format!(".{name}.nivora-{}-{stamp}-{attempt}.tmp", std::process::id()));
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => Some(Ok((path, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(err(&format!("не удалось подготовить сохранение: {error}")))),
            }
        })
        .transpose()?
        .ok_or_else(|| err("не удалось создать временный файл"))?;
    let replacement = (|| {
        use std::io::Write;
        file.write_all(contents).map_err(|error| err(&format!("не удалось записать файл: {error}")))?;
        fs::set_permissions(&temporary, metadata.permissions()).map_err(|error| err(&format!("не удалось сохранить права файла: {error}")))?;
        file.sync_all().map_err(|error| err(&format!("не удалось завершить запись файла: {error}")))?;
        drop(file);
        let (latest, latest_contents) = read_text_file_snapshot(&target)?;
        if !same_file_snapshot(&metadata, &latest) || !same_file_permissions(&metadata, &latest) {
            return Err(err("файл заменён или его права изменились во время сохранения"));
        }
        if latest.permissions().readonly() {
            return Err(err("файл доступен только для чтения"));
        }
        if latest_contents != expected {
            return Err(err("файл изменился вне лаунчера. Закройте его и откройте заново, чтобы не затереть чужие изменения."));
        }
        atomic_replace(&temporary, &target).map_err(|error| err(&format!("не удалось заменить файл: {error}")))?;
        Ok(())
    })();
    if replacement.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    replacement?;
    Ok(target)
}

#[cfg(not(windows))]
fn atomic_replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn atomic_replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) };
    if result == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

pub fn fs_save_text_file(request: &Value) -> Result<Value, String> {
    let raw_path = request.get("path").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).ok_or_else(|| err("не указан путь файла"))?;
    let expected = request.get("expected_data").and_then(Value::as_str).unwrap_or_default();
    let contents = request.get("data").and_then(Value::as_str).unwrap_or_default();
    let expected = base64::engine::general_purpose::STANDARD.decode(expected).map_err(|_| err("исходное содержимое повреждено (base64)"))?;
    let contents = base64::engine::general_purpose::STANDARD.decode(contents).map_err(|_| err("новое содержимое повреждено (base64)"))?;
    let target = save_text_file_in_roots(raw_path, &expected, &contents, &allowed_roots())?;
    Ok(json!({ "written": true, "path": target.to_string_lossy(), "size": contents.len() }))
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

    fn sandbox_roots(sandbox: &Sandbox) -> Vec<(String, PathBuf)> {
        vec![("test".to_owned(), sandbox.0.canonicalize().unwrap())]
    }

    #[test]
    fn basic_file_operations_create_rename_and_delete_entries() {
        let sandbox = Sandbox::new("basic-ops");
        let roots = sandbox_roots(&sandbox);
        let directory = sandbox.0.to_string_lossy();
        let file = create_file_in_roots(&directory, "notes.txt", &roots).unwrap();
        assert!(file.is_file());
        let folder = create_directory_in_roots(&directory, "configs", &roots).unwrap();
        assert!(folder.is_dir());

        let renamed = rename_entry_in_roots(file.to_str().unwrap(), "readme.txt", &roots).unwrap();
        assert_eq!(renamed.file_name().unwrap(), "readme.txt");
        assert!(!file.exists());
        assert!(delete_entry_in_roots(folder.to_str().unwrap(), &roots).unwrap());
        assert!(!folder.exists());
        assert!(!delete_entry_in_roots(renamed.to_str().unwrap(), &roots).unwrap());
        assert!(!renamed.exists());
    }

    #[test]
    fn basic_file_operations_reject_path_traversal_and_existing_names() {
        let sandbox = Sandbox::new("basic-reject");
        let roots = sandbox_roots(&sandbox);
        let directory = sandbox.0.to_string_lossy();
        assert!(create_file_in_roots(&directory, "../escape.txt", &roots).is_err());
        fs::write(sandbox.0.join("taken.txt"), b"safe").unwrap();
        assert!(create_file_in_roots(&directory, "taken.txt", &roots).is_err());
        assert!(rename_entry_in_roots(sandbox.0.join("taken.txt").to_str().unwrap(), "..", &roots).is_err());
        assert_eq!(fs::read(sandbox.0.join("taken.txt")).unwrap(), b"safe");
    }

    #[cfg(unix)]
    #[test]
    fn basic_file_operations_refuse_symlinks() {
        use std::os::unix::fs::symlink;
        let sandbox = Sandbox::new("basic-symlink");
        let outside = std::env::temp_dir().join(format!("nivora-fsbrowser-outside-{}", std::process::id()));
        fs::write(&outside, b"outside").unwrap();
        let link = sandbox.0.join("link.txt");
        symlink(&outside, &link).unwrap();
        let roots = sandbox_roots(&sandbox);

        assert!(rename_entry_in_roots(link.to_str().unwrap(), "renamed.txt", &roots).is_err());
        assert!(delete_entry_in_roots(link.to_str().unwrap(), &roots).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        let _ = fs::remove_file(outside);
    }

    #[test]
    fn text_editor_replaces_file_atomically_when_original_is_unchanged() {
        let sandbox = Sandbox::new("text-save");
        let path = sandbox.0.join("settings.txt");
        fs::write(&path, b"before\r\n").unwrap();
        let roots = sandbox_roots(&sandbox);

        save_text_file_in_roots(path.to_str().unwrap(), b"before\r\n", b"after\r\n", &roots).unwrap();

        assert_eq!(fs::read(path).unwrap(), b"after\r\n");
    }

    #[test]
    fn text_editor_refuses_to_overwrite_external_changes() {
        let sandbox = Sandbox::new("text-conflict");
        let path = sandbox.0.join("settings.txt");
        fs::write(&path, b"changed elsewhere").unwrap();
        let roots = sandbox_roots(&sandbox);

        let error = save_text_file_in_roots(path.to_str().unwrap(), b"original", b"replacement", &roots).unwrap_err();

        assert!(error.contains("изменился вне лаунчера"), "{error}");
        assert_eq!(fs::read(path).unwrap(), b"changed elsewhere");
    }

    #[cfg(unix)]
    #[test]
    fn text_editor_refuses_symlink_targets() {
        use std::os::unix::fs::symlink;
        let sandbox = Sandbox::new("text-symlink");
        let outside = std::env::temp_dir().join(format!("nivora-fsbrowser-outside-{}", std::process::id()));
        fs::write(&outside, b"outside").unwrap();
        let link = sandbox.0.join("link.txt");
        symlink(&outside, &link).unwrap();
        let roots = sandbox_roots(&sandbox);
        assert!(read_text_file_snapshot(&link).is_err());

        let error = save_text_file_in_roots(link.to_str().unwrap(), b"outside", b"replacement", &roots).unwrap_err();

        assert!(error.contains("символьные ссылки"), "{error}");
        assert_eq!(fs::read(outside).unwrap(), b"outside");
        let _ = fs::remove_file(std::env::temp_dir().join(format!("nivora-fsbrowser-outside-{}", std::process::id())));
    }
}
