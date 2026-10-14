// Операции над локальными данными без сети:
//   jvm_presets_list — каталог JVM-пресетов;
//   launch_history   — история запусков экземпляра (только чтение;
//                      запись делает процесс запуска);
//   cache_stats      — сводка каталога загрузок (размер/число файлов);
//   cache_prune      — удаление устаревших метаданных кеша (*.json/*.cache).
//
// Корни — XDG-переменные либо домашний каталог; корень игры — из
// settings.json, кеш — cache_home/downloads.
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::state::{launcher_roots, resolve_instance_path, settings_game_dir};

const LAUNCH_HISTORY_FILE: &str = ".nivora-launcher-launch-history.json";
const MAX_LAUNCH_HISTORY_JSON_BYTES: u64 = 256 * 1024;

pub fn jvm_presets_list(_request: &Value) -> Result<Value, String> {
    Ok(json!({ "presets": jvm_presets() }))
}

pub fn launch_history(request: &Value) -> Result<Value, String> {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if name.is_empty() {
        return Ok(json!({ "history": [] }));
    }
    let roots = launcher_roots();
    let game_dir = settings_game_dir(&roots.config_home, &roots.data_home);
    Ok(launch_history_from(&game_dir, name))
}

pub(crate) fn launch_history_from(game_dir: &Path, name: &str) -> Value {
    // «Экземпляр не найден» — это пустая история, а не ошибка.
    let Some(instance_path) = resolve_instance_path(game_dir, name) else {
        return json!({ "history": [] });
    };
    json!({ "history": read_launch_history(&instance_path) })
}

pub fn cache_stats(_request: &Value) -> Result<Value, String> {
    let roots = launcher_roots();
    Ok(cache_stats_from(&roots.cache_home.join("downloads")))
}

pub(crate) fn cache_stats_from(cache_dir: &Path) -> Value {
    let mut total_bytes: u64 = 0;
    let mut file_count: u64 = 0;
    let mut pending = vec![cache_dir.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let is_symlink = fs::symlink_metadata(&path).map(|meta| meta.is_symlink()).unwrap_or(true);
            if is_symlink {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total_bytes += meta.len();
                    file_count += 1;
                }
            }
        }
    }
    json!({
        "cache": {
            "total_bytes": total_bytes,
            "total_mib": round_mib_one_decimal(total_bytes),
            "file_count": file_count,
        }
    })
}

pub fn cache_prune(request: &Value) -> Result<Value, String> {
    let max_age_hours = request_age_hours(request)?;
    let roots = launcher_roots();
    Ok(cache_prune_from(&roots.cache_home.join("downloads"), max_age_hours))
}

pub(crate) fn cache_prune_from(cache_dir: &Path, max_age_hours: i64) -> Value {
    let now = epoch_seconds();
    let cutoff = now - max_age_hours as f64 * 3600.0;
    let mut deleted: u64 = 0;
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return json!({ "deleted_files": 0 });
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let extension = path
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy()))
            .unwrap_or_default();
        if extension != ".json" && extension != ".cache" {
            continue;
        }
        let is_symlink = fs::symlink_metadata(&path).map(|meta| meta.is_symlink()).unwrap_or(true);
        if is_symlink || !path.is_file() {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let modified_seconds = modified
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs_f64())
            .unwrap_or(0.0);
        if modified_seconds < cutoff && fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    json!({ "deleted_files": deleted })
}

fn request_age_hours(request: &Value) -> Result<i64, String> {
    // Ключ опционален: по умолчанию 48; ошибка разбора падает наружу.
    let default_value = json!(48);
    let raw = request.get("max_age_hours").unwrap_or(&default_value);
    if let Some(value) = raw.as_i64() {
        return Ok(value);
    }
    if let Some(value) = raw.as_f64() {
        return Ok(value.trunc() as i64);
    }
    if let Some(value) = raw.as_str().map(str::trim).and_then(|value| value.parse::<i64>().ok()) {
        return Ok(value);
    }
    Err("max_age_hours должен быть целым числом".to_owned())
}

fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

