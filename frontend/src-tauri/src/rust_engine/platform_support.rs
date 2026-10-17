// Тонкий слой различий платформ: права дескрипторов, reparse-точки, сигналы
// процессам, открытие путей в системе.
use std::path::{Path, PathBuf};

/// `canonicalize` на Windows возвращает verbatim-пути вида `\\?\C:\…`.
/// Java их не понимает (InvalidPathException: Illegal char <?>): Forge/NeoForge
/// installer, JVM и моды падали на таких путях. Для путей, уходящих во внешние
/// процессы и в сравнения, используем обычную форму `C:\…`; длинные пути
/// стандартная библиотека Rust при обращении к ФС префиксует сама.
pub(crate) fn simplify_path(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        if let Some(simple) = strip_verbatim_prefix(&path.to_string_lossy()) {
            return PathBuf::from(simple);
        }
    }
    path
}

/// `canonicalize` + [`simplify_path`].
pub(crate) fn canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify_path)
}

pub(crate) fn strip_verbatim_prefix(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = text.strip_prefix(r"\\?\")?;
    let bytes = rest.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some(rest.to_owned());
    }
    None
}

/// Применяет POSIX-биты прав там, где они есть; Windows пользуется ACL профиля.
#[cfg(unix)]
pub(crate) fn set_private_fd(fd: i32, mode: u32) {
    unsafe {
        libc::fchmod(fd, mode);
    }
}

#[cfg(not(unix))]
pub(crate) fn set_private_fd(_fd: i32, _mode: u32) {}

pub(crate) fn is_reparse_point(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.is_symlink() {
                return true;
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                return metadata.file_attributes() & 0x400 != 0; // FILE_ATTRIBUTE_REPARSE_POINT
            }
            #[cfg(not(windows))]
            false
        }
        Err(_) => false,
    }
}

pub(crate) fn process_exists(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    unsafe {
        // kill с нулевым сигналом только проверяет существование.
        let status = libc::kill(pid, 0);
        if status == 0 {
            return true;
        }
        std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
            CloseHandle(handle);
            return true;
        }
        // Отказ в доступе всё равно означает, что PID существует.
        std::io::Error::last_os_error().raw_os_error() == Some(5)
    }
}

/// Платформенные флаги запуска дочерних процессов: отдельная сессия/группа,
/// чтобы серверы и Java не получали сигналы лаунчера.
pub(crate) fn prepare_process(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

/// Завершение дерева процессов: группа на unix, taskkill /T на Windows.
pub(crate) fn terminate_process_tree(pid: i32, force: bool) {
    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let killed = unsafe { libc::kill(-pid, signal) } == 0;
        if !killed {
            unsafe { libc::kill(pid, signal) };
        }
    }
    #[cfg(windows)]
    {
        let taskkill = which_taskkill();
        if let Some(taskkill) = taskkill {
            let output = std::process::Command::new(taskkill)
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if output.is_ok() {
                return;
            }
        }
        unsafe {
            use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}

#[cfg(windows)]
fn which_taskkill() -> Option<std::path::PathBuf> {
    let system32 = std::env::var_os("SystemRoot").map(|root| {
        Path::new(&root).join("System32").join("taskkill.exe")
    })?;
    system32.exists().then_some(system32)
}

/// Открытие пути средствами системы (файловый менеджер, браузер и т.п.).
pub(crate) fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(path.as_os_str())
            .spawn();
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let opener = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|directory| {
                let candidate = directory.join("xdg-open");
                candidate.is_file().then_some(candidate)
            })
        });
        let Some(opener) = opener else {
            return Err("Не найден xdg-open. Установите пакет xdg-utils.".to_owned());
        };
        let _ = std::process::Command::new(opener).arg(path).spawn();
        Ok(())
    }
}

/// Имя платформы в терминах sys.platform из Python.
pub(crate) fn sys_platform() -> &'static str {
    #[cfg(windows)]
    { "win32" }
    #[cfg(target_os = "macos")]
    { "darwin" }
    #[cfg(all(unix, not(target_os = "macos")))]
    { "linux" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefix_is_stripped_for_drive_and_unc_paths() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\Users\Nick\game").as_deref(), Some(r"C:\Users\Nick\game"));
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\server\share\x").as_deref(), Some(r"\\server\share\x"));
        assert_eq!(strip_verbatim_prefix(r"\\?\Volume{abc}\x"), None);
        assert_eq!(strip_verbatim_prefix(r"C:\plain"), None);
        assert_eq!(strip_verbatim_prefix("/home/user"), None);
    }

    #[test]
    fn process_existence_reflects_current_and_bogus_pids() {
        assert!(process_exists(std::process::id() as i32));
        assert!(!process_exists(0));
        assert!(!process_exists(-5));
        // PID 0x7fffffff на Linux в пользовательском пространстве не существует.
        assert!(!process_exists(i32::MAX));
    }

    #[test]
    fn reparse_detection_treats_symlinks_as_reparse() {
        let base = std::env::temp_dir().join(format!("nivora-platform-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("target", base.join("link")).unwrap();
        assert!(is_reparse_point(&base.join("link")));
        assert!(!is_reparse_point(&base));
        assert!(!is_reparse_point(&base.join("нет-такого")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn platform_name_matches_python() {
        let platform = sys_platform();
        assert!(matches!(platform, "linux" | "darwin" | "win32"));
    }
}
