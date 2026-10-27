//! открывает внешние ссылки, папки игры и системные уведомления

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{json, Value};

use super::config::ConfigStore;
use super::platform_support::open_path;

/// только HTTP(S) без учётных данных и управляющих символов
pub(crate) fn open_external(request: &Value) -> Result<Value, String> {
    let Some(url) = request.get("url").and_then(Value::as_str) else {
        return Err("Некорректная внешняя ссылка".to_owned());
    };
    if url.is_empty() || url != url.trim() || url.chars().any(|char| char < ' ' || char == '\u{7F}') {
        return Err("Некорректная внешняя ссылка".to_owned());
    }
    let parsed = url::Url::parse(url).map_err(|_| "Некорректная внешняя ссылка".to_owned())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("Разрешены только HTTP(S)-ссылки без credentials".to_owned());
    }
    if parsed.host_str().map(|value| value.is_empty()).unwrap_or(true) {
        return Err("Разрешены только HTTP(S)-ссылки без credentials".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Разрешены только HTTP(S)-ссылки без credentials".to_owned());
    }
    open_path(Path::new(url)).map_err(|error| format!("Не удалось открыть ссылку: {error}"))?;
    Ok(json!({"opened": true}))
}

/// раскрывает ~ и ~/… в домашний каталог
pub(crate) fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn path_within(target: &Path, root: &Path) -> bool {
    let Ok(resolved_root) = root.canonicalize() else { return false };
    if target.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return false;
    }
    // если часть пути не существует, проверяем ближайший существующий предок
    let mut current = target.to_path_buf();
    loop {
        if let Ok(resolved) = current.canonicalize() {
            return resolved.starts_with(&resolved_root);
        }
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent.to_path_buf(),
            _ => return false,
        }
    }
}

/// папка игры или её содержимое: ничего вне игрового каталога не открываем
pub(crate) fn open_folder(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let root = config.game_dir()?;
    let requested = request
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(expand_home)
        .unwrap_or_else(|| root.clone());
    let target = requested;
    if !path_within(&target, &root) {
        return Err("Можно открыть только папку игры или её содержимое".to_owned());
    }
    std::fs::create_dir_all(&target).map_err(|error| format!("Не удалось открыть папку: {error}"))?;
    open_path(&target).map_err(|error| format!("Не удалось открыть папку: {error}"))?;
    Ok(json!({"opened": true}))
}

/// отправляет системное уведомление, если лаунчер свёрнут; решение принимает интерфейс
pub(crate) fn system_notify(request: &Value) -> Result<Value, String> {
    let title: String = request
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .chars()
        .take(120)
        .collect();
    let body: String = request
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .chars()
        .take(400)
        .collect();
    if title.is_empty() && body.is_empty() {
        return Ok(json!({"sent": false}));
    }
    let quiet = |command: &mut std::process::Command| {
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    };
    #[cfg(target_os = "macos")]
    {
        let text = (if body.is_empty() { &title } else { &body }).replace('\\', "\\\\").replace('"', "\\\"");
        let header = (if title.is_empty() { "Nivora Launcher" } else { title.as_str() }).replace('\\', "\\\\").replace('"', "\\\"");
        let mut command = super::platform::command("osascript");
        quiet(&mut command);
        let _ = command
            .arg("-e")
            .arg(format!("display notification \"{text}\" with title \"{header}\""))
            .status();
        return Ok(json!({"sent": true}));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let Some(notifier) = which("notify-send") else {
            return Ok(json!({"sent": false}));
        };
        let mut command = super::platform::command(&notifier);
        quiet(&mut command);
        let status = command
            .args(["--app-name", "Nivora Launcher", "--expire-time", "8000"])
            .arg(if title.is_empty() { "Nivora Launcher" } else { title.as_str() })
            .arg(if body.is_empty() { title.as_str() } else { body.as_str() })
            .status();
        match status {
            Ok(_) => Ok(json!({"sent": true})),
            Err(_) => Ok(json!({"sent": false})),
        }
    }
}

/// ищет исполняемый файл в каталогах PATH
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&paths) {
        let candidate = directory.join(name);
        if let Ok(meta) = std::fs::metadata(&candidate) {
            if meta.is_file() && super::platform::is_executable(&meta) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_url_validation() {
        let good = json!({"url": "https://example.org/path"});
        assert!(open_external(&good).is_ok() || open_external(&good).unwrap_err().contains("xdg-open"));

        for bad in [
            json!({"url": ""}),
            json!({"url": " https://example.org"}),
            json!({"url": "https://example.org/\u{7F}"}),
            json!({"url": "ftp://example.org/"}),
            json!({"url": "https://user:pass@example.org/"}),
            json!({"url": "https://example.org:99999/"}),
            json!({"url": "not a url"}),
            json!({}),
        ] {
            let error = open_external(&bad).unwrap_err();
            assert!(
                error.starts_with("Некорректная внешняя ссылка")
                    || error.starts_with("Разрешены только HTTP(S)"),
                "неожиданная ошибка {error:?} для {bad:?}"
            );
        }
    }

    #[test]
    fn folder_validation_uses_game_root() {
        // путь вне игрового каталога отклоняется
        let outside = json!({"path": "/etc"});
        let error = open_folder(&outside).unwrap_err();
        assert_eq!(error, "Можно открыть только папку игры или её содержимое");
    }

    #[test]
    fn expand_home_variants() {
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(expand_home("~"), PathBuf::from(&home));
            assert_eq!(expand_home("~/mods"), PathBuf::from(&home).join("mods"));
        }
        assert_eq!(expand_home("/tmp/absolute"), PathBuf::from("/tmp/absolute"));
        assert_eq!(expand_home("relative"), PathBuf::from("relative"));
    }

    #[test]
    fn notify_skips_empty_payload() {
        assert_eq!(system_notify(&json!({})).unwrap(), json!({"sent": false}));
        assert_eq!(system_notify(&json!({"title": "   ", "body": ""})).unwrap(), json!({"sent": false}));
        // без notify-send в PATH уведомление не отправляется
        let result = system_notify(&json!({"title": "Готово", "body": "Модпак установлен"})).unwrap();
        assert!(result["sent"].is_boolean());
    }

    #[test]
    fn which_finds_a_real_binary() {
        // на Unix cat доступен в PATH; проверяем возвращаемый путь
        if let Some(path) = which("cat") {
            assert!(path.is_file());
            assert!(path.file_name().map(|name| name == "cat").unwrap_or(false));
        } else {
            panic!("cat должен находиться в PATH");
        }
        assert!(which("nivora-definitely-missing-binary-xyz").is_none());
    }
}
