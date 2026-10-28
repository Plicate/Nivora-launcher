// защищённый доступ к файлам: O_NOFOLLOW на Unix и safe_path на Windows
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(any(windows, test))]
use std::fs;
#[cfg(windows)]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::config::ConfigStore;
use super::instances::{InstanceInfo, InstanceStore};
use super::platform_support::process_exists;
#[cfg(windows)]
use super::platform_support::is_reparse_point;
use super::process_guard::InstanceProcessLock;
#[cfg(windows)]
use super::portable_fs::{atomic_write, remove_tree, safe_path};

pub(crate) const MAX_INSTANCE_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INSTANCE_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
const MAX_INSTANCE_UPLOAD_BYTES: usize = 32 * 1024 * 1024;
const MIN_INSTANCE_LOG_READ_BYTES: i64 = 4;
pub(crate) const MAX_INSTANCE_LOG_READ_BYTES: i64 = 131_072;
const MAX_INSTANCE_LIVE_LOG_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const MAX_INSTANCE_LOG_LIST_LIMIT: i64 = 100;
const MAX_LIST_ENTRIES: usize = 1000;
const MAX_DELETE_ENTRIES: usize = 100_000;
const MAX_DELETE_DEPTH: usize = 128;

pub(crate) fn instance_log_hash(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect()
}

/// простой идентификатор UTF-8 текста
const IMAGE_MIMES: &[(&[u8], &str)] = &[
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    (b"\xff\xd8\xff", "image/jpeg"),
];

pub(crate) fn preview_image_mime(content: &[u8]) -> Option<&'static str> {
    for (signature, mime_type) in IMAGE_MIMES {
        if content.starts_with(signature) {
            return Some(mime_type);
        }
    }
    if content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

pub(crate) fn reserved_name(name: &str) -> bool {
    let lowered = name.to_lowercase();
    matches!(
        lowered.as_str(),
        ".trial-cell" | ".launch.lock" | ".xodus-keyring.ron" | "msa" | "profile.json" | "xodus-home"
    ) || lowered.starts_with(".trial-launcher")
}


#[cfg(windows)]
fn as_parts(parts: &[String]) -> Vec<&str> {
    parts.iter().map(String::as_str).collect()
}