/// Округление МиБ до одного знака, «половина к чётному»:
/// 262144 байта (ровно 0,25 МиБ) → 0.2, а не 0.3.
fn round_mib_one_decimal(total_bytes: u64) -> f64 {
    let scaled = total_bytes as f64 * 10.0 / (1024.0 * 1024.0);
    let floor = scaled.floor();
    let rounded = match scaled - floor {
        difference if difference == 0.5 => {
            let lower = floor as i64;
            // Соседи отличаются на 1 — чётный выбирается точно.
            if lower % 2 == 0 {
                lower as f64
            } else {
                lower as f64 + 1.0
            }
        }
        difference if difference > 0.5 => floor + 1.0,
        _ => floor,
    };
    rounded / 10.0
}

/// История запусков: файл не-ссылка, ≤256 КБ, JSON-массив;
/// из него берутся только объекты (строки/числа отбрасываются).
fn read_launch_history(instance_dir: &Path) -> Value {
    let path = instance_dir.join(LAUNCH_HISTORY_FILE);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return json!([]);
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_LAUNCH_HISTORY_JSON_BYTES {
        return json!([]);
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return json!([]);
    };
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&text) else {
        return json!([]);
    };
    Value::Array(entries.into_iter().filter(Value::is_object).collect())
}

/// Пресеты продублированы в optimizer.py (optimization_analyze/apply);
/// порядок значим; менять только вместе.
fn jvm_presets() -> Value {
    let fps_flags = [
        "-XX:+UseG1GC",
        "-XX:+UnlockExperimentalVMOptions",
        "-XX:G1NewSizePercent=20",
        "-XX:G1ReservePercent=20",
        "-XX:MaxGCPauseMillis=50",
        "-XX:G1HeapRegionSize=32M",
        "-XX:+ParallelRefProcEnabled",
        "-XX:+AlwaysPreTouch",
        "-XX:+DisableExplicitGC",
        "-Dfml.earlyprogresswindow=false",
    ];
    let balanced_flags = [
        "-XX:+UseG1GC",
        "-XX:+UnlockExperimentalVMOptions",
        "-XX:G1NewSizePercent=20",
        "-XX:G1ReservePercent=20",
        "-XX:MaxGCPauseMillis=100",
        "-XX:G1HeapRegionSize=32M",
        "-XX:+ParallelRefProcEnabled",
        "-XX:+AlwaysPreTouch",
        "-XX:+DisableExplicitGC",
    ];
    let aikars_flags = [
        "-XX:+UseG1GC",
        "-XX:+UnlockExperimentalVMOptions",
        "-XX:G1NewSizePercent=30",
        "-XX:G1MaxNewSizePercent=40",
        "-XX:G1HeapRegionSize=8M",
        "-XX:G1ReservePercent=20",
        "-XX:G1HeapWastePercent=5",
        "-XX:G1MixedGCCountTarget=4",
        "-XX:InitiatingHeapOccupancyPercent=15",
        "-XX:G1MixedGCLiveThresholdPercent=90",
        "-XX:G1RSetUpdatingPauseTimePercent=5",
        "-XX:SurvivorRatio=32",
        "-XX:+PerfDisableSharedMem",
        "-XX:MaxTenuringThreshold=1",
        "-Dusing.aikars.flags=https://mcflags.emc.gs",
        "-Daikars.new.flags=true",
    ];
    let compat_flags = ["-XX:+UseSerialGC", "-Dfml.earlyprogresswindow=false"];
    let streaming_flags = [
        "-XX:+UseG1GC",
        "-XX:+UnlockExperimentalVMOptions",
        "-XX:G1NewSizePercent=20",
        "-XX:G1ReservePercent=20",
        "-XX:MaxGCPauseMillis=100",
        "-XX:G1HeapRegionSize=32M",
        "-XX:+ParallelRefProcEnabled",
        "-XX:+AlwaysPreTouch",
        "-XX:+DisableExplicitGC",
        "-XX:+PerfDisableSharedMem",
    ];
    let preset = |id: &str, name: &str, description: &str, flags: &[&str], recommended: &[&str]| {
        json!({
            "id": id,
            "name": name,
            "description": description,
            "flags": flags,
            "recommended_for": recommended,
        })
    };
    json!([
        preset("fps", "FPS (низкие паузы GC)", "Минимальные паузы сборщика мусора. Лучший FPS для ванильных и лёгких сборок.", &fps_flags, &["vanilla", "light"]),
        preset("balanced", "Balanced", "Хороший баланс FPS и стабильности. Универсальный вариант для большинства сборок.", &balanced_flags, &["vanilla", "light", "medium"]),
        preset("heavy_modpack", "Heavy Modpack (Aikar's)", "Флаги Aikar — проверенный стандарт для тяжёлых сборок с сотнями модов.", &aikars_flags, &["medium", "heavy"]),
        preset("compatibility", "Compatibility", "Только SerialGC, без эксперименталок. Для старых версий и проблемных окружений.", &compat_flags, &["legacy"]),
        preset("streaming", "Streaming", "Равномерный фреймтайм. Снижает дропы при стриминге/записи.", &streaming_flags, &["light", "medium"]),
        preset("none", "Без пресета", "JVM-аргументы из профиля не изменяются.", &[], &[]),
    ])
}

