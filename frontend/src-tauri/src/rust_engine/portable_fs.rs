// безопасная работа с путями и атомарная запись
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::mods::random_hex;
use super::platform_support::is_reparse_point;

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// проверяет, что путь остаётся внутри корня и не проходит через ссылку
pub(crate) fn safe_path(root: &Path, parts: &[&str]) -> Result<PathBuf, String> {
    if is_reparse_point(root) {
        return Err("Корневой каталог является ссылкой".to_owned());
    }
    let resolved_root = fs::canonicalize(root).map_err(|_| "Корневой каталог недоступен".to_owned())?;
    let mut current = resolved_root.clone();
    for part in parts {
        validate_component(part)?;
        current = current.join(part);
        if is_reparse_point(&current) {
            return Err("Путь содержит символьную ссылку или junction".to_owned());
        }
        if fs::symlink_metadata(&current).is_ok() {
            let resolved = fs::canonicalize(&current).map_err(|_| "Путь выходит за пределы корневого каталога".to_owned())?;
            if !resolved.starts_with(&resolved_root) {
                return Err("Путь выходит за пределы корневого каталога".to_owned());
            }
        }
    }
    Ok(current)
}

pub(crate) fn validate_component(part: &str) -> Result<(), String> {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part.contains('/')
        || part.contains('\\')
        || part.contains('\0')
        || part.chars().any(|character| "<>:\"|?*".contains(character))
        || part.chars().any(|character| (character as u32) < 32)
        || part.ends_with(' ')
        || part.ends_with('.')
    {
        return Err("Путь содержит недопустимый компонент".to_owned());
    }
    let device_name = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&device_name.as_str()) {
        return Err("Путь содержит зарезервированное имя Windows".to_owned());
    }
    Ok(())
}

pub(crate) fn atomic_write(root: &Path, parts: &[&str], content: &[u8]) -> Result<PathBuf, String> {
    let Some((_last, parents)) = parts.split_last() else {
        return Err("Укажите файл".to_owned());
    };
    let target = safe_path(root, parts)?;
    let parent = safe_path(root, parents)?;
    if !parent.is_dir() {
        return Err("Родительский каталог недоступен".to_owned());
    }
    if fs::symlink_metadata(&target).is_ok() && !fs::metadata(&target).map(|meta| meta.is_file()).unwrap_or(false) {
        return Err("Путь файла является ссылкой или имеет неверный тип".to_owned());
    }
    let mut temporary: Option<PathBuf> = None;
    for _ in 0..8 {
        let candidate = parent.join(format!(".nivora-{}.part", random_hex(8)));
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&candidate);
        match file {
            Ok(mut handle) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&candidate, fs::Permissions::from_mode(0o600));
                }
                let outcome = handle.write_all(content).and_then(|_| handle.sync_all());
                if let Err(error) = outcome {
                    let _ = fs::remove_file(&candidate);
                    return Err(format!("Не удалось записать файл: {error}"));
                }
                temporary = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Не удалось записать файл: {error}")),
        }
    }
    let Some(temporary) = temporary else {
        return Err("Не удалось создать временный файл".to_owned());
    };
    // повторно проверяем путь перед заменой файла
    if safe_path(root, parts).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("Путь выходит за пределы корневого каталога".to_owned());
    }
    if fs::rename(&temporary, &target).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("Не удалось атомарно заменить файл".to_owned());
    }
    sync_parent(&parent);
    Ok(target)
}

fn sync_parent(parent: &Path) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c_path) = std::ffi::CString::new(parent.as_os_str().as_bytes()) {
            let fd = libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY);
            if fd >= 0 {
                libc::fsync(fd);
                libc::close(fd);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
}

pub(crate) fn remove_tree(
    root: &Path,
    parts: &[&str],
    reserved: Option<&dyn Fn(&str) -> bool>,
    max_entries: usize,
    max_depth: usize,
) -> Result<(), String> {
    if parts.is_empty() {
        return Err("Нельзя удалить корневой каталог".to_owned());
    }
    let target = safe_path(root, parts)?;
    let mut count = 0usize;
    let relative: Vec<String> = parts.iter().map(|part| (*part).to_owned()).collect();
    remove_recursive(root, &target, &relative, 0, reserved, max_entries, max_depth, &mut count)
}

fn remove_recursive(
    root: &Path,
    path: &Path,
    relative: &[String],
    depth: usize,
    reserved: Option<&dyn Fn(&str) -> bool>,
    max_entries: usize,
    max_depth: usize,
    count: &mut usize,
) -> Result<(), String> {
    if depth > max_depth {
        return Err("Папка содержит слишком много уровней вложенности".to_owned());
    }
    let parts: Vec<&str> = relative.iter().map(String::as_str).collect();
    safe_path(root, &parts)?;
    if is_reparse_point(path) {
        return Err("Нельзя удалять символьные ссылки или junction".to_owned());
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_file() {
            fs::remove_file(path).map_err(|_| "Не удалось удалить файл".to_owned())?;
            return Ok(());
        }
    }
    if !path.is_dir() {
        return Err("Можно удалить только обычный файл или папку".to_owned());
    }
    let children: Vec<(String, PathBuf)> = fs::read_dir(path)
        .map_err(|_| "Можно удалить только обычный файл или папку".to_owned())?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((name, entry.path()))
        })
        .collect();
    for (name, child) in children {
        if let Some(reserved) = reserved {
            if reserved(&name) {
                return Err("Папка содержит служебные данные лаунчера".to_owned());
            }
        }
        *count += 1;
        if *count > max_entries {
            return Err("Папка содержит слишком много файлов для удаления".to_owned());
        }
        let mut next = relative.to_vec();
        next.push(name);
        remove_recursive(root, &child, &next, depth + 1, reserved, max_entries, max_depth, count)?;
    }
    fs::remove_dir(path).map_err(|_| "Не удалось удалить папку".to_owned())
}

