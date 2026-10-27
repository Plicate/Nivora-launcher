//! импорт и удаление обоев лаунчера с проверкой формата и атомарной записью
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use super::config::ConfigStore;

const MAX_WALLPAPER_BYTES: usize = 32 * 1024 * 1024;
const WALLPAPER_SUFFIXES: [&str; 4] = [".png", ".jpg", ".jpeg", ".webp"];

fn err(message: &str) -> String {
    message.to_owned()
}

fn valid_wallpaper_bytes(content: &[u8], suffix: &str) -> bool {
    match suffix {
        ".png" => content.starts_with(b"\x89PNG\r\n\x1a\n"),
        ".jpg" | ".jpeg" => content.starts_with(b"\xff\xd8\xff"),
        _ => content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP",
    }
}

fn decode_wallpaper(encoded: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| err("Обои повреждены"))?;
    if content.len() > MAX_WALLPAPER_BYTES {
        return Err(err("Файл обоев слишком большой (максимум 32 MiB)"));
    }
    Ok(content)
}

fn wallpaper_dir(config: &ConfigStore) -> Result<PathBuf, String> {
    let directory = config.paths.data_home.join("wallpapers");
    if directory.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
        return Err(err("Каталог обоев является символьной ссылкой"));
    }
    fs::create_dir_all(&directory).map_err(|_| err("Каталог обоев недоступен"))?;
    let meta = fs::symlink_metadata(&directory).map_err(|_| err("Каталог обоев недоступен"))?;
    if meta.is_symlink() || !meta.is_dir() {
        return Err(err("Каталог обоев недоступен"));
    }
    Ok(directory)
}

pub fn wallpaper_import(request: &Value) -> Result<Value, String> {
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or_default();
    let content = decode_wallpaper(encoded)?;
    let raw_name = request.get("file_name").and_then(Value::as_str).unwrap_or("wallpaper.png");
    let file_name = PathBuf::from(raw_name)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "wallpaper.png".to_owned());
    let suffix = PathBuf::from(&file_name)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if !WALLPAPER_SUFFIXES.contains(&suffix.as_str()) {
        return Err(err("Поддерживаются PNG, JPEG и WebP"));
    }
    if !valid_wallpaper_bytes(&content, &suffix) {
        return Err(err("Файл не является корректным изображением"));
    }
    let mut config = ConfigStore::load()?;
    let directory = wallpaper_dir(&config)?;
    let target = directory.join(format!("background{suffix}"));
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let previous = entry.path();
            if previous == target {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("background.")
                && (previous.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) || previous.is_file())
            {
                let _ = fs::remove_file(&previous);
            }
        }
    }
    super::portable_fs::atomic_write(&directory, &[&format!("background{suffix}")], &content)?;
    config.set_values(&json!({ "wallpaper_path": target.to_string_lossy() }))?;
    Ok(json!({ "settings": config.settings_public() }))
}

/// удаляет файл обоев лаунчера и очищает настройку пути
pub fn wallpaper_clear(_request: &Value) -> Result<Value, String> {
    let mut config = ConfigStore::load()?;
    let current = config.str_setting("wallpaper_path");
    let directory = config.paths.data_home.join("wallpapers");
    if directory.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
        return Err(err("Каталог обоев является символьной ссылкой"));
    }
    if !current.is_empty() {
        let path = PathBuf::from(&current);
        if path.parent() == Some(&directory)
            && path.symlink_metadata().is_ok_and(|meta| !meta.is_symlink())
            && path.exists()
        {
            let _ = fs::remove_file(&path);
        }
    }
    config.set_values(&json!({ "wallpaper_path": "" }))?;
    Ok(json!({ "settings": config.settings_public() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_validate_formats() {
        assert!(valid_wallpaper_bytes(b"\x89PNG\r\n\x1a\nrest", ".png"));
        assert!(!valid_wallpaper_bytes(b"\x89PNG", ".png"));
        assert!(valid_wallpaper_bytes(b"\xff\xd8\xffdata", ".jpg"));
        assert!(valid_wallpaper_bytes(b"\xff\xd8\xffdata", ".jpeg"));
        assert!(!valid_wallpaper_bytes(b"\xff\xd8\xffdata", ".png"));
        assert!(valid_wallpaper_bytes(b"RIFF\x00\x00\x00\x00WEBPVP8 ", ".webp"));
        assert!(!valid_wallpaper_bytes(b"RIFF\x00\x00\x00\x00WAVEfmt ", ".webp"));
        assert!(!valid_wallpaper_bytes(b"RIFF\x00WEBP", ".webp"));
    }

    #[test]
    fn import_writes_file_updates_settings_and_clear_removes() {
        let root = std::env::temp_dir().join(format!("nivora-wallpaper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let roots = super::super::state::LauncherRoots {
            config_home: root.join("config"),
            data_home: root.join("data"),
            cache_home: root.join("cache"),
        };
        let mut config = ConfigStore::from_paths(super::super::config::Paths::from_roots(&roots)).expect("хранилище");
        let directory = wallpaper_dir(&config).expect("каталог обоев");

        let encoded = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        let content = decode_wallpaper(encoded).expect("декодирование");
        let suffix = ".png".to_owned();
        assert!(valid_wallpaper_bytes(&content, &suffix));
        super::super::portable_fs::atomic_write(&directory, &["background.png"], &content).expect("запись");
        let target = directory.join("background.png");
        config.set_values(&json!({ "wallpaper_path": target.to_string_lossy() })).unwrap();
        assert!(target.is_file());
        assert_eq!(config.str_setting("wallpaper_path"), target.to_string_lossy().to_string());
        assert!(decode_wallpaper("###").is_err(), "битый base64 отклоняется");

        let old = directory.join("background.jpg");
        fs::write(&old, b"\xff\xd8\xffold").unwrap();
        let next = directory.join("background.jpg");
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let previous = entry.path();
                if previous == next {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("background.") && previous.is_file() {
                    let _ = fs::remove_file(&previous);
                }
            }
        }
        assert!(!old.is_file() || old == next);

        config.set_values(&json!({ "wallpaper_path": "" })).unwrap();
        let _ = fs::remove_file(&target);
        assert!(!target.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