// ----------------------------------------------------------- Тесты ----

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rust_engine::state::safe_instance_name;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-ops-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn jvm_presets_catalog_anchors() {
        let presets = jvm_presets();
        let ids: Vec<&str> = presets.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["fps", "balanced", "heavy_modpack", "compatibility", "streaming", "none"]);
        // Количество флагов и якорные значения, которые видит пользователь.
        let counts: Vec<usize> = presets.as_array().unwrap().iter().map(|p| p["flags"].as_array().unwrap().len()).collect();
        assert_eq!(counts, vec![10, 9, 16, 2, 10, 0]);
        assert_eq!(presets[0]["flags"][5], json!("-XX:G1HeapRegionSize=32M"));
        assert_eq!(presets[2]["flags"][14], json!("-Dusing.aikars.flags=https://mcflags.emc.gs"));
        assert_eq!(presets[3]["flags"][0], json!("-XX:+UseSerialGC"));
        let response = jvm_presets_list(&json!({})).unwrap();
        assert_eq!(response["presets"].as_array().map(Vec::len), Some(6));
    }

    #[test]
    fn launch_history_reads_only_object_entries() {
        let root = fixture_root("history");
        let game = root.join("game");
        // Каталоги экземпляров всегда хранятся в безопасной форме имени
        // («My World» → «My-World»), запрос же может приходить «человеческий».
        let instance = game.join("instances").join("My-World");
        fs::create_dir_all(&instance).unwrap();
        write_json(
            &instance.join(LAUNCH_HISTORY_FILE),
            json!([
                { "launch_id": "a", "exit_code": 0 },
                "повреждённая строка",
                42,
                { "launch_id": "b", "exit_code": null }
            ]),
        );
        let response = launch_history_from(&game, "My World");
        assert_eq!(response["history"].as_array().map(Vec::len), Some(2));
        assert_eq!(response["history"][0]["launch_id"], json!("a"));

        // Неизвестный экземпляр — пустая история, а не ошибка.
        assert_eq!(launch_history_from(&game, "нет такого")["history"], json!([]));
        // Пустое имя — тоже (уровень обработчика).
        assert_eq!(launch_history(&json!({ "name": "  " })).unwrap()["history"], json!([]));

        // Разрушенный JSON → пустая история, не ошибка.
        fs::write(instance.join(LAUNCH_HISTORY_FILE), "{not json").unwrap();
        assert_eq!(read_launch_history(&instance), json!([]));
    }

    #[test]
    fn cache_stats_and_prune_rules() {
        let root = fixture_root("cache");
        let cache = root.join("downloads");
        fs::create_dir_all(cache.join("nested")).unwrap();
        fs::write(cache.join("a.jar"), vec![0u8; 1024]).unwrap();
        fs::write(cache.join("nested").join("b.jar"), vec![0u8; 2048]).unwrap();
        let stale = cache.join("meta.json");
        fs::write(&stale, b"{}").unwrap();
        let fresh = cache.join("index.cache");
        fs::write(&fresh, b"{}").unwrap();

        let stats = cache_stats_from(&cache)["cache"].clone();
        assert_eq!(stats["file_count"], json!(4));
        assert_eq!(stats["total_bytes"], json!(1024 + 2048 + 2 + 2));

        // Огромный возраст: cutoff глубоко в прошлом — ничего не удалено.
        assert_eq!(cache_prune_from(&cache, 100_000)["deleted_files"], json!(0));
        assert!(stale.exists());
        assert!(fresh.exists());

        // max_age_hours=0: cutoff = «сейчас»; mtime файлов строго раньше —
        // метаданные удаляются (ровно как path.stat().st_mtime < cutoff).
        assert_eq!(cache_prune_from(&cache, 0)["deleted_files"], json!(2));
        assert!(!stale.exists() && !fresh.exists());
        // .jar не тронуты — чистка выбирает только .json/.cache.
        assert!(cache.join("a.jar").exists());

        // Отсутствующий каталог кеша — нули без ошибок.
        let empty = cache_stats_from(&root.join("no-such-cache"));
        assert_eq!(empty["cache"]["file_count"], json!(0));
        assert_eq!(empty["cache"]["total_bytes"], json!(0));
        assert_eq!(empty["cache"]["total_mib"], json!(0.0));
    }

    #[test]
    fn mib_rounding_is_half_even() {
        // 262144 байта = ровно 0,25 МиБ → 0.2, а не 0.3.
        assert_eq!(round_mib_one_decimal(262_144), 0.2);
        assert_eq!(round_mib_one_decimal(786_432), 0.8);
        assert_eq!(round_mib_one_decimal(1_048_576), 1.0);
        assert_eq!(round_mib_one_decimal(0), 0.0);
    }

    #[test]
    fn instance_names_reference() {
        assert_eq!(safe_instance_name("My World"), "My-World");
        assert_eq!(safe_instance_name("  ../..//evil  "), "evil");
        assert_eq!(safe_instance_name("???"), "default");
        assert_eq!(safe_instance_name("a/b\\c"), "a-b-c");
        let long = "x".repeat(100);
        assert_eq!(safe_instance_name(&long).len(), 64);
    }

    #[test]
    fn age_hours_parsing_covers_int_forms() {
        assert_eq!(request_age_hours(&json!({ "max_age_hours": 48 })).unwrap(), 48);
        assert_eq!(request_age_hours(&json!({ "max_age_hours": 12.9 })).unwrap(), 12);
        assert_eq!(request_age_hours(&json!({ "max_age_hours": "24" })).unwrap(), 24);
        assert_eq!(request_age_hours(&json!({})).unwrap(), 48);
        assert!(request_age_hours(&json!({ "max_age_hours": "abc" })).is_err());
    }

    /// Сверка с эталонной реализацией на общих fixture-каталогах:
    /// scripts/parity_ops.sh подставляет пути через переменные окружения
    /// и сравнивает ответы. Без них тест пропускается.
    #[test]
    fn live_parity_ops_fixture() {
        let Ok(presets_out) = std::env::var("NIVORA_PARITY_PRESETS_OUT") else {
            return;
        };
        fs::write(&presets_out, serde_json::to_string_pretty(&json!({ "presets": jvm_presets() })).unwrap()).unwrap();
        if let (Ok(game), Ok(name), Ok(history_out)) = (
            std::env::var("NIVORA_PARITY_HISTORY_GAME"),
            std::env::var("NIVORA_PARITY_HISTORY_NAME"),
            std::env::var("NIVORA_PARITY_HISTORY_OUT"),
        ) {
            let value = launch_history_from(Path::new(&game), &name);
            fs::write(&history_out, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        }
        if let (Ok(cache_dir), Ok(cache_out)) = (
            std::env::var("NIVORA_PARITY_CACHE_DIR"),
            std::env::var("NIVORA_PARITY_CACHE_OUT"),
        ) {
            let value = cache_stats_from(Path::new(&cache_dir));
            fs::write(&cache_out, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        }
    }
}
