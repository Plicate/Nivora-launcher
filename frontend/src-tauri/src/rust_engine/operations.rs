// долгие операции проверяют файл-маркер отмены в безопасных точках
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct CancellationToken {
    path: Option<PathBuf>,
}

impl CancellationToken {
    pub fn new(operation_id: &str) -> Self {
        let id = operation_id.trim();
        let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let path = valid.then(|| marker_path(id));
        Self { path }
    }

    pub fn cancelled(&self) -> bool {
        match &self.path {
            Some(path) => path.exists(),
            None => false,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            return Err("Операция остановлена пользователем".to_owned());
        }
        Ok(())
    }

    pub fn wait(&self, seconds: f64) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
        loop {
            self.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            std::thread::sleep(remaining.min(Duration::from_millis(100)));
        }
    }

    pub fn cleanup(&self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn request_cancel(operation_id: &str) -> bool {
    let token = CancellationToken::new(operation_id);
    match &token.path {
        Some(path) => {
            if ensure_runtime_dir().is_err() {
                return false;
            }
            fs::write(path, b"").map(|_| true).unwrap_or(false)
        }
        None => false,
    }
}

fn runtime_dir() -> PathBuf {
    let xdg_runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty());
    match xdg_runtime {
        Some(path) => PathBuf::from(path).join("nivora-launcher"),
        None => {
            #[cfg(unix)]
            {
                std::env::temp_dir().join(format!("nivora-launcher-runtime-{}", unsafe { libc::geteuid() }))
            }
            #[cfg(not(unix))]
            {
                std::env::temp_dir().join("nivora-launcher-runtime")
            }
        }
    }
}

pub(crate) fn ensure_runtime_dir() -> Result<PathBuf, String> {
    let directory = runtime_dir();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("Не удалось создать runtime-каталог: {error}")),
        }
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|error| format!("Не удалось проверить runtime-каталог: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Runtime-каталог недоступен".to_owned());
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Не удалось ограничить runtime-каталог: {error}"))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать runtime-каталог: {error}"))?;
    Ok(directory)
}

fn marker_path(safe_id: &str) -> PathBuf {
    runtime_dir().join(format!("nivora-launcher-{safe_id}.cancel"))
}