#[cfg(windows)]
fn modified_seconds_meta(metadata: &fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn path_parts(raw: &str, allow_root: bool) -> Result<Vec<String>, String> {
    if raw.is_empty() && allow_root {
        return Ok(Vec::new());
    }
    if raw.is_empty()
        || raw.len() > 4096
        || raw.starts_with('/')
        || raw.contains('\\')
        || raw.contains('\0')
        || raw.chars().next().is_some_and(|first| first.is_ascii_alphabetic()) && raw.chars().nth(1) == Some(':')
        || raw.chars().any(|character| (character as u32) < 32)
    {
        return Err("Путь выходит за пределы экземпляра".to_owned());
    }
    let parts: Vec<String> = raw.split('/').map(str::to_owned).collect();
    if parts.iter().any(|part| {
        part.is_empty() || part == "." || part == ".." || part.chars().count() > 255 || reserved_name(part)
    }) {
        return Err("Путь содержит недопустимый компонент".to_owned());
    }
    Ok(parts)
}

#[cfg(unix)]
mod fd {
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub(super) const DIRECTORY_FLAGS: i32 =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    pub(super) const NOFOLLOW_FLAGS: i32 = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;

    pub(super) fn c_string(name: &str) -> io::Result<std::ffi::CString> {
        std::ffi::CString::new(name.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "имя с нулём"))
    }

    pub(super) fn open_directory(path: &Path) -> io::Result<i32> {
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "путь с нулём"))?;
        let fd = unsafe { libc::open(c_path.as_ptr(), DIRECTORY_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    pub(super) fn openat_directory(parent: i32, part: &str) -> io::Result<i32> {
        let name = c_string(part)?;
        let fd = unsafe { libc::openat(parent, name.as_ptr(), DIRECTORY_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    /// цепочка открытых каталогов: каждый шаг атомарно проверяет "не ссылка"
    pub(super) fn open_directory_chain(root: i32, parts: &[String]) -> io::Result<i32> {
        let mut current = unsafe { libc::dup(root) };
        if current < 0 {
            return Err(io::Error::last_os_error());
        }
        for part in parts {
            let next = openat_directory(current, part);
            unsafe { libc::close(current) };
            current = next?;
        }
        Ok(current)
    }

    pub(super) fn stat_at(parent: i32, name: &str) -> io::Result<libc::stat> {
        let cname = c_string(name)?;
        let mut value: libc::stat = unsafe { std::mem::zeroed() };
        let status = unsafe { libc::fstatat(parent, cname.as_ptr(), &mut value, libc::AT_SYMLINK_NOFOLLOW) };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }

    pub(super) fn is_regular(value: &libc::stat) -> bool {
        value.st_mode & libc::S_IFMT == libc::S_IFREG
    }

    pub(super) fn fstat(fd: i32) -> io::Result<libc::stat> {
        let mut value: libc::stat = unsafe { std::mem::zeroed() };
        let status = unsafe { libc::fstat(fd, &mut value) };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }

    pub(super) fn is_directory(value: &libc::stat) -> bool {
        value.st_mode & libc::S_IFMT == libc::S_IFDIR
    }

    pub(super) fn modified_seconds(value: &libc::stat) -> f64 {
        value.st_mtime as f64 + value.st_mtime_nsec as f64 / 1e9
    }

    /// список записей каталога с lstat-данными каждого элемента
    pub(super) fn scan_directory(fd: i32) -> io::Result<Vec<(String, libc::stat)>> {
        let duplicate = unsafe { libc::dup(fd) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let directory = unsafe { libc::fdopendir(duplicate) };
        if directory.is_null() {
            unsafe { libc::close(duplicate) };
            return Err(io::Error::last_os_error());
        }
        let mut entries = Vec::new();
        unsafe {
            loop {
                // каноничная идиома readdir: errno=0 до вызова, проверка после NULL
                *libc::__errno_location() = 0;
                let entry = libc::readdir(directory);
                if entry.is_null() {
                    if *libc::__errno_location() != 0 {
                        libc::closedir(directory);
                        return Err(io::Error::last_os_error());
                    }
                    break;
                }
                let name = std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()).to_string_lossy().into_owned();
                if name == "." || name == ".." {
                    continue;
                }
                if let Ok(value) = stat_at(fd, &name) {
                    entries.push((name, value));
                }
            }
            libc::closedir(directory);
        }
        Ok(entries)
    }

    pub(super) fn read_at(fd: i32, limit: usize, offset: u64) -> io::Result<Vec<u8>> {
        let mut buffer = vec![0u8; limit];
        let mut read_total = 0usize;
        while read_total < limit {
            let count = unsafe {
                libc::pread(
                    fd,
                    buffer[read_total..].as_mut_ptr().cast(),
                    limit - read_total,
                    (offset + read_total as u64) as i64,
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                break;
            }
            read_total += count as usize;
        }
        buffer.truncate(read_total);
        Ok(buffer)
    }

    pub(super) fn open_file_nofollow(parent: i32, name: &str) -> io::Result<i32> {
        let cname = c_string(name)?;
        let fd = unsafe { libc::openat(parent, cname.as_ptr(), NOFOLLOW_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    /// временный файл: только создание с нуля, без следования ссылкам
    pub(super) fn openat_exclusive(parent: i32, name: &str) -> io::Result<i32> {
        let cname = c_string(name)?;
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let fd = unsafe { libc::openat(parent, cname.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    pub(super) fn write_fd(fd: i32, content: &[u8]) -> io::Result<()> {
        let mut written = 0usize;
        while written < content.len() {
            let count = unsafe { libc::write(fd, content[written..].as_ptr().cast(), content.len() - written) };
            if count <= 0 {
                return Err(io::Error::last_os_error());
            }
            written += count as usize;
        }
        Ok(())
    }

    pub(super) fn fsync_fd(fd: i32) -> io::Result<()> {
        if unsafe { libc::fsync(fd) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn rename_at(parent: i32, from: &str, to: &str) -> io::Result<()> {
        let from_name = c_string(from)?;
        let to_name = c_string(to)?;
        if unsafe { libc::renameat(parent, from_name.as_ptr(), parent, to_name.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn unlink_at(parent: i32, name: &str) -> io::Result<()> {
        let cname = c_string(name)?;
        if unsafe { libc::unlinkat(parent, cname.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn mkdir_at(parent: i32, name: &str) -> io::Result<()> {
        let cname = c_string(name)?;
        if unsafe { libc::mkdirat(parent, cname.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn rmdir_at(parent: i32, name: &str) -> io::Result<()> {
        let cname = c_string(name)?;
        if unsafe { libc::unlinkat(parent, cname.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn close_fd(fd: i32) {
        unsafe { libc::close(fd) };
    }
}

pub(crate) type BedrockProfileResolver =
    Arc<dyn Fn(&InstanceInfo) -> Result<PathBuf, String> + Send + Sync>;

pub(crate) struct InstanceFiles {
    data_home: PathBuf,
    logs_dir: PathBuf,
    instances: InstanceStore,
    bedrock_profiles: Option<BedrockProfileResolver>,
}

impl InstanceFiles {
    pub(crate) fn new(
        config: &ConfigStore,
        bedrock_profiles: Option<BedrockProfileResolver>,
    ) -> Result<Self, String> {
        Ok(Self {
            data_home: config.paths.data_home.clone(),
            logs_dir: config.paths.logs_dir.clone(),
            instances: InstanceStore::new(config)?,
            bedrock_profiles,
        })
    }

    fn client_instance(&self, name: &str) -> Result<InstanceInfo, String> {
        let info = self.instances.get(name)?;
        // логи/архивы доступны и Bedrock-экземплярам: сессии пишутся тем же
        if info.name != name || info.kind != "client" || !matches!(info.game_edition.as_str(), "java" | "bedrock") {
            return Err("Файловый доступ доступен только для игровых экземпляров".to_owned());
        }
        Ok(info)
    }

    fn file_instance(&self, name: &str) -> Result<InstanceInfo, String> {
        let info = self.instances.get(name)?;
        if info.name != name || info.kind != "client" {
            return Err("Файловый доступ доступен только для игровых экземпляров".to_owned());
        }
        if info.game_edition == "bedrock" {
            let Some(resolver) = &self.bedrock_profiles else {
                return Err("Профиль Bedrock недоступен".to_owned());
            };
            resolver(&info)?;
            return Ok(info);
        }
        if info.game_edition != "java" {
            return Err("Неизвестное издание игрового экземпляра".to_owned());
        }
        Ok(info)
    }

    fn root_path(&self, info: &InstanceInfo) -> Result<PathBuf, String> {
        if info.game_edition == "bedrock" {
            let Some(resolver) = &self.bedrock_profiles else {
                return Err("Профиль Bedrock недоступен".to_owned());
            };
            return resolver(info);
        }
        Ok(info.path.clone())
    }

    pub(crate) fn list_files(&self, name: &str, directory: &str) -> Result<Vec<Value>, String> {
        let info = self.file_instance(name)?;
        let root = self.root_path(&info)?;
        let parts = path_parts(directory, true)?;
        let mut values: Vec<Value> = Vec::new();
        #[cfg(windows)]
        {
            let target = safe_path(&root, &as_parts(&parts))?;
            if !target.is_dir() {
                return Err("Папка экземпляра не найдена".to_owned());
            }
            for entry in fs::read_dir(&target).map_err(|_| "Не удалось прочитать папку экземпляра".to_owned())?.flatten() {
                let entry_name = entry.file_name().to_string_lossy().into_owned();
                if reserved_name(&entry_name) || is_reparse_point(&entry.path()) {
                    continue;
                }
                let Ok(metadata) = entry.metadata() else { continue };
                let is_file = metadata.is_file();
                let is_directory = metadata.is_dir();
                if !is_file && !is_directory {
                    continue;
                }
                let relative = joined_path(&parts, &entry_name);
                values.push(json!({
                    "name": entry_name,
                    "path": relative,
                    "kind": if is_file { "file" } else { "directory" },
                    "size": if is_file { metadata.len() as i64 } else { 0 },
                    "modified_at": modified_seconds_meta(&metadata),
                    "editable": is_file && metadata.len() <= MAX_INSTANCE_TEXT_BYTES,
                }));
            }
        }
        #[cfg(unix)]
        {
            let root_fd = fd::open_directory(&root).map_err(|_| "Не удалось прочитать папку экземпляра".to_owned())?;
            let outcome = (|| -> Result<(), String> {
                let directory_fd = fd::open_directory_chain(root_fd, &parts)
                    .map_err(|_| "Не удалось прочитать папку экземпляра".to_owned())?;
                let entries = fd::scan_directory(directory_fd)
                    .map_err(|_| "Не удалось прочитать папку экземпляра".to_owned())?;
                unsafe { libc::close(directory_fd) };
                for (entry_name, stat) in entries {
                    if reserved_name(&entry_name) {
                        continue;
                    }
                    let is_file = fd::is_regular(&stat);
                    let is_directory = fd::is_directory(&stat);
                    if !is_file && !is_directory {
                        continue;
                    }
                    let relative = joined_path(&parts, &entry_name);
                    values.push(json!({
                        "name": entry_name,
                        "path": relative,
                        "kind": if is_file { "file" } else { "directory" },
                        "size": if is_file { stat.st_size } else { 0 },
                        "modified_at": fd::modified_seconds(&stat),
                        "editable": is_file && stat.st_size >= 0 && stat.st_size as u64 <= MAX_INSTANCE_TEXT_BYTES,
                    }));
                }
                Ok(())
            })();
            unsafe { libc::close(root_fd) };
            outcome?;
        }
        values.sort_by(|left, right| {
            // сначала каталоги, затем имена без учёта регистра
            let left_dir = left["kind"].as_str() != Some("directory");
            let right_dir = right["kind"].as_str() != Some("directory");
            left_dir
                .cmp(&right_dir)
                .then_with(|| sort_key(left).cmp(&sort_key(right)))
        });
        values.truncate(MAX_LIST_ENTRIES);
        Ok(values)
    }

    pub(crate) fn read_file(&self, name: &str, path: &str, encoding: &str) -> Result<Value, String> {
        if encoding != "text" && encoding != "base64" {
            return Err("Кодировка должна быть text или base64".to_owned());
        }
        let info = self.file_instance(name)?;
        let root = self.root_path(&info)?;
        let parts = path_parts(path, false)?;
        let limit = if encoding == "text" { MAX_INSTANCE_TEXT_BYTES } else { MAX_INSTANCE_PREVIEW_BYTES };
        #[cfg(windows)]
        {
            let target = safe_path(&root, &as_parts(&parts))?;
            let metadata = super::portable_fs::regular_file(&target, Some(limit))
                .map_err(|_| "Файл не найден или превышает допустимый размер".to_owned())?;
            let content = fs::read(&target).map_err(|_| "Не удалось безопасно прочитать файл экземпляра".to_owned())?;
            if content.len() as u64 > limit || metadata.len() > limit {
                return Err("Файл не найден или превышает допустимый размер".to_owned());
            }
            finish_read(content, encoding)
        }
        #[cfg(unix)]
        {
            let root_fd = fd::open_directory(&root).map_err(|_| "Не удалось безопасно прочитать файл экземпляра".to_owned())?;
            let outcome = (|| -> Result<Vec<u8>, String> {
                if parts.is_empty() {
                    return Err("Укажите файл или папку".to_owned());
                }
                let parent_fd = fd::open_directory_chain(root_fd, &parts[..parts.len() - 1])
                    .map_err(|_| "Не удалось безопасно прочитать файл экземпляра".to_owned())?;
                let file_name = parts.last().unwrap();
                let content = (|| -> Result<Vec<u8>, String> {
                    let stat = fd::stat_at(parent_fd, file_name)
                        .map_err(|_| "Файл не найден или превышает допустимый размер".to_owned())?;
                    if !fd::is_regular(&stat) || stat.st_size as u64 > limit {
                        return Err("Файл не найден или превышает допустимый размер".to_owned());
                    }
                    let file_fd = fd::open_file_nofollow(parent_fd, file_name)
                        .map_err(|_| "Не удалось безопасно прочитать файл экземпляра".to_owned())?;
                    let result = (|| -> Result<Vec<u8>, String> {
                        let live_stat = fd::fstat(file_fd)
                            .map_err(|_| "Не удалось безопасно прочитать файл экземпляра".to_owned())?;
                        if !fd::is_regular(&live_stat) || live_stat.st_size as u64 > limit {
                            return Err("Файл не найден или превышает допустимый размер".to_owned());
                        }
                        let content = fd::read_at(file_fd, limit as usize + 1, 0)
                            .map_err(|_| "Не удалось безопасно прочитать файл экземпляра".to_owned())?;
                        if content.len() as u64 > limit {
                            return Err("Файл не найден или превышает допустимый размер".to_owned());
                        }
                        Ok(content)
                    })();
                    unsafe { libc::close(file_fd) };
                    result
                })();
                unsafe { libc::close(parent_fd) };
                content
            })();
            unsafe { libc::close(root_fd) };
            finish_read(outcome?, encoding)
        }
    }
}

fn finish_read(content: Vec<u8>, encoding: &str) -> Result<Value, String> {
    if encoding == "text" {
        let text = String::from_utf8(content).map_err(|_| "Файл не является UTF-8 текстом".to_owned())?;
        if text.contains('\0') {
            return Err("Бинарный файл нельзя открыть как текст".to_owned());
        }
        return Ok(json!({"content": text, "encoding": "text"}));
    }
    use base64::Engine as _;
    let Some(mime_type) = preview_image_mime(&content) else {
        return Err("Предпросмотр доступен только для поддерживаемых изображений".to_owned());
    };
    Ok(json!({
        "content": base64::engine::general_purpose::STANDARD.encode(content),
        "encoding": "base64",
        "mime_type": mime_type,
    }))
}

/// сравнивает имена без учёта регистра
fn sort_key(value: &Value) -> String {
    value["name"].as_str().unwrap_or("").to_casefold()
}

trait CaseFold {
    fn to_casefold(&self) -> String;
}

impl CaseFold for str {
    fn to_casefold(&self) -> String {
        self.chars().flat_map(|character| character.to_lowercase()).collect()
    }
}

fn joined_path(parts: &[String], name: &str) -> String {
    let mut joined: Vec<&str> = parts.iter().map(String::as_str).collect();
    joined.push(name);
    joined.join("/")
}

#[cfg(unix)]
enum FdFailure {
    Io,
    Launcher(String),
}

#[cfg(unix)]
impl From<std::io::Error> for FdFailure {
    fn from(_: std::io::Error) -> Self {
        FdFailure::Io
    }
}

/// заменяет файл, предварительно проверив, что цель не ссылка
#[cfg(unix)]
fn ensure_regular_destination(parent_fd: i32, file_name: &str) -> Result<(), FdFailure> {
    match fd::stat_at(parent_fd, file_name) {
        Err(ref error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(FdFailure::Io),
        Ok(value) if fd::is_regular(&value) => Ok(()),
        Ok(_) => Err(FdFailure::Launcher("Файл является ссылкой или специальным объектом".to_owned())),
    }
}

#[cfg(unix)]
fn atomic_write_at(parent_fd: i32, file_name: &str, content: &[u8]) -> Result<(), FdFailure> {
    ensure_regular_destination(parent_fd, file_name)?;
    let temporary = format!(".trial-launcher-workspace-{}.part", super::mods::random_hex(16));
    let fd = fd::openat_exclusive(parent_fd, &temporary)?;
    let outcome = (|| -> Result<(), FdFailure> {
        fd::write_fd(fd, content)?;
        fd::fsync_fd(fd)?;
        ensure_regular_destination(parent_fd, file_name)?;
        fd::rename_at(parent_fd, &temporary, file_name)?;
        // ошибка синхронизации каталога не отменяет успешную запись файла
        let _ = fd::fsync_fd(parent_fd);
        Ok(())
    })();
    fd::close_fd(fd);
    let cleanup = fd::unlink_at(parent_fd, &temporary);
    if let Err(error) = cleanup {
        if error.kind() != std::io::ErrorKind::NotFound {
            let _ = outcome?;
            return Err(FdFailure::Io);
        }
    }
    outcome
}

#[cfg(unix)]
fn preflight_delete_at(parent_fd: i32, name: &str, count: &mut usize, depth: usize) -> Result<(), FdFailure> {
    if depth > MAX_DELETE_DEPTH {
        return Err(FdFailure::Launcher("Папка содержит слишком много уровней вложенности".to_owned()));
    }
    let value = fd::stat_at(parent_fd, name)?;
    if fd::is_regular(&value) {
        return Ok(());
    }
    if !fd::is_directory(&value) {
        return Err(FdFailure::Launcher("Можно удалить только файл или папку".to_owned()));
    }
    let directory_fd = fd::openat_directory(parent_fd, name)?;
    let outcome = (|| -> Result<(), FdFailure> {
        for (entry, _stat) in fd::scan_directory(directory_fd)? {
            if reserved_name(&entry) {
                return Err(FdFailure::Launcher("Папка содержит служебные данные лаунчера".to_owned()));
            }
            *count += 1;
            if *count > MAX_DELETE_ENTRIES {
                return Err(FdFailure::Launcher("Папка содержит слишком много файлов для удаления".to_owned()));
            }
            preflight_delete_at(directory_fd, &entry, count, depth + 1)?;
        }
        Ok(())
    })();
    fd::close_fd(directory_fd);
    outcome
}

#[cfg(unix)]
fn remove_entry_at(parent_fd: i32, name: &str, depth: usize) -> Result<(), FdFailure> {
    if depth > MAX_DELETE_DEPTH {
        return Err(FdFailure::Launcher("Папка содержит слишком много уровней вложенности".to_owned()));
    }
    let value = fd::stat_at(parent_fd, name)?;
    if fd::is_regular(&value) {
        fd::unlink_at(parent_fd, name)?;
        return Ok(());
    }
    if !fd::is_directory(&value) {
        return Err(FdFailure::Launcher("Можно удалить только файл или папку".to_owned()));
    }
    let directory_fd = fd::openat_directory(parent_fd, name)?;
    let outcome = (|| -> Result<(), FdFailure> {
        for (entry, _stat) in fd::scan_directory(directory_fd)? {
            if reserved_name(&entry) {
                return Err(FdFailure::Launcher("Папка содержит служебные данные лаунчера".to_owned()));
            }
            remove_entry_at(directory_fd, &entry, depth + 1)?;
        }
        Ok(())
    })();
    fd::close_fd(directory_fd);
    outcome?;
    fd::rmdir_at(parent_fd, name)?;
    Ok(())
}

#[cfg(unix)]
fn map_fd_failure(failure: FdFailure, message: &str) -> String {
    match failure {
        FdFailure::Io => message.to_owned(),
        FdFailure::Launcher(text) => text,
    }
}

impl InstanceFiles {
    /// блокировка экземпляра + проверка "не запущен" перед любыми изменениями
    fn with_mutation<R>(
        &self,
        name: &str,
        action: impl FnOnce(&InstanceInfo) -> Result<R, String>,
    ) -> Result<R, String> {
        let info = self.file_instance(name)?;
        let mut lock = InstanceProcessLock::new(&self.data_home, &info.name);
        lock.acquire()?;
        let outcome = (|| -> Result<R, String> {
            let refreshed = self.file_instance(&info.name)?;
            if instance_process_running(&refreshed.path)? {
                return Err("Нельзя изменять файлы запущенного экземпляра".to_owned());
            }
            action(&refreshed)
        })();
        lock.release();
        outcome
    }

    pub(crate) fn write_file(&self, name: &str, path: &str, content: &str) -> Result<(), String> {
        let parts = path_parts(path, false)?;
        let encoded = content.as_bytes();
        if encoded.len() as u64 > MAX_INSTANCE_TEXT_BYTES {
            return Err("Текстовый файл слишком большой (максимум 2 MiB)".to_owned());
        }
        self.with_mutation(name, move |info| {
            let root = self.root_path(info)?;
            #[cfg(windows)]
            {
                atomic_write(&root, &as_parts(&parts), encoded)
                    .map(|_| ())
                    .map_err(|_| "Не удалось сохранить файл экземпляра".to_owned())
            }
            #[cfg(unix)]
            {
                let root_fd =
                    fd::open_directory(&root).map_err(|_| "Не удалось сохранить файл экземпляра".to_owned())?;
                let outcome = open_parent_chain(root_fd, &parts).and_then(|(parent_fd, file_name)| {
                    let result = atomic_write_at(parent_fd, &file_name, encoded);
                    fd::close_fd(parent_fd);
                    result
                });
                fd::close_fd(root_fd);
                outcome.map_err(|failure| map_fd_failure(failure, "Не удалось сохранить файл экземпляра"))
            }
        })
    }

    pub(crate) fn mkdir(&self, name: &str, path: &str) -> Result<(), String> {
        let parts = path_parts(path, false)?;
        self.with_mutation(name, move |info| {
            let root = self.root_path(info)?;
            #[cfg(windows)]
            {
                let target = safe_path(&root, &as_parts(&parts))?;
                fs::create_dir(&target).map_err(|_| "Не удалось создать папку экземпляра".to_owned())
            }
            #[cfg(unix)]
            {
                let root_fd =
                    fd::open_directory(&root).map_err(|_| "Не удалось создать папку экземпляра".to_owned())?;
                let outcome = open_parent_chain(root_fd, &parts).and_then(|(parent_fd, directory_name)| {
                    let result = fd::mkdir_at(parent_fd, &directory_name).map_err(FdFailure::from);
                    fd::close_fd(parent_fd);
                    result
                });
                fd::close_fd(root_fd);
                outcome.map_err(|failure| map_fd_failure(failure, "Не удалось создать папку экземпляра"))
            }
        })
    }

    pub(crate) fn upload(&self, name: &str, path: &str, content: &[u8]) -> Result<(), String> {
        let parts = path_parts(path, false)?;
        if content.len() > MAX_INSTANCE_UPLOAD_BYTES {
            return Err("Файл слишком большой (максимум 32 MiB)".to_owned());
        }
        self.with_mutation(name, move |info| {
            let root = self.root_path(info)?;
            #[cfg(windows)]
            {
                atomic_write(&root, &as_parts(&parts), content)
                    .map(|_| ())
                    .map_err(|_| "Не удалось загрузить файл в экземпляр".to_owned())
            }
            #[cfg(unix)]
            {
                let root_fd =
                    fd::open_directory(&root).map_err(|_| "Не удалось загрузить файл в экземпляр".to_owned())?;
                let outcome = open_parent_chain(root_fd, &parts).and_then(|(parent_fd, file_name)| {
                    let result = atomic_write_at(parent_fd, &file_name, content);
                    fd::close_fd(parent_fd);
                    result
                });
                fd::close_fd(root_fd);
                outcome.map_err(|failure| map_fd_failure(failure, "Не удалось загрузить файл в экземпляр"))
            }
        })
    }

    pub(crate) fn delete(&self, name: &str, path: &str) -> Result<(), String> {
        let parts = path_parts(path, false)?;
        self.with_mutation(name, move |info| {
            let root = self.root_path(info)?;
            #[cfg(windows)]
            {
                let reserved = |name: &str| reserved_name(name);
                remove_tree(&root, &as_parts(&parts), Some(&reserved), MAX_DELETE_ENTRIES, MAX_DELETE_DEPTH)
            }
            #[cfg(unix)]
            {
                let root_fd =
                    fd::open_directory(&root).map_err(|_| "Не удалось удалить путь экземпляра".to_owned())?;
                let outcome = open_parent_chain(root_fd, &parts).and_then(|(parent_fd, entry_name)| {
                    let mut count = 0usize;
                    let result = preflight_delete_at(parent_fd, &entry_name, &mut count, 0)
                        .and_then(|()| remove_entry_at(parent_fd, &entry_name, 0));
                    fd::close_fd(parent_fd);
                    result
                });
                fd::close_fd(root_fd);
                outcome.map_err(|failure| map_fd_failure(failure, "Не удалось удалить путь экземпляра"))
            }
        })
    }

    pub(crate) fn list_log_archives(&self, name: &str, offset: &Value, limit: &Value) -> Result<Value, String> {
        let info = self.client_instance(name)?;
        let (offset, limit) = pagination(offset, limit, MAX_INSTANCE_LOG_LIST_LIMIT)?;
        let instances = self.instances.list();
        let mut values: Vec<(f64, String, i64)> = Vec::new();
        #[cfg(windows)]
        {
            let logs_dir = self.logs_dir.clone();
            if logs_dir.is_dir() && !is_reparse_point(&logs_dir) {
                for entry in fs::read_dir(&logs_dir).into_iter().flatten().flatten() {
                    let id = entry.file_name().to_string_lossy().into_owned();
                    if !log_id_matches(&id) || is_reparse_point(&entry.path()) {
                        continue;
                    }
                    let Ok(metadata) = entry.metadata() else { continue };
                    if metadata.is_file() && log_belongs_to_instance(&info, &id, &instances) {
                        values.push((modified_seconds_meta(&metadata), id, metadata.len() as i64));
                    }
                }
            }
        }
        #[cfg(unix)]
        {
            match fd::open_directory(&self.logs_dir) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("Не удалось прочитать архивы логов экземпляра".to_owned()),
                Ok(logs_fd) => {
                    let outcome = (|| -> Result<(), String> {
                        let entries = fd::scan_directory(logs_fd)
                            .map_err(|_| "Не удалось прочитать архивы логов экземпляра".to_owned())?;
                        for (id, stat) in entries {
                            if !log_id_matches(&id) || !fd::is_regular(&stat) {
                                continue;
                            }
                            if log_belongs_to_instance(&info, &id, &instances) {
                                values.push((fd::modified_seconds(&stat), id, stat.st_size));
                            }
                        }
                        Ok(())
                    })();
                    fd::close_fd(logs_fd);
                    outcome?;
                }
            }
        }
        values.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let total = values.len();
        let page: Vec<Value> = values
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(modified_at, id, size)| json!({"id": id, "modified_at": modified_at, "size": size}))
            .collect();
        let consumed = offset + page.len();
        let next_offset = if consumed < total { Some(consumed) } else { None };
        Ok(json!({"logs": page, "next_offset": next_offset}))
    }

    pub(crate) fn read_log_archive(
        &self,
        name: &str,
        log_id: &str,
        offset: &Value,
        limit: &Value,
    ) -> Result<Value, String> {
        let info = self.client_instance(name)?;
        if !log_id_matches(log_id) {
            return Err("Некорректный идентификатор архива лога".to_owned());
        }
        let (offset, limit) = log_read_range(offset, limit)?;
        let instances = self.instances.list();
        if !log_belongs_to_instance(&info, log_id, &instances) {
            return Err("Архив лога не относится к этому экземпляру".to_owned());
        }
        #[cfg(windows)]
        {
            use std::io::Seek as _;
            let target = safe_path(&self.logs_dir, &[log_id])?;
            super::portable_fs::regular_file(&target, None).map_err(|_| "Архив лога недоступен".to_owned())?;
            let file_size = fs::metadata(&target).map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?.len() as i64;
            let offset = offset.min(file_size);
            let mut handle = fs::File::open(&target).map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?;
            let mut content = Vec::new();
            handle
                .seek(std::io::SeekFrom::Start(offset as u64))
                .and_then(|_| (&mut handle).take(limit as u64).read_to_end(&mut content))
                .map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?;
            let (text, next_offset, eof) = decode_log_chunk(&content, offset as usize, file_size);
            Ok(json!({"content": text, "next_offset": next_offset, "eof": eof}))
        }
        #[cfg(unix)]
        {
            let logs_fd = fd::open_directory(&self.logs_dir)
                .map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?;
            let outcome = (|| -> Result<Value, String> {
                let target_stat = fd::stat_at(logs_fd, log_id).map_err(|_| "Архив лога недоступен".to_owned())?;
                if !fd::is_regular(&target_stat) {
                    return Err("Архив лога недоступен".to_owned());
                }
                let file_fd = fd::open_file_nofollow(logs_fd, log_id)
                    .map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?;
                let result = (|| -> Result<Value, String> {
                    let live_stat = fd::fstat(file_fd).map_err(|_| "Архив лога недоступен".to_owned())?;
                    if !fd::is_regular(&live_stat) {
                        return Err("Архив лога недоступен".to_owned());
                    }
                    let file_size = live_stat.st_size;
                    let offset = offset.min(file_size);
                    let content = fd::read_at(file_fd, limit as usize, offset as u64)
                        .map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?;
                    // размер берётся повторно: лог может расти во время чтения
                    let file_size = fd::fstat(file_fd)
                        .map_err(|_| "Не удалось безопасно прочитать архив лога".to_owned())?
                        .st_size;
                    let (text, next_offset, eof) = decode_log_chunk(&content, offset as usize, file_size);
                    Ok(json!({"content": text, "next_offset": next_offset, "eof": eof}))
                })();
                fd::close_fd(file_fd);
                result
            })();
            fd::close_fd(logs_fd);
            outcome
        }
    }

    pub(crate) fn read_log_tail(&self, name: &str, log_id: &str, line_limit: i64) -> Result<Vec<String>, String> {
        let info = self.client_instance(name)?;
        if !log_id_matches(log_id) {
            return Err("Некорректный идентификатор архива лога".to_owned());
        }
        if !(1..=1000).contains(&line_limit) {
            return Err("Размер хвоста лога должен быть от 1 до 1000 строк".to_owned());
        }
        if !log_belongs_to_instance(&info, log_id, &self.instances.list()) {
            return Err("Архив лога не относится к этому экземпляру".to_owned());
        }
        #[cfg(windows)]
        {
            use std::io::Seek as _;
            let target = safe_path(&self.logs_dir, &[log_id])?;
            super::portable_fs::regular_file(&target, None).map_err(|_| "Архив лога недоступен".to_owned())?;
            let metadata = fs::metadata(&target).map_err(|_| "Не удалось безопасно прочитать текущий лог".to_owned())?;
            let file_size = metadata.len() as i64;
            let offset = (file_size - MAX_INSTANCE_LIVE_LOG_BYTES as i64).max(0);
            let mut handle = fs::File::open(&target).map_err(|_| "Не удалось безопасно прочитать текущий лог".to_owned())?;
            let mut content = Vec::new();
            handle
                .seek(std::io::SeekFrom::Start(offset as u64))
                .and_then(|_| (&mut handle).take(MAX_INSTANCE_LIVE_LOG_BYTES).read_to_end(&mut content))
                .map_err(|_| "Не удалось безопасно прочитать текущий лог".to_owned())?;
            Ok(tail_lines(&content, offset, line_limit as usize))
        }
        #[cfg(unix)]
        {
            let logs_fd = fd::open_directory(&self.logs_dir)
                .map_err(|_| "Не удалось безопасно прочитать текущий лог".to_owned())?;
            let outcome = (|| -> Result<Vec<String>, String> {
                let target_stat = fd::stat_at(logs_fd, log_id).map_err(|_| "Архив лога недоступен".to_owned())?;
                if !fd::is_regular(&target_stat) {
                    return Err("Архив лога недоступен".to_owned());
                }
                let file_fd = fd::open_file_nofollow(logs_fd, log_id)
                    .map_err(|_| "Не удалось безопасно прочитать текущий лог".to_owned())?;
                let result = (|| -> Result<Vec<String>, String> {
                    let file_stat = fd::fstat(file_fd).map_err(|_| "Архив лога недоступен".to_owned())?;
                    if !fd::is_regular(&file_stat) {
                        return Err("Архив лога недоступен".to_owned());
                    }
                    let file_size = file_stat.st_size;
                    let offset = (file_size - MAX_INSTANCE_LIVE_LOG_BYTES as i64).max(0);
                    let content = fd::read_at(file_fd, (file_size - offset) as usize, offset as u64)
                        .map_err(|_| "Не удалось безопасно прочитать текущий лог".to_owned())?;
                    Ok(tail_lines(&content, offset, line_limit as usize))
                })();
                fd::close_fd(file_fd);
                result
            })();
            fd::close_fd(logs_fd);
            outcome
        }
    }
}

/// открывает родителя цепочкой и возвращает (fd, имя последнего компонента)
#[cfg(unix)]
fn open_parent_chain(root_fd: i32, parts: &[String]) -> Result<(i32, String), FdFailure> {
    let Some(last) = parts.last() else {
        return Err(FdFailure::Launcher("Укажите файл или папку".to_owned()));
    };
    let parents: Vec<&str> = parts[..parts.len() - 1].iter().map(String::as_str).collect();
    let parents: Vec<String> = parents.into_iter().map(str::to_owned).collect();
    let parent_fd = fd::open_directory_chain(root_fd, &parents)?;
    Ok((parent_fd, last.clone()))
}

static LOG_ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9._-]{1,240}\.log\z").expect("валидный regex идентификатора лога")
});
static LOG_VERSION_TAIL_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9._-]+-\d{1,20}-[0-9a-f]{10}\.log\z").expect("валидный regex хвоста лога")
});
static LOG_SUFFIX_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^\d{1,20}-[0-9a-f]{10}\.log\z").expect("валидный regex суффикса лога")
});
static KNOWN_VERSION_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9._-]{1,128}\z").expect("валидный regex версии"));

fn log_id_matches(log_id: &str) -> bool {
    LOG_ID_RE.is_match(log_id)
}

pub(crate) fn log_component(value: &str) -> String {
    static NON_COMPONENT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[^A-Za-z0-9._-]+").expect("валидный regex компонента"));
    let normalized: String = NON_COMPONENT.replace_all(value, "_").into_owned();
    let trimmed = normalized.trim_matches(|c: char| matches!(c, '.' | '_' | '-'));
    let limited: String = trimmed.chars().take(80).collect();
    if limited.is_empty() { "launcher".to_owned() } else { limited }
}

fn known_versions(info: &InstanceInfo) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    if !info.version_id.is_empty() {
        values.push(info.version_id.clone());
    }
    #[cfg(windows)]
    {
        if let Ok(versions_dir) = safe_path(&info.path, &["versions"]) {
            if versions_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&versions_dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if entry.path().is_dir() && !is_reparse_point(&entry.path()) && !reserved_name(&name) {
                            values.push(name);
                        }
                    }
                }
            }
        }
    }
    #[cfg(unix)]
    {
        if let Ok(root_fd) = fd::open_directory(&info.path) {
            if let Ok(versions_fd) = fd::open_directory_chain(root_fd, &["versions".to_owned()]) {
                if let Ok(entries) = fd::scan_directory(versions_fd) {
                    for (name, stat) in entries {
                        if fd::is_directory(&stat) && !reserved_name(&name) {
                            values.push(name);
                        }
                    }
                }
                fd::close_fd(versions_fd);
            }
            fd::close_fd(root_fd);
        }
    }
    values.retain(|value| KNOWN_VERSION_RE.is_match(value));
    values.sort();
    values.dedup();
    values
}

pub(crate) fn log_belongs_to_instance(info: &InstanceInfo, log_id: &str, instances: &[InstanceInfo]) -> bool {
    let modern_prefix = format!("{}-{}-", log_component(&info.name), instance_log_hash(&info.name));
    if let Some(rest) = log_id.strip_prefix(&modern_prefix) {
        return LOG_VERSION_TAIL_RE.is_match(rest);
    }
    let mut matches: Vec<String> = Vec::new();
    for candidate in instances {
        if candidate.kind != "client" {
            continue;
        }
        for version in known_versions(candidate) {
            let prefix = format!("{}-{}-", log_component(&candidate.name), log_component(&version));
            if let Some(rest) = log_id.strip_prefix(&prefix) {
                if LOG_SUFFIX_RE.is_match(rest) {
                    matches.push(candidate.name.clone());
                    break;
                }
            }
        }
    }
    matches.len() == 1 && matches[0] == info.name
}

fn pagination(offset: &Value, limit: &Value, maximum: i64) -> Result<(usize, usize), String> {
    let normalized_offset = coerce_int(offset).ok_or_else(|| "Смещение и размер страницы должны быть числами".to_owned())?;
    let normalized_limit = coerce_int(limit).ok_or_else(|| "Смещение и размер страницы должны быть числами".to_owned())?;
    if normalized_offset < 0 || !(1..=maximum).contains(&normalized_limit) {
        return Err(format!(
            "Размер страницы должен быть от 1 до {maximum}, смещение не может быть отрицательным"
        ));
    }
    Ok((normalized_offset as usize, normalized_limit as usize))
}

fn log_read_range(offset: &Value, limit: &Value) -> Result<(i64, i64), String> {
    let normalized_offset = coerce_int(offset).ok_or_else(|| "Смещение и размер чтения должны быть числами".to_owned())?;
    let normalized_limit = coerce_int(limit).ok_or_else(|| "Смещение и размер чтения должны быть числами".to_owned())?;
    if normalized_offset < 0 || !(MIN_INSTANCE_LOG_READ_BYTES..=MAX_INSTANCE_LOG_READ_BYTES).contains(&normalized_limit) {
        return Err(format!(
            "Размер чтения должен быть от {MIN_INSTANCE_LOG_READ_BYTES} до {MAX_INSTANCE_LOG_READ_BYTES}"
        ));
    }
    Ok((normalized_offset, normalized_limit))
}

/// разбирает число, строку или логическое значение, отбрасывая дробную часть
pub(crate) fn coerce_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        Value::Bool(flag) => Some(i64::from(*flag)),
        _ => None,
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|float| float != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn pid_from_value(value: &Value) -> Result<i64, ()> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64))
            .ok_or(()),
        Value::String(text) => text.trim().parse::<i64>().map_err(|_| ()),
        Value::Bool(flag) => Ok(i64::from(*flag)),
        _ => Err(()),
    }
}

/// живой маркер запуска: pid<=0 без процесса считается "работает"
pub(crate) fn instance_process_running(root: &Path) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let state_path = safe_path(root, &[".trial-launcher-live.json"])?;
        let metadata = fs::symlink_metadata(&state_path);
        let readable = metadata.is_ok()
            && !is_reparse_point(&state_path)
            && metadata.as_ref().map(|meta| meta.is_file()).unwrap_or(false)
            && metadata.as_ref().map(|meta| meta.len()).unwrap_or(0) <= 64 * 1024;
        if !readable {
            return Ok(false);
        }
        let raw = fs::read_to_string(&state_path)
            .map_err(|_| "Не удалось проверить состояние запуска экземпляра".to_owned())?;
        let value: Value = serde_json::from_str(&raw)
            .map_err(|_| "Не удалось проверить состояние запуска экземпляра".to_owned())?;
        let Some(fields) = value.as_object() else { return Ok(false) };
        if !fields.get("running").map(truthy).unwrap_or(false) {
            return Ok(false);
        }
        let pid = pid_from_value(fields.get("pid").unwrap_or(&Value::Null))
            .map_err(|_| "Состояние запуска экземпляра повреждено".to_owned())?;
        Ok(pid <= 0 || process_exists(pid as i32))
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let live_path = root.join(".trial-launcher-live.json");
        let bytes = live_path.as_os_str().as_bytes();
        let c_name = std::ffi::CString::new(bytes)
            .map_err(|_| "Не удалось проверить состояние запуска экземпляра".to_owned())?;
        let fd = unsafe { libc::open(c_name.as_ptr(), fd::NOFOLLOW_FLAGS) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.kind() {
                std::io::ErrorKind::NotFound => Ok(false),
                _ => Err("Не удалось проверить состояние запуска экземпляра".to_owned()),
            };
        }
        let outcome = (|| -> Result<bool, String> {
            let value_stat = fd::fstat(fd).map_err(|_| "Не удалось проверить состояние запуска экземпляра".to_owned())?;
            if !fd::is_regular(&value_stat) || value_stat.st_size > 64 * 1024 {
                return Err("Состояние запуска экземпляра повреждено".to_owned());
            }
            let raw = fd::read_at(fd, 64 * 1024 + 1, 0)
                .map_err(|_| "Не удалось проверить состояние запуска экземпляра".to_owned())?;
            let value: Value = serde_json::from_slice(&raw)
                .map_err(|_| "Не удалось проверить состояние запуска экземпляра".to_owned())?;
            let Some(fields) = value.as_object() else { return Ok(false) };
            if !fields.get("running").map(truthy).unwrap_or(false) {
                return Ok(false);
            }
            let pid = pid_from_value(fields.get("pid").unwrap_or(&Value::Null))
                .map_err(|_| "Состояние запуска экземпляра повреждено".to_owned())?;
            if pid <= 0 {
                return Ok(true);
            }
            Ok(process_exists(pid as i32))
        })();
        fd::close_fd(fd);
        outcome
    }
}

/// декодирование куска лога: обрыв мультибайта на границе не теряет строки
pub(crate) fn decode_log_chunk(content: &[u8], offset: usize, file_size: i64) -> (String, usize, bool) {
    let mut next_offset = offset + content.len();
    let mut eof = next_offset as i64 >= file_size;
    let text = match std::str::from_utf8(content) {
        Ok(text) => text.to_owned(),
        Err(error) if error.error_len().is_none() && !eof => {
            let start = error.valid_up_to();
            if start > 0 {
                let trimmed = &content[..start];
                next_offset = offset + trimmed.len();
                eof = next_offset as i64 >= file_size;
                String::from_utf8_lossy(trimmed).into_owned()
            } else {
                next_offset = offset;
                String::new()
            }
        }
        Err(_) => String::from_utf8_lossy(content).into_owned(),
    };
    (text, next_offset, eof)
}

/// после пропуска байтов первая строка хвоста может быть неполной
pub(crate) fn tail_lines(content: &[u8], offset: i64, line_limit: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(content).replace('\r', "");
    let text = if offset > 0 {
        match text.find('\n') {
            Some(position) => text[position + 1..].to_owned(),
            None => String::new(),
        }
    } else {
        text
    };
    let mut lines = python_splitlines(&text);
    if lines.len() > line_limit {
        let split = lines.len() - line_limit;
        lines.drain(..split);
    }
    lines
}

/// делит текст по переводам строк и другим поддерживаемым границам строк
fn python_splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if matches!(
            character,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            lines.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn files_with_config() -> Result<InstanceFiles, String> {
    let config = ConfigStore::load()?;
    // путь данных Bedrock определяется профилем BoL или официальной игры
    let resolver: BedrockProfileResolver = std::sync::Arc::new(|info| super::bedrock::profile_path_for(info));
    InstanceFiles::new(&config, Some(resolver))
}

pub fn instance_file_list(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let directory = request.get("directory").and_then(Value::as_str).unwrap_or_default();
    Ok(json!({ "files": files_with_config()?.list_files(name, directory)? }))
}

pub fn instance_file_read(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let path = request.get("path").and_then(Value::as_str).unwrap_or_default();
    let encoding = request.get("encoding").and_then(Value::as_str).unwrap_or("text");
    files_with_config()?.read_file(name, path, encoding)
}

pub fn instance_file_write(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let path = request.get("path").and_then(Value::as_str).unwrap_or_default();
    let Some(content) = request.get("content").and_then(Value::as_str) else {
        return Err("Текстовый файл должен содержать UTF-8 текст".to_owned());
    };
    files_with_config()?.write_file(name, path, content)?;
    Ok(json!({ "saved": true }))
}

pub fn instance_file_mkdir(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let path = request.get("path").and_then(Value::as_str).unwrap_or_default();
    files_with_config()?.mkdir(name, path)?;
    Ok(json!({ "created": true }))
}

pub fn instance_file_upload(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let path = request.get("path").and_then(Value::as_str).unwrap_or_default();
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or_default();
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| "Файл экземпляра повреждён".to_owned())?;
    if decoded.len() > MAX_INSTANCE_UPLOAD_BYTES {
        return Err("Файл экземпляра слишком большой".to_owned());
    }
    files_with_config()?.upload(name, path, &decoded)?;
    Ok(json!({ "uploaded": true }))
}

pub fn instance_file_delete(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let path = request.get("path").and_then(Value::as_str).unwrap_or_default();
    files_with_config()?.delete(name, path)?;
    Ok(json!({ "removed": true }))
}

pub fn instance_log_archives(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let offset = request.get("offset").cloned().unwrap_or(json!(0));
    let limit = request.get("limit").cloned().unwrap_or(json!(50));
    files_with_config()?.list_log_archives(name, &offset, &limit)
}

pub fn instance_log_archive_read(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let log_id = request.get("log_id").and_then(Value::as_str).unwrap_or_default();
    let offset = request.get("offset").cloned().unwrap_or(json!(0));
    let limit = request.get("limit").cloned().unwrap_or(json!(131072));
    files_with_config()?.read_log_archive(name, log_id, &offset, &limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_parts_validation() {
        assert_eq!(path_parts("", true).unwrap(), Vec::<String>::new());
        assert_eq!(
            path_parts("mods/optifine", false).unwrap(),
            vec!["mods".to_owned(), "optifine".to_owned()]
        );
        assert!(path_parts("себе/файл.txt", false).is_ok(), "юникод допустим");
    }

    #[test]
    fn path_parts_rejects_escapes_and_reserved() {
        assert_eq!(path_parts("/abs", false).unwrap_err(), "Путь выходит за пределы экземпляра");
        assert_eq!(path_parts("a\\b", false).unwrap_err(), "Путь выходит за пределы экземпляра");
        assert_eq!(path_parts("C:/x", false).unwrap_err(), "Путь выходит за пределы экземпляра");
        assert_eq!(path_parts("..", false).unwrap_err(), "Путь содержит недопустимый компонент");
        assert_eq!(path_parts("a//b", false).unwrap_err(), "Путь содержит недопустимый компонент");
        assert_eq!(path_parts(".trial-launcher-live.json", false).unwrap_err(), "Путь содержит недопустимый компонент");
        assert_eq!(path_parts("profile.json", false).unwrap_err(), "Путь содержит недопустимый компонент");
        let long = "x".repeat(4097);
        assert_eq!(path_parts(&long, false).unwrap_err(), "Путь выходит за пределы экземпляра");
    }

    #[test]
    fn reserved_names_cover_launcher_and_auth_artifacts() {
        assert!(reserved_name(".trial-launcher-live.json"));
        assert!(reserved_name(".TRIAL-LAUNCHER-ANY"));
        assert!(reserved_name(".launch.lock"));
        assert!(reserved_name("MSA"));
        assert!(reserved_name("xodus-home"));
        assert!(!reserved_name("mods"));
        assert!(!reserved_name("saves"));
    }

    #[test]
    fn image_mime_signatures() {
        assert_eq!(preview_image_mime(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(preview_image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(preview_image_mime(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        let mut webp = vec![0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(preview_image_mime(&webp), Some("image/webp"));
        assert_eq!(preview_image_mime(b"BM...."), None);
    }

    #[test]
    fn pagination_coerces_and_validates() {
        assert_eq!(pagination(&json!(0), &json!(50), 100).unwrap(), (0, 50));
        assert_eq!(pagination(&json!("10"), &json!(7.0), 100).unwrap(), (10, 7));
        assert!(pagination(&json!(true), &json!(10), 100).is_ok());
        assert_eq!(
            pagination(&json!("abc"), &json!(10), 100).unwrap_err(),
            "Смещение и размер страницы должны быть числами"
        );
        assert_eq!(
            pagination(&json!(0), &json!(0), 100).unwrap_err(),
            "Размер страницы должен быть от 1 до 100, смещение не может быть отрицательным"
        );
        assert_eq!(
            pagination(&json!(-1), &json!(10), 100).unwrap_err(),
            "Размер страницы должен быть от 1 до 100, смещение не может быть отрицательным"
        );
        assert_eq!(
            log_read_range(&json!(0), &json!(2)).unwrap_err(),
            "Размер чтения должен быть от 4 до 131072"
        );
        assert!(log_read_range(&json!(0), &json!(131072)).is_ok());
        assert_eq!(
            log_read_range(&json!(null), &json!(10)).unwrap_err(),
            "Смещение и размер чтения должны быть числами"
        );
    }

    #[test]
    fn log_identifiers_match_expected_shapes() {
        assert!(log_id_matches("My.Server-1700000000-0123456789ab.log"));
        assert!(log_id_matches("a.log"));
        assert!(!log_id_matches("a.log.txt"));
        assert!(!log_id_matches("a/../b.log"));
        assert!(!log_id_matches(""));
        // суффикс версии содержит ровно 10 шестнадцатеричных символов
        assert!(LOG_VERSION_TAIL_RE.is_match("Name-123-0123456789.log"));
        assert!(!LOG_VERSION_TAIL_RE.is_match("Name-123-0123456789ab.log"));
        assert!(!LOG_VERSION_TAIL_RE.is_match("Name-123-0123456789.log.bak"));
        assert!(LOG_SUFFIX_RE.is_match("123-0123456789.log"));
        assert!(!LOG_SUFFIX_RE.is_match("x-123-0123456789.log"));
    }

    #[test]
    fn log_component_normalizes_like_python() {
        // непрерывный запуск запрещённых символов схлопывается в один "_"
        assert_eq!(log_component("Мой сервер"), "launcher");
        assert_eq!(log_component("сервер 1.20"), "1.20");
        assert_eq!(log_component("Alpha..___---"), "Alpha");
        assert_eq!(log_component("!!!"), "launcher");
        assert_eq!(log_component("a!!b"), "a_b");
        let long = "a".repeat(120);
        assert_eq!(log_component(&long).len(), 80);
    }

    #[test]
    fn chunk_decoder_trims_incomplete_multibyte() {
        // "Д" обрезана посередине, за куском есть ещё данные
        let full = "строка".as_bytes();
        let cut = &full[..full.len() - 1];
        let (text, next_offset, eof) = decode_log_chunk(cut, 0, full.len() as i64);
        // символ отброшен, следующее чтение начнётся с его начала
        assert_eq!(text, "строк");
        assert_eq!(next_offset, "строк".len());
        assert!(!eof);

        let head_only = &full[full.len() - 2..full.len() - 1];
        let (text, next_offset, _) = decode_log_chunk(head_only, 10, 100);
        assert_eq!(text, "");
        assert_eq!(next_offset, 10);

        // конец файла: неполный символ заменяется, а не отбрасывается
        let tail_only = &full[full.len() - 1..];
        let (text, _, eof) = decode_log_chunk(tail_only, 0, tail_only.len() as i64);
        assert_eq!(text, "\u{fffd}");
        assert!(eof);

        let (text, next_offset, eof) = decode_log_chunk(b"plain", 0, 5);
        assert_eq!((text.as_str(), next_offset, eof), ("plain", 5, true));
    }

    #[test]
    fn tail_lines_drop_partial_first_line_and_limit() {
        let content = b"first\nsecond\nthird\nfourth";
        assert_eq!(tail_lines(content, 100, 10), vec!["second", "third", "fourth"]);
        assert_eq!(tail_lines(content, 100, 2), vec!["third", "fourth"]);
        assert_eq!(tail_lines(content, 0, 10), vec!["first", "second", "third", "fourth"]);
        assert!(tail_lines(content, 100, 10).iter().all(|line| !line.contains("partial")));
        assert_eq!(tail_lines(b"no-newline", 5, 10), Vec::<String>::new());
        assert_eq!(tail_lines(b"crlf\r\nline", 0, 10), vec!["crlf", "line"]);
    }

    #[test]
    fn python_splitlines_matches_reference_semantics() {
        assert_eq!(python_splitlines("a\nb"), vec!["a", "b"]);
        assert_eq!(python_splitlines("a\n"), vec!["a"]);
        assert_eq!(python_splitlines(""), Vec::<String>::new());
        assert_eq!(python_splitlines("a\n\n"), vec!["a", ""]);
        assert_eq!(python_splitlines("a\u{2028}b"), vec!["a", "b"]);
    }

    #[test]
    fn process_running_reads_live_marker() {
        let base = std::env::temp_dir().join(format!("nivora-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let marker = base.join(".trial-launcher-live.json");

        assert_eq!(instance_process_running(&base).unwrap(), false);

        fs::write(&marker, br#"{"running": true, "pid": 0}"#).unwrap();
        assert_eq!(instance_process_running(&base).unwrap(), true, "pid<=0 считается работающим");

        let own_pid = format!(r#"{{"running": true, "pid": {}}}"#, std::process::id());
        fs::write(&marker, own_pid).unwrap();
        assert_eq!(instance_process_running(&base).unwrap(), true);

        fs::write(&marker, br#"{"running": true, "pid": 2147483000}"#).unwrap();
        assert_eq!(instance_process_running(&base).unwrap(), false, "несуществующий pid");

        fs::write(&marker, br#"{"running": false, "pid": 1}"#).unwrap();
        assert_eq!(instance_process_running(&base).unwrap(), false);

        fs::write(&marker, b"not json").unwrap();
        assert_eq!(
            instance_process_running(&base).unwrap_err(),
            "Не удалось проверить состояние запуска экземпляра"
        );

        fs::write(&marker, br#"{"running": true, "pid": "abc"}"#).unwrap();
        assert_eq!(instance_process_running(&base).unwrap_err(), "Состояние запуска экземпляра повреждено");

        fs::write(&marker, vec![b'x'; 64 * 1024 + 1]).unwrap();
        assert_eq!(instance_process_running(&base).unwrap_err(), "Состояние запуска экземпляра повреждено");

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn directory_chain_opens_nested_directories_only() {
        let base = std::env::temp_dir().join(format!("nivora-chain-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("a/b")).unwrap();
        fs::write(base.join("a/file.txt"), b"x").unwrap();
        let root_fd = fd::open_directory(&base).unwrap();
        let chain = fd::open_directory_chain(root_fd, &["a".to_owned(), "b".to_owned()]);
        assert!(chain.is_ok());
        fd::close_fd(chain.unwrap());
        assert!(fd::open_directory_chain(root_fd, &["a".to_owned(), "file.txt".to_owned()]).is_err());
        fd::close_fd(root_fd);
        let _ = fs::remove_dir_all(&base);
    }
}
