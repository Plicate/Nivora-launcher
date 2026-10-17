// Кроссплатформенная замена Unix-правам на файлах лаунчера. Токены и
// расписания на Unix лежат с 0600/0700; на Windows POSIX-битов нет —
// профиль пользователя уже изолирован ACL, поэтому сеттеры там no-op.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Выставить режим файла (0700/0755/0600…). На Windows — no-op.
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

/// Как create_private, но create_new: падает, если файл уже есть
/// (восстановление снапшота не должно затирать существующее).
#[cfg(unix)]
pub fn create_private_new(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
pub fn create_private_new(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Есть ли право исполнения (поиск java в PATH). На Windows исполнимость
/// определяется расширением, а не битами прав — файл считаем исполнимым.
#[cfg(unix)]
pub fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub fn is_executable(_meta: &std::fs::Metadata) -> bool {
    true
}
