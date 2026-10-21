// кроссплатформенная замена Unix-правам на файлах лаунчера. Токены и
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// выставить режим файла (0700/0755/0600…). На Windows — no-op.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) {}

pub fn restrict_dir(path: &Path) {
    set_mode(path, 0o700);
}

pub fn restrict_file(path: &Path) {
    set_mode(path, 0o600);
}

/// write+create+truncate с правами 0600 (временные файлы токенов/расписаний).
#[cfg(unix)]
pub fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
pub fn create_private(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

#[cfg(unix)]
pub fn create_private_new(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
pub fn create_private_new(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
pub fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub fn is_executable(_meta: &std::fs::Metadata) -> bool {
    true
}

/// Единственная точка создания дочерних процессов движка. На Windows процесс
/// GUI-подсистемы не имеет консоли, и каждый консольный дочерний процесс
/// (java, ffmpeg, powershell, taskkill, winget, хелперы Go/C++) открывал бы
/// своё окно. CREATE_NO_WINDOW подавляет его; на других ОС — обычный Command.
pub fn command<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Каталоги, где лежат нативные хелперы (nivora-turbo, nivora-modcat,
/// ModScan.java) в каждой раскладке: установщик Windows (`<exe>/backend`),
/// .app на macOS (`Contents/Resources/backend`), распакованный встроенный
/// бэкенд Windows, пакеты Linux и dev-дерево (`src-tauri/resources/backend`).
/// Раньше каждый модуль искал только `<exe>/имя` без `.exe` — в установленных
/// сборках Windows и macOS хелперы не находились вовсе.
pub fn helper_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            directories.push(dir.to_path_buf());
            directories.push(dir.join("backend"));
            directories.push(dir.join("resources").join("backend"));
            if let Some(parent) = dir.parent() {
                directories.push(parent.join("Resources").join("backend"));
                directories.push(parent.join("lib").join("nivora-launcher"));
                directories.push(parent.join("lib").join("Nivora Launcher").join("backend"));
                directories.push(parent.join("resources").join("backend"));
                if let Some(grand) = parent.parent() {
                    directories.push(grand.join("resources").join("backend"));
                }
            }
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        directories.push(
            PathBuf::from(local)
                .join(super::state::LAUNCHER_DIR)
                .join("backend")
                .join(format!("v{}", env!("CARGO_PKG_VERSION"))),
        );
    }
    directories.push(PathBuf::from("/usr/lib/nivora-launcher"));
    directories.push(PathBuf::from("/usr/lib/Nivora Launcher/backend"));
    directories
}

/// Путь к хелперу: сначала переменная окружения, затем [`helper_dirs`].
/// `name` — имя без расширения; на Windows пробуется и `name.exe`.
pub fn helper_path(name: &str, env_var: &str) -> Option<std::path::PathBuf> {
    if let Ok(value) = std::env::var(env_var) {
        let path = std::path::PathBuf::from(value.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let file_names: Vec<String> = if cfg!(windows) && !name.contains('.') {
        vec![format!("{name}.exe"), name.to_owned()]
    } else {
        vec![name.to_owned()]
    };
    helper_dirs()
        .into_iter()
        .flat_map(|directory| file_names.iter().map(move |file| directory.join(file)).collect::<Vec<_>>())
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    #[test]
    fn helper_path_prefers_the_environment_override() {
        let directory = std::env::temp_dir().join(format!("nivora-helper-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("custom-helper");
        std::fs::write(&file, b"x").unwrap();
        std::env::set_var("NIVORA_TEST_HELPER_BIN", &file);
        assert_eq!(super::helper_path("nivora-test-helper", "NIVORA_TEST_HELPER_BIN"), Some(file.clone()));
        std::env::remove_var("NIVORA_TEST_HELPER_BIN");
        assert_eq!(super::helper_path("nivora-test-helper-absent", "NIVORA_TEST_HELPER_BIN"), None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn helper_dirs_cover_macos_bundle_and_windows_installer_layouts() {
        let directories = super::helper_dirs();
        assert!(directories.iter().any(|path| path.ends_with("Resources/backend")));
        assert!(directories.iter().any(|path| path.ends_with("backend")));
    }

    /// Регрессия: прямой Command::new в коде движка снова открыл бы окно
    /// консоли на Windows. Тестовые модули не проверяются.
    #[test]
    fn engine_spawns_processes_only_through_platform_command() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rust_engine");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&directory).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().and_then(|name| name.to_str()) == Some("platform.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let production = source.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
            if production.contains("Command::new(") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(offenders.is_empty(), "Command::new вне platform::command: {offenders:?}");
    }
}
