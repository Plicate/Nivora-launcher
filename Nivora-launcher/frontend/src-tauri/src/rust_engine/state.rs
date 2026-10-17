// Сводное состояние для игрового оверлея: запущенный экземпляр, метрики
// процесса игры, моды, статус записи и настройки оверлея — один запрос.
// Всё читается с диска и из /proc, без обращения к воркеру.
//
// Формат ответа фиксирует оверлей: когда игра не запущена, ключа
// system_metrics в ответе нет.
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::metrics;

pub(crate) const MAX_SETTINGS_JSON_BYTES: u64 = 1024 * 1024;
const MAX_INSTANCE_PROFILE_JSON_BYTES: u64 = 256 * 1024;
const MAX_INSTANCE_LIVE_STATE_JSON_BYTES: u64 = 64 * 1024;
const MAX_INSTANCE_VERSION_JSON_BYTES: u64 = 8 * 1024 * 1024;
const INSTANCE_METADATA_FILE: &str = ".nivora-launcher-instance.json";
const LIVE_STATE_FILE: &str = ".nivora-launcher-live.json";
const SERVER_CONFIG_FILE: &str = ".nivora-launcher-server.json";
const RECORDING_STATE_FILE: &str = "recording-state.json";
const PROJECT_METADATA_SUFFIX: &str = ".trial-project.json";
const MAX_MEASURED_FILES: usize = 20_000;
const EXCLUDED_PARTS: [&str; 3] = [".nivora-launcher-snapshots", "logs", "cache"];
const OVERLAY_MODS_LIMIT: usize = 200;
const OVERLAY_MODS_CACHE_SECONDS: f64 = 10.0;

static OVERLAY_MODS_CACHE: Mutex<Option<HashMap<String, (f64, Value)>>> = Mutex::new(None);

pub fn overlay_state(_request: &Value) -> Result<Value, String> {
    let now = epoch_seconds();
    let roots = launcher_roots();
    Ok(overlay_state_from(&roots.config_home, &roots.data_home, now))
}

pub(crate) fn home_dir() -> PathBuf {
    home_dir_for(cfg!(windows), &|key| std::env::var(key).ok())
}

