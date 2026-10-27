// один клиентский или серверный процесс на экземпляр; flock на Unix, LockFileEx на Windows
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct InstanceProcessLock {
    path: PathBuf,
    #[cfg(unix)]
    fd: Option<i32>,
    #[cfg(windows)]
    file: Option<std::fs::File>,
    lock_backend: Option<&'static str>,
}

impl InstanceProcessLock {
    pub(crate) fn new(data_home: &Path, instance_name: &str) -> Self {
        let mut sanitized = String::new();
        let mut in_run = false;
        for character in instance_name.chars() {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                sanitized.push(character);
                in_run = false;
            } else if !in_run {
                sanitized.push('_');
                in_run = true;
            }
        }
        let sanitized = sanitized.trim_matches(|c: char| matches!(c, '.' | '_' | '-')).to_owned();
        let sanitized = if sanitized.is_empty() { "instance".to_owned() } else { sanitized };
        let digest = Sha256::digest(instance_name.as_bytes());
        let name_hash: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();
        let safe: String = sanitized.chars().take(64).collect();
        Self {
            path: data_home.join(".locks").join(format!("{safe}-{name_hash}.lock")),
            #[cfg(unix)]
            fd: None,
            #[cfg(windows)]
            file: None,
            lock_backend: None,
        }
    }

    pub(crate) fn acquire(&mut self) -> Result<(), String> {
        if self.acquired() {
            return Ok(());
        }
        let parent = self.path.parent().unwrap_or(Path::new(".")).to_path_buf();
        fs::create_dir_all(&parent).map_err(|error| format!("Не удалось открыть блокировку экземпляра: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&parent, fs::Permissions::from_mode(0o700));
            let c_path = std::ffi::CString::new(self.path.as_os_str().as_bytes())
                .map_err(|_| "Не удалось открыть блокировку экземпляра".to_owned())?;
            let flags = libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW;
            let fd = unsafe { libc::open(c_path.as_ptr(), flags, 0o600) };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                return Err(format!("Не удалось открыть блокировку экземпляра: {error}"));
            }
            let status = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if status != 0 {
                let error = std::io::Error::last_os_error();
                unsafe { libc::close(fd) };
                let code = error.raw_os_error().unwrap_or(0);
                if code == libc::EACCES || code == libc::EAGAIN {
                    return Err("Экземпляр уже запущен".to_owned());
                }
                return Err(format!("Не удалось заблокировать экземпляр: {error}"));
            }
            self.fd = Some(fd);
            self.lock_backend = Some("fcntl");
            Ok(())
        }
        #[cfg(windows)]
        {
            use std::io::Write;
            use std::os::windows::fs::OpenOptionsExt;
            use std::os::windows::io::AsRawHandle;
            const FILE_SHARE_READ_WRITE: u32 = 0x3;
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .share_mode(FILE_SHARE_READ_WRITE)
                .open(&self.path)
                .map_err(|error| format!("Не удалось открыть блокировку экземпляра: {error}"))?;
            let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
            let mut file = file;
            if size == 0 {
                let _ = file.write_all(&[0]);
            }
            unsafe {
                use windows_sys::Win32::Foundation::HANDLE;
                use windows_sys::Win32::Storage::FileSystem::{
                    LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
                };
                use windows_sys::Win32::System::IO::OVERLAPPED;
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                let status = LockFileEx(
                    file.as_raw_handle() as HANDLE,
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    1,
                    0,
                    &mut overlapped,
                );
                if status == 0 {
                    let error = std::io::Error::last_os_error();
                    let denied = error.raw_os_error().is_some_and(|code| matches!(code, 13 | 36))
                        || error.kind() == std::io::ErrorKind::PermissionDenied;
                    if denied {
                        return Err("Экземпляр уже запущен".to_owned());
                    }
                    return Err(format!("Не удалось заблокировать экземпляр: {error}"));
                }
            }
            self.file = Some(file);
            self.lock_backend = Some("msvcrt");
            Ok(())
        }
    }

    fn acquired(&self) -> bool {
        #[cfg(unix)]
        { self.fd.is_some() }
        #[cfg(windows)]
        { self.file.is_some() }
    }

    pub(crate) fn release(&mut self) {
        #[cfg(unix)]
        if let Some(fd) = self.fd.take() {
            if self.lock_backend == Some("fcntl") {
                unsafe { libc::flock(fd, libc::LOCK_UN) };
            }
            self.lock_backend = None;
            unsafe { libc::close(fd) };
        }
        #[cfg(windows)]
        if let Some(file) = self.file.take() {
            if self.lock_backend == Some("msvcrt") {
                use std::os::windows::io::AsRawHandle;
                unsafe {
                    use windows_sys::Win32::Foundation::HANDLE;
                    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
                    use windows_sys::Win32::System::IO::OVERLAPPED;
                    let mut overlapped: OVERLAPPED = std::mem::zeroed();
                    UnlockFileEx(file.as_raw_handle() as HANDLE, 0, 1, 0, &mut overlapped);
                }
            }
            self.lock_backend = None;
            drop(file);
        }
    }
}

impl Drop for InstanceProcessLock {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_names_are_sanitized_and_stable() {
        let base = std::env::temp_dir();
        let first = InstanceProcessLock::new(&base, "мой сервер #1!");
        let second = InstanceProcessLock::new(&base, "мой сервер #1!");
        assert_eq!(first.path, second.path);
        let name = first.path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(".lock"), "{name}");
        assert!(!name.contains("сервер"));
        let fallback = InstanceProcessLock::new(&base, "///");
        assert!(fallback.path.file_name().unwrap().to_string_lossy().starts_with("instance"));
    }

    #[test]
    fn exclusive_lock_conflicts_with_itself() {
        let base = std::env::temp_dir().join(format!("nivora-lock-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let mut first = InstanceProcessLock::new(&base, "solo");
        first.acquire().unwrap();
        let mut second = InstanceProcessLock::new(&base, "solo");
        assert_eq!(second.acquire().unwrap_err(), "Экземпляр уже запущен");
        first.release();
        second.acquire().unwrap();
        drop(second);
        let _ = fs::remove_dir_all(&base);
    }
}
