// отмена длительных операций: Tauri-команда создаёт marker-файл, движок
// проверяет его в точках, где безопасно прерваться.
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct CancellationToken {
    operation_id: String,
    path: Option<PathBuf>,
}

impl CancellationToken {
    pub fn new(operation_id: &str) -> Self {
        let id = operation_id.trim();
        let safe: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let path = if id.is_empty() {
            None
        } else {
            Some(marker_path(&safe))
        };
        Self { operation_id: id.to_owned(), path }
    }

    pub fn cancelled(&self) -> bool {
        match &self.path {
            Some(path) => path.exists() || legacy_marker_path(&self.operation_id).exists(),
            None => false,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            return Err("Операция остановлена пользователем".to_owned());
        }
        Ok(())
    }

    /// ожидание с периодической проверкой отмены.
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
        let _ = fs::remove_file(legacy_marker_path(&self.operation_id));
    }
}

pub fn request_cancel(operation_id: &str) -> bool {
    let token = CancellationToken::new(operation_id);
    match &token.path {
        Some(path) => {
            if let Some(directory) = path.parent() {
                let _ = fs::create_dir_all(directory);
            }
            fs::write(path, b"").map(|_| true).unwrap_or(false)
        }
        None => false,
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nivora-launcher-runtime"))
}

fn marker_path(safe_id: &str) -> PathBuf {
    runtime_dir().join(format!("nivora-launcher-{safe_id}.cancel"))
}

fn legacy_marker_path(operation_id: &str) -> PathBuf {
    let safe: String = operation_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    std::env::temp_dir().join(format!("nivora-launcher-{safe}.cancel"))
}