pub(crate) fn regular_file(path: &Path, maximum_size: Option<u64>) -> Result<fs::Metadata, String> {
    if is_reparse_point(path) {
        return Err("Файл является ссылкой или junction".to_owned());
    }
    let value = fs::metadata(path).map_err(|_| "Файл недоступен".to_owned())?;
    if !value.is_file() || maximum_size.is_some_and(|limit| value.len() > limit) {
        return Err("Файл недоступен или превышает допустимый размер".to_owned());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("nivora-portable-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn components_reject_windows_and_control_hazards() {
        assert!(validate_component("mods").is_ok());
        assert!(validate_component("файл.txt").is_ok(), "юникод допустим");
        assert!(validate_component("").is_err());
        assert!(validate_component("..").is_err());
        assert!(validate_component("a/b").is_err());
        assert!(validate_component("a\\b").is_err());
        assert!(validate_component("a:b").is_err());
        assert!(validate_component("con").is_err(), "имя устройства");
        assert!(validate_component("COM1.tar.gz").is_err(), "префикс имени устройства");
        assert!(validate_component("trailing. ").is_err());
        assert!(validate_component("trailing.").is_err());
        assert!(validate_component("ctrl\u{1}x").is_err());
        assert!(validate_component("nul.txt").is_err());
    }

    #[test]
    fn safe_path_rejects_links_and_escapes() {
        let base = sandbox("safe");
        fs::create_dir_all(base.join("inner")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", base.join("inner").join("escape")).unwrap();
        assert!(safe_path(&base, &["inner"]).is_ok());
        #[cfg(unix)]
        assert_eq!(
            safe_path(&base, &["inner", "escape", "passwd"]).unwrap_err(),
            "Путь содержит символьную ссылку или junction"
        );
        #[cfg(unix)]
        assert!(safe_path(&base, &["нет-такого", "глубже"]).is_ok(), "несуществующий путь допустим");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn atomic_write_replaces_content_safely() {
        let base = sandbox("atomic");
        fs::create_dir_all(base.join("dir")).unwrap();
        atomic_write(&base, &["dir", "file.txt"], "первый".as_bytes()).unwrap();
        assert_eq!(fs::read(base.join("dir/file.txt")).unwrap(), "первый".as_bytes());
        atomic_write(&base, &["dir", "file.txt"], "второй".as_bytes()).unwrap();
        assert_eq!(fs::read(base.join("dir/file.txt")).unwrap(), "второй".as_bytes());
        let leftovers: Vec<_> = fs::read_dir(base.join("dir")).unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".nivora-"))
            .collect();
        assert!(leftovers.is_empty());
        assert_eq!(atomic_write(&base, &[] as &[&str], b"x").unwrap_err(), "Укажите файл");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_tree_deletes_files_and_directories() {
        let base = sandbox("remove");
        fs::create_dir_all(base.join("pack/sub")).unwrap();
        fs::write(base.join("pack/a.txt"), b"a").unwrap();
        fs::write(base.join("pack/sub/b.txt"), b"b").unwrap();
        remove_tree(&base, &["pack"], None, 100, 8).unwrap();
        assert!(!base.join("pack").exists());

        fs::create_dir_all(base.join("keep")).unwrap();
        fs::write(base.join("keep/.trial-integrity"), b"").unwrap();
        let reserved = |name: &str| name.starts_with(".trial-");
        assert_eq!(
            remove_tree(&base, &["keep"], Some(&reserved), 100, 8).unwrap_err(),
            "Папка содержит служебные данные лаунчера"
        );
        assert_eq!(remove_tree(&base, &[] as &[&str], None, 10, 8).unwrap_err(), "Нельзя удалить корневой каталог");

        #[cfg(unix)]
        {
            let outside = sandbox("remove-outside");
            std::os::unix::fs::symlink(&outside, base.join("link")).unwrap();
            assert_eq!(
                remove_tree(&base, &["link"], None, 10, 8).unwrap_err(),
                "Путь содержит символьную ссылку или junction"
            );
            let _ = fs::remove_dir_all(&outside);
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn regular_file_checks_type_and_size() {
        let base = sandbox("regular");
        fs::write(base.join("data.bin"), vec![0u8; 10]).unwrap();
        assert!(regular_file(&base.join("data.bin"), None).is_ok());
        assert!(regular_file(&base.join("data.bin"), Some(5)).is_err());
        assert!(regular_file(&base.join("missing"), None).is_err());
        assert!(regular_file(&base, None).is_err(), "каталог не файл");
        let _ = fs::remove_dir_all(&base);
    }
}