/// Домашний каталог: на Windows переменной HOME обычно нет — берём
/// USERPROFILE (как os.path.expanduser в Python), иначе HOME, иначе корень.
/// До этого Windows-сборка получала «/» и складывала данные в C:\.config.
fn home_dir_for(windows: bool, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if windows {
        if let Some(profile) = env("USERPROFILE").filter(|value| !value.is_empty()) {
            return PathBuf::from(profile);
        }
    }
    if let Some(home) = env("HOME") {
        return PathBuf::from(home);
    }
    PathBuf::from(if windows { "\\" } else { "/" })
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

/// XDG-корни лаунчера (каждый уже с вложенным nivora-launcher).
pub(crate) struct LauncherRoots {
    pub(crate) config_home: PathBuf,
    pub(crate) data_home: PathBuf,
    pub(crate) cache_home: PathBuf,
}

/// Приоритет корней — одна точка правды для state, ConfigStore и bedrock.
/// Каталог настроек/данных лаунчера. ОБЯЗАН совпадать с APP_ID в Python
/// (nivora_launcher/constants.py): историческое имя «trial-launcher»
/// осталось только в миграции ниже — данные пользователей переносятся
/// в актуальный «nivora-launcher» при первом запуске.
pub(crate) const LAUNCHER_DIR: &str = "nivora-launcher";
/// Исторический каталог до переименования бренда (миграция trial → nivora).
pub(crate) const LEGACY_LAUNCHER_DIR: &str = "trial-launcher";

pub(crate) fn launcher_roots() -> LauncherRoots {
    launcher_roots_for(cfg!(windows), &|key| std::env::var(key).ok())
}

/// Старые сборки Rust-движка писали в параллельный каталог «nivora-launcher»
/// (до синхронизации APP_ID с Python). Созданные там экземпляры переносим в
/// актуальную папку игры, чтобы профили не пропадали после обновления.
/// Повторные запуски ничего не делают: источник переименовывается.
pub(crate) fn migrate_legacy_roots() -> Option<String> {
    let roots = launcher_roots();
    let mut notes: Vec<String> = Vec::new();
    let pairs = [
        (&roots.config_home, "настройки"),
        (&roots.data_home, "данные"),
        (&roots.cache_home, "кэш"),
    ];
    for (current, label) in pairs {
        let Some(legacy) = current.parent().map(|parent| parent.join(LEGACY_LAUNCHER_DIR)) else {
            continue;
        };
        if !legacy.is_dir() || is_symlink(&legacy) {
            continue;
        }
        if !current.exists() {
            // Чистый случай: новый каталог ещё не создавался — переносим всё
            // целиком, это сохраняет права и структуру без копирования.
            if fs::rename(&legacy, current).is_ok() {
                notes.push(format!("{label}: каталог перенесён целиком"));
                continue;
            }
        }
        let _ = fs::create_dir_all(current);
        // settings.json сливаются отдельно: значения из старого каталога
        // приоритетнее (их всю дорогу писал Python-воркер — это реальные
        // настройки пользователя; в новом каталоге мог появиться только
        // автостаб от прошлых Rust-сборок).
        merge_settings_json(&legacy.join("settings.json"), &current.join("settings.json"), &legacy, current);
        let moved = merge_tree(&legacy, current);
        if moved > 0 {
            notes.push(format!("{label}: перенесено записей — {moved}"));
        }
        retire_legacy_dir(&legacy);
    }
    // Пути в настройках обязаны указывать в новый каталог и после
    // переименования целиком (game_dir хранится абсолютным путём).
    rewrite_settings_path_prefixes(&roots.config_home.join("settings.json"), &roots);
    if notes.is_empty() {
        None
    } else {
        Some(notes.join("; "))
    }
}

/// Слияние settings.json: отсутствующий новый файл просто переносится;
/// при обоих — объекты объединяются, значения legacy выигрывают.
fn merge_settings_json(legacy: &Path, current: &Path, legacy_base: &Path, current_base: &Path) {
    let Some(legacy_value) = read_bounded_json(legacy, MAX_SETTINGS_JSON_BYTES) else { return };
    let Some(mut merged) = legacy_value.as_object().cloned() else {
        // Настройки — не объект (неожиданно): переносим как есть, если нового нет.
        if !current.exists() {
            let _ = fs::rename(legacy, current);
        }
        return;
    };
    if let Some(current_value) = read_bounded_json(current, MAX_SETTINGS_JSON_BYTES) {
        if let Some(object) = current_value.as_object() {
            for (key, value) in object {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    rewrite_object_path_prefixes(&mut merged, legacy_base, current_base);
    let _ = write_json_private(current, &serde_json::Value::Object(merged));
    let _ = fs::remove_file(legacy);
}

/// Перенос содержимого legacy-каталога в текущий без затирания: файлы
/// переименовываются, одноимённые каталоги обходятся рекурсивно.
fn merge_tree(legacy: &Path, current: &Path) -> usize {
    let Ok(entries) = fs::read_dir(legacy) else { return 0 };
    let mut moved = 0;
    let root_level = legacy.parent() == current.parent();
    for entry in entries.filter_map(|item| item.ok()) {
        let source = entry.path();
        if is_symlink(&source) {
            continue;
        }
        let name = source.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // settings.json корневого уровня уже слит — не переносим повторно
        if root_level && name == "settings.json" {
            continue;
        }
        let target = current.join(&name);
        if target.exists() {
            if source.is_dir() && target.is_dir() {
                moved += merge_tree(&source, &target);
            }
            continue;
        }
        if fs::rename(&source, &target).is_ok() {
            moved += 1;
        }
    }
    moved
}

/// Опустевший legacy-каталог убираем; если что-то осталось (конфликт имён) —
/// переименовываем в «trial-launcher.migrated», чтобы не мешал и не тёк
/// повторной миграцией.
fn retire_legacy_dir(legacy: &Path) {
    if dir_is_empty(legacy) {
        let _ = fs::remove_dir_all(legacy);
        return;
    }
    let retired = legacy.with_file_name(format!("{LEGACY_LAUNCHER_DIR}.migrated"));
    if !retired.exists() {
        let _ = fs::rename(legacy, &retired);
    }
}

fn dir_is_empty(path: &Path) -> bool {
    fs::read_dir(path).map(|mut entries| entries.next().is_none()).unwrap_or(false)
}

/// Атомарная запись настроек (tmp + rename) с закрытыми правами.
fn write_json_private(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(value).map_err(|error| format!("Не удалось сериализовать настройки: {error}"))?;
    fs::write(&temporary, bytes).map_err(|error| format!("Не удалось записать настройки: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&temporary, path).map_err(|error| format!("Не удалось заменить настройки: {error}"))
}

/// Абсолютные пути в верхнем уровне настроек, указывающие в старый каталог,
/// переписываются на новый (game_dir и другие ключи-пути).
fn rewrite_settings_path_prefixes(settings_path: &Path, roots: &LauncherRoots) {
    let Some(value) = read_bounded_json(settings_path, MAX_SETTINGS_JSON_BYTES) else { return };
    let Some(object) = value.as_object().cloned() else { return };
    let replacements: Vec<(PathBuf, PathBuf)> = [
        (roots.config_home.parent().map(|parent| parent.join(LEGACY_LAUNCHER_DIR)), roots.config_home.clone().into()),
        (roots.data_home.parent().map(|parent| parent.join(LEGACY_LAUNCHER_DIR)), roots.data_home.clone().into()),
    ]
    .into_iter()
    .filter_map(|(legacy, current)| legacy.zip(current))
    .collect();
    let mut changed = false;
    let mut updated = serde_json::Map::new();
    for (key, value) in object {
        if let Some(text) = value.as_str() {
            let mut text = text.to_owned();
            for (legacy, current) in &replacements {
                let legacy_text = format!("{}", legacy.display());
                if text.starts_with(&legacy_text) {
                    text = format!("{}{}", current.display(), &text[legacy_text.len()..]);
                    changed = true;
                }
            }
            updated.insert(key, serde_json::Value::String(text));
        } else {
            updated.insert(key, value);
        }
    }
    if changed {
        let _ = write_json_private(settings_path, &serde_json::Value::Object(updated));
    }
}

fn rewrite_object_path_prefixes(object: &mut serde_json::Map<String, serde_json::Value>, legacy_base: &Path, current_base: &Path) {
    let legacy = format!("{}", legacy_base.display());
    let current = format!("{}", current_base.display());
    for value in object.values_mut() {
        if let Some(text) = value.as_str() {
            if text.starts_with(&legacy) {
                *value = serde_json::Value::String(format!("{}{}", current, &text[legacy.len()..]));
            }
        }
    }
}


/// Windows берёт известные папки пользователя (%APPDATA%, %LOCALAPPDATA%) —
/// ровно как Python Paths; XDG-переменные работают на обеих ОС. До этого
/// Windows-сборка без HOME проваливалась в /.config и C:\.local: родные
/// операции (оверлей, аккаунты, логи сессий Bedrock) искали настройки и
/// экземпляры в параллельном пустом каталоге. Чистая функция — тестируется
/// с поддельным окружением на любой ОС.
pub(crate) fn launcher_roots_for(windows: bool, env: &dyn Fn(&str) -> Option<String>) -> LauncherRoots {
    if windows {
        let home = home_dir_for(true, env);
        let roaming = env("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let local = env("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        let cache_base = local.join("Cache");
        LauncherRoots {
            config_home: xdg_dir_for(true, env, "XDG_CONFIG_HOME", roaming).join(LAUNCHER_DIR),
            data_home: xdg_dir_for(true, env, "XDG_DATA_HOME", local).join(LAUNCHER_DIR),
            cache_home: xdg_dir_for(true, env, "XDG_CACHE_HOME", cache_base).join(LAUNCHER_DIR),
        }
    } else {
        let home = home_dir_for(false, env);
        LauncherRoots {
            config_home: xdg_dir_for(false, env, "XDG_CONFIG_HOME", home.join(".config")).join(LAUNCHER_DIR),
            data_home: xdg_dir_for(false, env, "XDG_DATA_HOME", home.join(".local").join("share")).join(LAUNCHER_DIR),
            cache_home: xdg_dir_for(false, env, "XDG_CACHE_HOME", home.join(".cache")).join(LAUNCHER_DIR),
        }
    }
}

fn xdg_dir_for(windows: bool, env: &dyn Fn(&str) -> Option<String>, variable: &str, fallback: PathBuf) -> PathBuf {
    if let Some(value) = env(variable) {
        // Unix требует абсолютный путь (защита от мусора в env); Windows
        // принимает любое непустое значение — так же, как _xdg_home в Python.
        let usable = !value.is_empty() && (windows || value.starts_with('/'));
        if usable {
            return PathBuf::from(value);
        }
    }
    fallback
}

/// Каталог игры из settings.json; по умолчанию — data_home/game.
pub(crate) fn settings_game_dir(config_home: &Path, data_home: &Path) -> PathBuf {
    read_bounded_json(&config_home.join("settings.json"), MAX_SETTINGS_JSON_BYTES)
        .and_then(|settings| {
            settings
                .get("game_dir")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| data_home.join("game"))
}

/// Безопасное имя экземпляра: прогоны недопустимых символов → один «-»,
/// обрезка точек/дефисов по краям, «default» для пустого результата,
/// максимум 64 символа.
pub(crate) fn safe_instance_name(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut previous_invalid = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            cleaned.push(character);
            previous_invalid = false;
        } else if !previous_invalid {
            cleaned.push('-');
            previous_invalid = true;
        }
    }
    let trimmed = cleaned.trim_matches(|c| c == '.' || c == '-');
    let base = if trimmed.is_empty() { "default" } else { trimmed };
    base.chars().take(64).collect()
}

/// Поиск экземпляра по имени: валидация, проверка каталога (не symlink,
/// существует) и принадлежности корню экземпляров. None — «не найден».
pub(crate) fn resolve_instance_path(game_dir: &Path, name: &str) -> Option<PathBuf> {
    let instances_root = game_dir.join("instances");
    let path = instances_root.join(safe_instance_name(name));
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.is_symlink() || !metadata.is_dir() {
        return None;
    }
    let resolved = path.canonicalize().ok()?;
    let root = instances_root.canonicalize().ok()?;
    if !resolved.starts_with(&root) {
        return None;
    }
    Some(path)
}

/// Читаем маленький JSON с ограничением размера и без symlink-файлов.
pub(crate) fn read_bounded_json(path: &Path, max_bytes: u64) -> Option<Value> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.is_symlink() || !metadata.is_file() || metadata.len() > max_bytes {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

pub fn overlay_state_from(config_home: &Path, data_home: &Path, now: f64) -> Value {
    let settings = read_bounded_json(&config_home.join("settings.json"), MAX_SETTINGS_JSON_BYTES)
        .unwrap_or_else(|| json!({}));
    let game_dir = settings_game_dir(config_home, data_home);
    let overlay_settings = match settings.get("overlay_settings") {
        Some(value) if value.is_object() => value.clone(),
        _ => default_overlay_settings(),
    };
    let recording = recording_status(&game_dir, now);
    let instances_root = game_dir.join("instances");
    let Some(running) = scan_running_instance(&instances_root, now) else {
        // Ответ «ничего не запущено» не содержит ключа system_metrics.
        return json!({
            "running": false,
            "instance": null,
            "session_seconds": 0,
            "played_seconds": 0,
            "metrics": {},
            "mods": [],
            "recording": recording,
            "overlay": overlay_settings,
        });
    };
    let session_seconds = running.metrics["uptime_seconds"].as_i64().unwrap_or(0);
    let mods = if running.game_edition != "bedrock" {
        overlay_mods_cached(&running.name, &running.path, now)
    } else {
        json!([])
    };
    json!({
        "running": true,
        "instance": {
            "name": running.name,
            "display_name": running.display_name,
            "game_edition": running.game_edition,
            "kind": running.kind,
            "minecraft_version": running.minecraft_version,
            "loader": running.loader,
        },
        "session_seconds": session_seconds,
        "played_seconds": running.played_seconds + session_seconds,
        "metrics": {
            "cpu_percent": running.metrics["cpu_percent"].clone(),
            "ram_bytes": running.metrics["ram_bytes"].clone(),
            "peak_cpu_percent": running.metrics["peak_cpu_percent"].clone(),
            "peak_ram_bytes": running.metrics["peak_ram_bytes"].clone(),
            "storage_bytes": running.metrics["storage_bytes"].clone(),
        },
        "system_metrics": metrics::system_metrics(&json!({})).unwrap_or(json!({})),
        "mods": mods,
        "recording": recording,
        "overlay": overlay_settings,
    })
}

fn default_overlay_settings() -> Value {
    // Значения по умолчанию; копия живёт и на фронте (overlayHelpers.ts) —
    // менять только вместе.
    json!({
        "enabled": true, "hotkey_overlay": "Shift+Tab", "hotkey_recording": "F7",
        "show_mods": true, "show_metrics": true, "recording_fps": 60, "recording_quality": 23,
        "recording_encoder": "auto", "recording_container": "mp4", "recording_audio": true,
        "position": "top-left", "scale": 100, "opacity": 96, "compact": false,
        "show_clock": true, "show_system_metrics": true, "metrics_cpu": true, "metrics_ram": true,
        "metrics_gpu": true, "metrics_disk": true, "metrics_temperatures": true,
        "system_metrics_display": "overlay", "show_chat": true,
    })
}

// ------------------------------------------------------- Экземпляры ----

struct RunningInstance {
    name: String,
    display_name: String,
    kind: String,
    game_edition: String,
    minecraft_version: String,
    loader: String,
    played_seconds: i64,
    path: PathBuf,
    metrics: Value,
}

fn scan_running_instance(instances_root: &Path, now: f64) -> Option<RunningInstance> {
    let entries = fs::read_dir(instances_root).ok()?;
    let mut directories: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !is_symlink(path))
        .collect();
    directories.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
    for path in directories {
        let live = read_bounded_json(&path.join(LIVE_STATE_FILE), MAX_INSTANCE_LIVE_STATE_JSON_BYTES)
            .filter(|value| value.is_object());
        let pid = live
            .as_ref()
            .filter(|state| state.get("running").and_then(Value::as_bool).unwrap_or(false))
            .map(|state| state.get("pid").and_then(Value::as_i64).unwrap_or(0));
        let running = pid.map(|pid| pid > 0 && process_exists(pid)).unwrap_or(false);
        if !running {
            continue;
        }
        let metadata = read_bounded_json(&path.join(INSTANCE_METADATA_FILE), MAX_INSTANCE_PROFILE_JSON_BYTES)
            .unwrap_or_else(|| json!({}));
        let started_at = live
            .as_ref()
            .and_then(|state| state.get("started_at"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let kind = instance_kind(&path, &metadata);
        let game_edition = instance_game_edition(&metadata, &kind);
        let (minecraft_version, loader) = if game_edition == "bedrock" {
            (String::new(), "vanilla".to_owned())
        } else {
            latest_version_info(&path)
        };
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let display_name = metadata
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(80).collect::<String>())
            .unwrap_or_else(|| name.clone());
        return Some(RunningInstance {
            name,
            display_name,
            kind,
            game_edition,
            minecraft_version,
            loader,
            played_seconds: metadata.get("played_seconds").and_then(Value::as_i64).unwrap_or(0).max(0)
                + live_play_seconds(pid.is_some(), started_at, now),
            metrics: instance_metrics(&path, pid.unwrap_or(0), started_at, now),
            path,
        });
    }
    None
}

fn live_play_seconds(running: bool, started_at: f64, now: f64) -> i64 {
    if running && started_at > 0.0 {
        (now - started_at).max(0.0) as i64
    } else {
        0
    }
}

fn instance_kind(path: &Path, metadata: &Value) -> String {
    let is_server = metadata
        .get("kind")
        .and_then(Value::as_str)
        .map(|value| value.trim().eq_ignore_ascii_case("server"))
        .unwrap_or(false)
        || read_bounded_json(&path.join(SERVER_CONFIG_FILE), 1024 * 1024)
            .map(|value| value.is_object())
            .unwrap_or(false);
    if is_server {
        "server".to_owned()
    } else {
        "client".to_owned()
    }
}

/// Метаданные экземпляра (.nivora-launcher-instance.json), ограниченные
/// по размеру; повреждённый файл — пустой объект.
pub(crate) fn instance_metadata(path: &Path) -> Value {
    read_bounded_json(&path.join(INSTANCE_METADATA_FILE), MAX_INSTANCE_PROFILE_JSON_BYTES).unwrap_or(Value::Null)
}

pub(crate) fn instance_kind_of(path: &Path) -> String {
    instance_kind(path, &instance_metadata(path))
}

/// Запущен ли экземпляр: файл живого состояния пишет лаунчер, живость
/// процесса проверяется по pid.
pub(crate) fn instance_running(path: &Path) -> bool {
    let Some(state) = read_bounded_json(&path.join(LIVE_STATE_FILE), MAX_INSTANCE_LIVE_STATE_JSON_BYTES) else {
        return false;
    };
    if state.get("running").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    state
        .get("pid")
        .and_then(Value::as_i64)
        .map(|pid| pid > 0 && process_exists(pid))
        .unwrap_or(false)
}

fn instance_game_edition(metadata: &Value, kind: &str) -> String {
    let edition = metadata
        .get("game_edition")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    if kind == "server" || (edition != "java" && edition != "bedrock") {
        "java".to_owned()
    } else {
        edition
    }
}

/// minecraft_version + loader из самого свежего versions/<id>/<id>.json.
fn latest_version_info(path: &Path) -> (String, String) {
    let versions = path.join("versions");
    if is_symlink(&versions) || !versions.is_dir() {
        return (String::new(), "vanilla".to_owned());
    }
    let Ok(entries) = fs::read_dir(&versions) else {
        return (String::new(), "vanilla".to_owned());
    };
    let mut candidates: Vec<(SystemTime, PathBuf, Value)> = Vec::new();
    for entry in entries.filter_map(|entry| entry.ok()) {
        let version_dir = entry.path();
        if is_symlink(&version_dir) || !version_dir.is_dir() {
            continue;
        }
        let Ok(children) = fs::read_dir(&version_dir) else {
            continue;
        };
        for metadata_file in children.filter_map(|child| child.ok()) {
            let metadata_path = metadata_file.path();
            let file_name = metadata_path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            if !file_name.ends_with(".json") || is_symlink(&metadata_path) || !metadata_path.is_file() {
                continue;
            }
            let Some(value) = read_bounded_json(&metadata_path, MAX_INSTANCE_VERSION_JSON_BYTES) else {
                continue;
            };
            if !value.is_object() {
                continue;
            }
            let modified = metadata_file.metadata().and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            candidates.push((modified, metadata_path, value));
        }
    }
    let Some((_, metadata_path, value)) = candidates.into_iter().max_by_key(|(modified, _, _)| *modified) else {
        return (String::new(), "vanilla".to_owned());
    };
    let stem = metadata_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let id_value = value.get("id").and_then(Value::as_str).unwrap_or("");
    let version_id = if valid_version_id(id_value) { id_value.to_owned() } else { stem };
    if !valid_version_id(&version_id) {
        return (String::new(), "vanilla".to_owned());
    }
    let mut minecraft_version = value
        .get("inheritsFrom")
        .or_else(|| value.get("minecraft"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(80)
        .collect::<String>();
    if minecraft_version.is_empty() {
        minecraft_version = version_from_id(&version_id);
    }
    (minecraft_version, loader_from_version(&version_id))
}

fn valid_version_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id != "."
        && id != ".."
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Версия игры из id: последнее вхождение «1.x»/«2.x», иначе последний
/// численный фрагмент, иначе весь id.
fn version_from_id(version_id: &str) -> String {
    static VERSION_RE: OnceLock<Regex> = OnceLock::new();
    let regex = VERSION_RE.get_or_init(|| Regex::new(r"\d+\.\d+(?:\.\d+)?").unwrap());
    let matches: Vec<&str> = regex.find_iter(version_id).map(|m| m.as_str()).collect();
    if matches.is_empty() {
        return version_id.to_owned();
    }
    // Перебор с конца: нужен последний фрагмент, предпочтительно 1.x/2.x.
    let preferred: Vec<&&str> = matches.iter().rev().filter(|m| m.starts_with("1.") || m.starts_with("2.")).collect();
    let chosen: &str = preferred.first().copied().copied().unwrap_or(*matches.last().unwrap());
    chosen.to_owned()
}

fn loader_from_version(version_id: &str) -> String {
    let lowered = version_id.to_lowercase();
    for name in ["neoforge", "forge", "quilt", "fabric"] {
        if lowered.contains(name) {
            return name.to_owned();
        }
    }
    "vanilla".to_owned()
}

// -------------------------------------------------- Метрики процесса ----

fn instance_metrics(root: &Path, pid: i64, started_at: f64, now: f64) -> Value {
    let sample = if pid > 0 { process_sample(pid) } else { None };
    let ram_bytes = sample.as_ref().map(|(_, ram)| *ram).unwrap_or(0);
    let uptime_seconds = if started_at > 0.0 { (now - started_at).max(0.0) as i64 } else { 0 };
    json!({
        "timestamp": now,
        "pid": if sample.is_some() { pid } else { 0 },
        "running": sample.is_some(),
        "uptime_seconds": uptime_seconds,
        "peak_ram_bytes": ram_bytes,
        "peak_cpu_percent": 0.0,
        "ram_bytes": ram_bytes,
        "cpu_percent": 0.0,
        "storage_bytes": directory_size(root),
        "players": 0,
        "max_players": 0,
        "tps": 0.0,
    })
}

/// Сумма размеров файлов каталога (без symlink), исключая служебные
/// подкаталоги; не больше 20 000 файлов.
fn directory_size(root: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut count: usize = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            if count >= MAX_MEASURED_FILES {
                return total;
            }
            let path = entry.path();
            if is_symlink(&path) {
                continue;
            }
            if path
                .components()
                .any(|component| EXCLUDED_PARTS.contains(&component.as_os_str().to_string_lossy().as_ref()))
            {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                    count += 1;
                }
            }
        }
    }
    total
}

fn process_exists(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(windows)]
    {
        // /proc нет: живость процесса проверяем OpenProcess'ом с минимальными
        // правами; OpenProcess падает и на доступ, и на мёртвый PID — оба
        // случая для наблюдателя эквивалентны «процесса нет».
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Сумма CPU/RAM процесса и всех его потомков.
#[cfg(unix)]
fn process_sample(pid: i64) -> Option<(f64, u64)> {    let proc_root = Path::new("/proc");
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    if ticks <= 0.0 {
        return None;
    }
    let mut pending = vec![pid];
    let mut visited: u32 = 0;
    let mut cpu_seconds_total = 0.0;
    let mut ram_bytes_total: u64 = 0;
    let mut root_found = false;
    while let Some(current) = pending.pop() {
        visited += 1;
        if visited > 4096 {
            break;
        }
        let proc = proc_root.join(current.to_string());
        let sample = single_process_sample(&proc, ticks);
        if current == pid {
            if sample.is_none() {
                return None;
            }
            root_found = true;
        }
        if let Some((cpu, ram)) = sample {
            cpu_seconds_total += cpu;
            ram_bytes_total += ram;
        }
        let children_path = proc.join("task").join(current.to_string()).join("children");
        if let Ok(text) = fs::read_to_string(&children_path) {
            for value in text.split_whitespace() {
                if let Ok(child) = value.parse::<i64>() {
                    pending.push(child);
                }
            }
        }
    }
    if root_found {
        Some((cpu_seconds_total, ram_bytes_total))
    } else {
        None
    }
}

#[cfg(unix)]
fn single_process_sample(proc: &Path, ticks: f64) -> Option<(f64, u64)> {
    let stat_line = fs::read_to_string(proc.join("stat")).ok()?;
    // comm может содержать пробелы и скобки: поля — после последней «)».
    let closing = stat_line.rfind(')')?;
    let fields: Vec<&str> = stat_line[closing + 2..].split_whitespace().collect();
    if fields.len() < 13 {
        return None;
    }
    let utime: f64 = fields[11].parse().ok()?;
    let stime: f64 = fields[12].parse().ok()?;
    let cpu_seconds = (utime + stime) / ticks.max(1.0);
    let status = fs::read_to_string(proc.join("status")).unwrap_or_default();
    let mut rss_kib: u64 = 0;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss_kib = rest.split_whitespace().next().and_then(|value| value.parse().ok()).unwrap_or(0);
            break;
        }
    }
    Some((cpu_seconds, rss_kib.saturating_mul(1024)))
}

// /proc нет: RAM берём из GetProcessMemoryInfo (WorkingSet); CPU-доля в
// метриках экземпляра и так всегда отдаётся нулём, а перечисление потомков
// Java на Windows дороже самой метрики — берём один процесс.
#[cfg(windows)]
fn process_sample(pid: i64) -> Option<(f64, u64)> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if handle.is_null() {
            return None;
        }
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        let ok = GetProcessMemoryInfo(handle, &mut counters, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some((0.0, counters.WorkingSetSize as u64))
    }
}

#[cfg(not(any(unix, windows)))]
fn process_sample(_pid: i64) -> Option<(f64, u64)> {
    None
}

// ------------------------------------------------------------- Моды ----

fn overlay_mods_cached(name: &str, instance_root: &Path, now: f64) -> Value {
    {
        let cache = OVERLAY_MODS_CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(caches) = cache.as_ref() {
            if let Some((at, mods)) = caches.get(name) {
                if now - at < OVERLAY_MODS_CACHE_SECONDS {
                    return mods.clone();
                }
            }
        }
    }
    let mods = list_mods(&instance_root.join("mods"));
    let mut cache = OVERLAY_MODS_CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    cache
        .get_or_insert_with(HashMap::new)
        .insert(name.to_owned(), (now, mods.clone()));
    mods
}

/// Список модов для виджета оверлея: имя, красивое имя, включённость.
/// Порядок — обход всего дерева, сортировка по пути в нижнем регистре.
fn list_mods(directory: &Path) -> Value {
    if is_symlink(directory) || !directory.is_dir() {
        return json!([]);
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_files_sorted(directory, &mut paths, 0);
    let mods: Vec<Value> = paths
        .iter()
        .filter_map(|path| {
            let file_name = path.file_name()?.to_string_lossy().to_string();
            // .disabled-файлы остаются в списке с enabled=false —
            // так их показывает оверлей.
            if file_name.ends_with(".part")
                || file_name.ends_with(".trial-meta")
                || file_name.ends_with(PROJECT_METADATA_SUFFIX)
            {
                return None;
            }
            Some(json!({
                "name": file_name,
                "display_name": mod_display_name(path, &file_name),
                "enabled": !file_name.ends_with(".disabled"),
            }))
        })
        .take(OVERLAY_MODS_LIMIT)
        .collect();
    Value::Array(mods)
}

fn collect_files_sorted(directory: &Path, output: &mut Vec<PathBuf>, depth: usize) {
    if depth > 16 || output.len() >= 5000 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect();
    children.sort_by_key(|path| path.to_string_lossy().to_lowercase());
    for child in children {
        if is_symlink(&child) {
            continue;
        }
        if child.is_dir() {
            collect_files_sorted(&child, output, depth + 1);
        } else if child.is_file() {
            output.push(child);
        }
    }
}

fn mod_display_name(path: &Path, file_name: &str) -> String {
    let sidecar = path.with_file_name(format!(".{file_name}{PROJECT_METADATA_SUFFIX}"));
    if let Some(metadata) = read_bounded_json(&sidecar, 256 * 1024) {
        let value = metadata
            .get("display_name")
            .or_else(|| metadata.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(160)
            .collect::<String>();
        if !value.is_empty() {
            return value;
        }
    }
    pretty_asset_name(file_name)
}

/// Красивое имя ассета: расширение и хвостовой номер версии прочь,
/// дефисы/подчёркивания → пробелы, camel-split («J E I»). Разделение
/// регистров вручную: «пробел перед каждой заглавной, кроме первой
/// позиции».
fn pretty_asset_name(file_name: &str) -> String {
    static TRAILING_VERSION: OnceLock<Regex> = OnceLock::new();
    static SEPARATORS: OnceLock<Regex> = OnceLock::new();
    let version_re = TRAILING_VERSION.get_or_init(|| Regex::new(r"(?i)[-_ ]v?\d+(?:\.\d+)+(?:[-+][\w.-]+)?$").unwrap());
    let separators_re = SEPARATORS.get_or_init(|| Regex::new(r"[-_]+").unwrap());

    let mut value = file_name.to_owned();
    if let Some(stripped) = value.strip_suffix(".disabled") {
        value = stripped.to_owned();
    }
    for suffix in [".jar", ".zip"] {
        if value.to_lowercase().ends_with(suffix) {
            value = value[..value.len() - suffix.len()].to_owned();
            break;
        }
    }
    let value = version_re.replace(&value, "").to_string();
    let mut camel = String::with_capacity(value.len() + 4);
    for (index, character) in value.chars().enumerate() {
        if index != 0 && character.is_ascii_uppercase() {
            camel.push(' ');
        }
        camel.push(character);
    }
    let value = separators_re.replace_all(&camel, " ").to_string();
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "Неизвестный ресурс".to_owned()
    } else {
        collapsed
    }
}

// ---------------------------------------------------------- Запись ----

fn recording_status(game_root: &Path, now: f64) -> Value {
    let state_path = game_root.join("recordings").join(RECORDING_STATE_FILE);
    let Some(state) = read_bounded_json(&state_path, 64 * 1024).filter(|value| value.is_object()) else {
        return json!({ "active": false, "file": "", "size_bytes": 0, "started_at": 0.0, "elapsed_seconds": 0, "encoder": "", "instance": "" });
    };
    let pid = state.get("pid").and_then(Value::as_i64).unwrap_or(0);
    let active = pid > 0 && process_exists(pid);
    let file = state.get("file").and_then(Value::as_str).unwrap_or("");
    let size_bytes = if !file.is_empty() {
        fs::metadata(file).map(|meta| meta.len()).unwrap_or(0)
    } else {
        0
    };
    let started_at = state.get("started_at").and_then(Value::as_f64).unwrap_or(0.0);
    json!({
        "active": active,
        "file": file,
        "size_bytes": size_bytes,
        "started_at": started_at,
        "elapsed_seconds": if active { (now - started_at).max(0.0) as i64 } else { 0 },
        "encoder": state.get("encoder").and_then(Value::as_str).unwrap_or(""),
        "instance": state.get("instance").and_then(Value::as_str).unwrap_or(""),
    })
}

// ----------------------------------------------------------- Тесты ----

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn legacy_roots_migration_moves_trial_world_into_nivora() {
        let root = fixture_root("legacy");
        let config = root.join("config");
        let data = root.join("data");
        // Мир пользователя целиком в старом «trial-launcher»: настройки с
        // абсолютным game_dir по умолчанию + экземпляры внутри data-каталога.
        let legacy_config = config.join(LEGACY_LAUNCHER_DIR);
        let legacy_data = data.join(LEGACY_LAUNCHER_DIR);
        fs::create_dir_all(&legacy_config).unwrap();
        fs::create_dir_all(legacy_data.join("game").join("instances").join("main-world")).unwrap();
        fs::write(legacy_data.join("accounts.json"), "{}").unwrap();
        let legacy_game_dir = legacy_data.join("game");
        fs::write(
            legacy_config.join("settings.json"),
            format!("{{\"game_dir\": {}, \"memory\": \"4096 MiB\"}}", serde_json::to_string(legacy_game_dir.to_string_lossy().as_ref()).unwrap()),
        )
        .unwrap();
        // «Новый» каталог уже существует — времена смешанных сборок: там
        // остался автостаб настроек и осиротевший Rust-экземпляр.
        let current_config = config.join(LAUNCHER_DIR);
        let current_data = data.join(LAUNCHER_DIR);
        fs::create_dir_all(&current_config).unwrap();
        fs::create_dir_all(current_data.join("game").join("instances").join("rust-orphan")).unwrap();
        fs::write(
            current_config.join("settings.json"),
            "{\"game_dir\": \"\", \"memory\": \"2048 MiB\"}",
        )
        .unwrap();

        let old_config = std::env::var("XDG_CONFIG_HOME").ok();
        let old_data = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_DATA_HOME", &data);
        let note = migrate_legacy_roots();
        match old_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_data {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }

        assert!(note.is_some(), "миграция обязана отчитаться о переносе");
        // Экземпляры из старого мира переехали, Rust-осиротевший не тронут
        assert!(current_data.join("game").join("instances").join("main-world").is_dir());
        assert!(current_data.join("game").join("instances").join("rust-orphan").is_dir());
        assert!(current_data.join("accounts.json").exists(), "данные (аккаунты) переехали");
        // Настройки слиты: пользовательские значения выиграли, путь переписан
        let merged: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(current_config.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(merged["memory"], serde_json::json!("4096 MiB"), "значение пользователя приоритетнее автостаба");
        assert_eq!(
            merged["game_dir"].as_str().map(str::to_owned),
            Some(current_data.join("game").to_string_lossy().into_owned()),
            "game_dir переписан на новый каталог"
        );
        assert!(!legacy_config.exists(), "старый каталог настроек убран");
        assert!(!legacy_data.exists(), "старый каталог данных убран");
        // Повторный запуск — no-op
        let old_config = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        assert_eq!(migrate_legacy_roots(), None);
        match old_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_roots_wholesale_rename_when_nivora_absent() {
        let root = fixture_root("legacy-rename");
        let config = root.join("config");
        let data = root.join("data");
        // Только старый мир — новый каталог не создавался вовсе
        let legacy_config = config.join(LEGACY_LAUNCHER_DIR);
        fs::create_dir_all(legacy_config.join("game").join("instances").join("solo")).unwrap();
        fs::write(legacy_config.join("settings.json"), "{}").unwrap();
        let old_config = std::env::var("XDG_CONFIG_HOME").ok();
        let old_data = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_DATA_HOME", &data);
        let note = migrate_legacy_roots();
        match old_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_data {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        assert!(note.is_some());
        assert!(config.join(LAUNCHER_DIR).join("game").join("instances").join("solo").is_dir(), "каталог переименован целиком");
        assert!(!legacy_config.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn launcher_roots_on_windows_use_known_folders() {
        // На Windows HOME нет: корни обязаны идти в %APPDATA%/%LOCALAPPDATA%,
        // а не в /.config у корня диска — как это было до исправления.
        // Каталог = trial-launcher: APP_ID Python, данные обязаны совпадать.
        let roots = launcher_roots_for(true, &|key| match key {
            "APPDATA" => Some("C:/Users/Player/AppData/Roaming".to_owned()),
            "LOCALAPPDATA" => Some("C:/Users/Player/AppData/Local".to_owned()),
            _ => None,
        });
        assert_eq!(
            roots.config_home,
            PathBuf::from("C:/Users/Player/AppData/Roaming").join(LAUNCHER_DIR)
        );
        assert_eq!(
            roots.data_home,
            PathBuf::from("C:/Users/Player/AppData/Local").join(LAUNCHER_DIR)
        );
        assert_eq!(
            roots.cache_home,
            PathBuf::from("C:/Users/Player/AppData/Local").join("Cache").join(LAUNCHER_DIR)
        );
    }

    #[test]
    fn launcher_roots_on_windows_derive_from_userprofile_without_appdata() {
        let roots = launcher_roots_for(true, &|key| match key {
            "USERPROFILE" => Some("C:/Users/Player".to_owned()),
            _ => None,
        });
        assert_eq!(
            roots.config_home,
            PathBuf::from("C:/Users/Player").join("AppData").join("Roaming").join(LAUNCHER_DIR)
        );
        assert_eq!(
            roots.data_home,
            PathBuf::from("C:/Users/Player").join("AppData").join("Local").join(LAUNCHER_DIR)
        );
    }

    #[test]
    fn launcher_roots_on_windows_honor_xdg_overrides_like_python() {
        // Python Paths уважает XDG_* и на Windows — движок должен видеть те же
        // каталоги, иначе настройки и экземпляры расходятся между воркером и
        // родными операциями.
        let roots = launcher_roots_for(true, &|key| match key {
            "APPDATA" => Some("C:/Roam".to_owned()),
            "LOCALAPPDATA" => Some("C:/Local".to_owned()),
            "XDG_CONFIG_HOME" => Some("D:/Portable/config".to_owned()),
            "XDG_DATA_HOME" => Some("D:/Portable/data".to_owned()),
            _ => None,
        });
        assert_eq!(roots.config_home, PathBuf::from("D:/Portable/config").join(LAUNCHER_DIR));
        assert_eq!(roots.data_home, PathBuf::from("D:/Portable/data").join(LAUNCHER_DIR));
        assert_eq!(roots.cache_home, PathBuf::from("C:/Local").join("Cache").join(LAUNCHER_DIR));
    }

    #[test]
    fn launcher_roots_on_unix_keep_absolute_xdg_filter() {
        let roots = launcher_roots_for(false, &|key| match key {
            "HOME" => Some("/home/gamer".to_owned()),
            "XDG_CONFIG_HOME" => Some("relative/junk".to_owned()),
            "XDG_DATA_HOME" => Some("/xdg/data".to_owned()),
            _ => None,
        });
        assert_eq!(roots.config_home, PathBuf::from("/home/gamer/.config").join(LAUNCHER_DIR));
        assert_eq!(roots.data_home, PathBuf::from("/xdg/data").join(LAUNCHER_DIR));
        assert_eq!(roots.cache_home, PathBuf::from("/home/gamer/.cache").join(LAUNCHER_DIR));
    }

    fn fixture_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn idle_state_shape() {
        let root = fixture_root("idle");
        let game = root.join("game");
        fs::create_dir_all(game.join("instances").join("Empty")).unwrap();
        let response = overlay_state_from(&root.join("config"), &root.join("data"), 1000.0);
        // Ключа system_metrics в «пустом» ответе нет.
        assert_eq!(response["running"], serde_json::json!(false));
        assert_eq!(response["instance"], serde_json::json!(null));
        assert_eq!(response["metrics"], serde_json::json!({}));
        assert!(response.get("system_metrics").is_none());
        assert_eq!(response["overlay"]["hotkey_overlay"], serde_json::json!("Shift+Tab"));
        assert_eq!(response["recording"]["active"], serde_json::json!(false));
    }

    #[test]
    fn running_instance_assembles_full_state() {
        let root = fixture_root("running");
        let config = root.join("config");
        let game = root.join("game");
        let instance = game.join("instances").join("Fabric Test");
        fs::create_dir_all(instance.join("versions").join("1.20.4-fabric0.92.2")).unwrap();
        write_json(
            &config.join("settings.json"),
            json!({ "game_dir": game.to_string_lossy(), "overlay_settings": { "hotkey_overlay": "Ctrl+O", "enabled": false } }),
        );
        write_json(
            &instance.join(INSTANCE_METADATA_FILE),
            json!({ "display_name": "Мой мир", "played_seconds": 100, "kind": "client", "game_edition": "java" }),
        );
        write_json(
            &instance.join("versions").join("1.20.4-fabric0.92.2").join("1.20.4-fabric0.92.2.json"),
            json!({ "id": "1.20.4-fabric0.92.2", "inheritsFrom": "1.20.4" }),
        );
        // Живой процесс: собственный pid тестового раннера.
        let pid = std::process::id() as i64;
        write_json(
            &instance.join(LIVE_STATE_FILE),
            json!({ "running": true, "pid": pid, "started_at": 990.0 }),
        );
        fs::create_dir_all(instance.join("mods")).unwrap();
        fs::write(instance.join("mods").join("sodium-1.20.4.jar"), b"x").unwrap();
        fs::write(instance.join("mods").join("jei.disabled"), b"x").unwrap();

        let response = overlay_state_from(&config, &root.join("data"), 1000.0);
        assert_eq!(response["running"], serde_json::json!(true));
        assert_eq!(response["instance"]["name"], serde_json::json!("Fabric Test"));
        assert_eq!(response["instance"]["display_name"], serde_json::json!("Мой мир"));
        assert_eq!(response["instance"]["minecraft_version"], serde_json::json!("1.20.4"));
        assert_eq!(response["instance"]["loader"], serde_json::json!("fabric"));
        // uptime = now - started_at; played = metadata(100) + live(10) + session(10)
        assert_eq!(response["session_seconds"], serde_json::json!(10));
        assert_eq!(response["played_seconds"], serde_json::json!(120));
        // Контракт оверлея: в metrics ровно 5 полей; pid/running наружу
        // не отдаются.
        assert_eq!(response["metrics"]["cpu_percent"], serde_json::json!(0.0));
        assert!(response["metrics"]["pid"].is_null());
        assert!(response["metrics"]["storage_bytes"].as_u64().unwrap_or(0) > 0);
        // Настройки оверлея приходят из settings.json как есть.
        assert_eq!(response["overlay"]["hotkey_overlay"], serde_json::json!("Ctrl+O"));
        assert_eq!(response["overlay"]["enabled"], serde_json::json!(false));
        // Моды отсортированы по полному пути в нижнем регистре:
        // jei.disabled < sodium-1.20.4.jar.
        assert_eq!(response["mods"].as_array().map(Vec::len), Some(2));
        let jei = &response["mods"][0];
        assert_eq!(jei["name"], serde_json::json!("jei.disabled"));
        assert_eq!(jei["enabled"], serde_json::json!(false));
        assert_eq!(jei["display_name"], serde_json::json!("jei"));
        let sodium = &response["mods"][1];
        assert_eq!(sodium["name"], serde_json::json!("sodium-1.20.4.jar"));
        assert_eq!(sodium["enabled"], serde_json::json!(true));
        assert_eq!(sodium["display_name"], serde_json::json!("sodium"));
    }

    #[test]
    fn dead_pid_is_not_running() {
        let root = fixture_root("dead");
        let game = root.join("game");
        let instance = game.join("instances").join("Ghost");
        fs::create_dir_all(&instance).unwrap();
        write_json(
            &instance.join(LIVE_STATE_FILE),
            json!({ "running": true, "pid": 4_000_000, "started_at": 1.0 }),
        );
        let response = overlay_state_from(&root.join("config"), &root.join("data"), 100.0);
        assert_eq!(response["running"], serde_json::json!(false));
    }

    #[test]
    fn recording_state_file_drives_status() {
        let root = fixture_root("rec");
        let game = root.join("game");
        let recordings = game.join("recordings");
        fs::create_dir_all(&recordings).unwrap();
        // Файл состояния пишет рекордер; pid живого процесса — наш.
        write_json(
            &recordings.join(RECORDING_STATE_FILE),
            json!({ "pid": std::process::id(), "file": "", "started_at": 500.0, "encoder": "h264_nvenc", "instance": "Test" }),
        );
        let status = recording_status(&game, 510.0);
        assert_eq!(status["active"], serde_json::json!(true));
        assert_eq!(status["elapsed_seconds"], serde_json::json!(10));
        assert_eq!(status["encoder"], serde_json::json!("h264_nvenc"));
        // Мёртвый pid — активность гаснет сама (ffmpeg крешнулся).
        write_json(
            &recordings.join(RECORDING_STATE_FILE),
            json!({ "pid": 4_000_000, "file": "", "started_at": 500.0, "encoder": "", "instance": "" }),
        );
        assert_eq!(recording_status(&game, 510.0)["active"], serde_json::json!(false));
    }

    #[test]
    fn pretty_names_match_reference() {
        assert_eq!(pretty_asset_name("sodium-1.20.4.jar"), "sodium");
        assert_eq!(pretty_asset_name("JEI-4.22.jar"), "J E I");
        assert_eq!(pretty_asset_name("my_coolest_mod.jar"), "my coolest mod");
        assert_eq!(pretty_asset_name("DistantHorizons-2.2.1-a-1.20.4.jar"), "Distant Horizons");
        assert_eq!(pretty_asset_name("Indium-1.0.34+mc1.20.4.jar"), "Indium");
        assert_eq!(pretty_asset_name(".disabled"), "Неизвестный ресурс");
    }

    #[test]
    fn version_detection_and_loaders() {
        assert_eq!(version_from_id("1.20.4-fabric0.92.2"), "1.20.4");
        assert_eq!(version_from_id("neoforge-21.0.143"), "21.0.143");
        assert_eq!(version_from_id("custom"), "custom");
        assert_eq!(version_from_id("1.8.9-forge1.8.9"), "1.8.9");
        assert_eq!(loader_from_version("Neoforge 21"), "neoforge");
        assert_eq!(loader_from_version("quilted-fabric"), "quilt");
        assert_eq!(loader_from_version("vanilla-1.21"), "vanilla");
        assert!(valid_version_id("1.20.4+build"));
        assert!(!valid_version_id("../evil"));
        assert!(!valid_version_id("a/b"));
    }

    #[test]
    fn directory_size_respects_exclusions() {
        let root = fixture_root("size");
        fs::write(root.join("world.dat"), vec![0u8; 100]).unwrap();
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs").join("huge.log"), vec![0u8; 5000]).unwrap();
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("real").join("mod.jar"), vec![0u8; 50]).unwrap();
        assert_eq!(directory_size(&root), 150);
    }

    /// Сверка с эталонной реализацией на общем fixture: скрипт
    /// scripts/parity_overlay_state.sh подставляет пути через переменные
    /// окружения и сравнивает ответы. Без них тест пропускается.
    #[test]
    fn live_parity_fixture() {
        let (Ok(config), Ok(data), Ok(output)) = (
            std::env::var("NIVORA_PARITY_CONFIG"),
            std::env::var("NIVORA_PARITY_DATA"),
            std::env::var("NIVORA_PARITY_OUT"),
        ) else {
            return;
        };
        let value = overlay_state_from(Path::new(&config), Path::new(&data), epoch_seconds());
        fs::write(&output, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }
}
