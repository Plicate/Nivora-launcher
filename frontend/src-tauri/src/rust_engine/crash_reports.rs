// разбор лога упавшего экземпляра: короткий вердикт и конкретные советы.
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

const MAX_EXCERPT_LINES: usize = 24;

fn re_fabric_required() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)mod\s+'([^']+)'\s+(\d[\w.\-+]*)?\s*(?:requires|depends)").unwrap())
}

fn re_fabric_incompatible() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)Mod\s+([A-Za-z0-9_.\-]+)\s+(?:requires|incompatible)").unwrap())
}

fn re_missing_general() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:Missing or unsupported mandatory dependencies|missing dependency)[:\s]*(.*)").unwrap())
}

fn re_mod_mention() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bmod\s+'?([A-Za-z0-9_ .\-]{2,48})'?(\s+\S+)?\s+(?:requires|is incompatible|caused)").unwrap())
}

fn re_compiled_newer_java() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"compiled by a more recent version of the java runtime").unwrap())
}

fn re_class_file_version() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"class file version (\d+)").unwrap())
}

fn re_mods_conflict() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"missing or unsupported mandatory dependencies|some of your mods are incompatible").unwrap())
}

fn re_port_busy() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"failed to bind to port|maybe another server is running").unwrap())
}

fn re_gpu() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"no opengl context|could not create gl|glfw error|failed to create window|opengl.*not supported").unwrap())
}

fn re_corrupted() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"corrupted|failed to download|checksum mismatch").unwrap())
}

fn re_hs_err() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"hs_err_pid\d+").unwrap())
}

fn re_error_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)error|exception|caused by").unwrap())
}

pub(crate) fn analyze_crash(
    lines: &[String],
    exit_code: Option<i64>,
    instance_path: Option<&Path>,
    memory: &str,
    _minecraft_version: &str,
    loader: &str,
) -> Value {
    let bound = lines.len().min(400);
    let text = lines[lines.len() - bound..].join("\n");
    let lowered = text.to_lowercase();
    let mut hints: Vec<Value> = Vec::new();
    let mut suspected_mods: Vec<Value> = Vec::new();
    let mut title = String::new();

    if lowered.contains("outofmemoryerror") || lowered.contains("could not reserve enough space for object heap") {
        title = "Java не хватило памяти".into();
        let mut detail = format!(
            "Выделено сейчас: {}. Тяжёлым сборкам обычно нужно 4–8 ГБ.",
            if memory.is_empty() { "auto" } else { memory }
        );
        if loader != "vanilla" {
            detail.push_str(" Пресет Heavy Modpack тоже помогает GC.");
        }
        hints.push(json!({ "kind": "memory", "title": "Увеличьте память экземпляра", "detail": detail }));
    }

    if lowered.contains("unsupportedclassversionerror") || re_compiled_newer_java().is_match(&lowered) {
        let mut java_hint = "Игра требует более новую Java, чем выбранная.".to_owned();
        if let Some(captures) = re_class_file_version().captures(&lowered) {
            // 61=17, 65=21, 66=22
            let major: i64 = captures[1].parse().unwrap_or(0) - 44;
            if major >= 8 {
                java_hint.push_str(&format!(" Нужна Java {major} или новее."));
            }
        }
        if title.is_empty() {
            title = "Несовместимая версия Java".into();
        }
        hints.push(json!({ "kind": "java", "title": "Смените Java", "detail": java_hint }));
    }

    if re_mods_conflict().is_match(&lowered) {
        if title.is_empty() {
            title = "Конфликт модов".into();
        }
        hints.push(json!({
            "kind": "mods",
            "title": "Несовместимые или отсутствующие зависимости",
            "detail": "Загрузчик перечислил проблемные моды ниже. Отключите или удалите их и попробуйте снова.",
        }));
        suspected_mods = extract_mods(&text);
    }

    if re_port_busy().is_match(&lowered) {
        if title.is_empty() {
            title = "Порт сервера занят".into();
        }
        hints.push(json!({
            "kind": "port",
            "title": "Смените порт",
            "detail": "Порт уже используется другим процессом — измените server_port в настройках сервера.",
        }));
    }

    if lowered.contains("you need to agree to the eula") {
        if title.is_empty() {
            title = "Не принято EULA".into();
        }
        hints.push(json!({
            "kind": "eula",
            "title": "Примите EULA",
            "detail": "Откройте настройки сервера и отметьте согласие с EULA.",
        }));
    }

    if re_gpu().is_match(&lowered) {
        if title.is_empty() {
            title = "Проблема с графикой".into();
        }
        hints.push(json!({
            "kind": "gpu",
            "title": "Обновите драйвер видеокарты",
            "detail": "Minecraft не смог создать OpenGL-контекст. Также попробуйте переключить режим графики в настройках экземпляра.",
        }));
    }

    if re_corrupted().is_match(&lowered) {
        if title.is_empty() {
            title = "Повреждённые файлы".into();
        }
        hints.push(json!({
            "kind": "files",
            "title": "Проверьте файлы",
            "detail": "Часть файлов не скачалась или повреждена. Переустановите версию или повторите загрузку.",
        }));
    }

    let hs_err = re_hs_err().captures(&lowered);
    if hs_err.is_some() || lowered.contains("sigsegv") || lowered.contains("exception_access_violation") {
        if title.is_empty() {
            title = "Аварийное завершение JVM".into();
        }
        let report = hs_err.map(|captures| captures[0].to_owned()).unwrap_or_default();
        let mut detail = "Уберите кастомные JVM-аргументы или смените пресет GC; при повторении обновите Java.".to_owned();
        if !report.is_empty() {
            detail.push_str(&format!(" Отчёт JVM: {report}"));
        }
        hints.push(json!({ "kind": "jvm", "title": "Упала сама Java, не Minecraft", "detail": detail }));
    }

    if title.is_empty() {
        title = match exit_code {
            Some(code) if code != 0 => format!("Экземпляр завершился с кодом {code}"),
            _ => "Экземпляр завершился с ошибкой".to_owned(),
        };
        hints.push(json!({
            "kind": "generic",
            "title": "Явных признаков не найдено",
            "detail": "Посмотрите выдержку лога — обычно причина в последних 20 строках перед падением.",
        }));
    }

    // retain не даёт менять элементы, а фильтр дописывает найденный файл.
    let mut kept = Vec::with_capacity(suspected_mods.len());
    for mut value in std::mem::take(&mut suspected_mods) {
        if match_mod_file(&mut value, instance_path) {
            kept.push(value);
        }
    }
    suspected_mods = kept;
    json!({
        "exit_code": match exit_code { Some(code) => json!(code), None => Value::Null },
        "title": title,
        "hints": hints,
        "suspected_mods": suspected_mods,
        "excerpt": excerpt(lines),
    })
}

fn excerpt(lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let tail: Vec<String> = lines.iter().rev().take(MAX_EXCERPT_LINES).rev().cloned().collect();
    // последняя запись об ошибке часто в середине — оставляем и её контекст.
    let stop = (lines.len() as i64 - (MAX_EXCERPT_LINES as i64) * 4).max(-1);
    let mut index = lines.len() as i64 - 1;
    while index > stop {
        let position = index as usize;
        if re_error_line().is_match(&lines[position]) {
            let start = position.saturating_sub(4);
            let end = (position + 8).min(lines.len());
            let window: Vec<String> = lines[start..end].to_vec();
            let window_is_last = window.last().map(String::as_str) == lines.last().map(String::as_str);
            if window_is_last {
                return window;
            }
            let mut result = window;
            result.push("…".into());
            result.extend(tail.iter().rev().take(6).rev().cloned());
            return result;
        }
        index -= 1;
    }
    tail
}

fn extract_mods(text: &str) -> Vec<Value> {
    let patterns = [re_fabric_required(), re_fabric_incompatible(), re_mod_mention(), re_missing_general()];
    let mut found: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for line in text.lines() {
        for pattern in patterns {
            let Some(captures) = pattern.captures(line) else { continue };
            let name = captures
                .get(1)
                .map(|match_value| match_value.as_str())
                .unwrap_or_default()
                .trim_matches(|c| c == ' ' || c == '\'' || c == '"')
                .to_owned();
            if name.chars().count() < 2 || seen.contains(&name.to_lowercase()) {
                continue;
            }
            seen.push(name.to_lowercase());
            let reason: String = line.trim().chars().take(300).collect();
            found.push(json!({ "name": name, "reason": reason }));
        }
    }
    found.truncate(10);
    found
}

fn match_mod_file(mod_value: &mut Value, instance_path: Option<&Path>) -> bool {
    let Some(root) = instance_path else { return true };
    let mods_dir = root.join("mods");
    if !mods_dir.is_dir() {
        return true;
    }
    let needle = mod_value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase()
        .replace(' ', "");
    let Ok(entries) = fs::read_dir(&mods_dir) else { return true };
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        let stem = Path::new(&file_name)
            .file_stem()
            .map(|value| value.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let stem = stem.strip_suffix(".disabled").unwrap_or(&stem).to_owned();
        if !needle.is_empty() && (stem.contains(&needle) || needle.contains(stem.as_str())) {
            if let Some(fields) = mod_value.as_object_mut() {
                fields.insert("file".into(), json!(file_name));
            }
            return true;
        }
    }
    false
}

/// хвост последнего hs_err_pid*.log в каталоге экземпляра.
pub(crate) fn read_hs_err_report(instance_path: &Path, limit: usize) -> Vec<String> {
    let Ok(entries) = fs::read_dir(instance_path) else { return Vec::new() };
    let mut reports: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("hs_err_pid") && name.ends_with(".log")) || !entry.path().is_file() {
            continue;
        }
        // stat недоступен хотя бы для одного файла — как в Python, отчётов нет.
        let Ok(metadata) = entry.metadata() else { return Vec::new() };
        let Ok(modified) = metadata.modified() else { return Vec::new() };
        reports.push((modified, entry.path()));
    }
    if reports.is_empty() {
        return Vec::new();
    }
    reports.sort_by(|left, right| right.0.cmp(&left.0));
    let content = fs::read(&reports[0].1).unwrap_or_default();
    String::from_utf8_lossy(&content)
        .lines()
        .take(limit)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-crash-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    fn lines(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn out_of_memory_hint() {
        let report = analyze_crash(
            &lines(&["java.lang.OutOfMemoryError: Java heap space", "at net.minecraft.server.MinecraftServer"]),
            Some(1),
            None,
            "2 ГБ",
            "1.20.4",
            "fabric",
        );
        assert_eq!(report["title"], json!("Java не хватило памяти"));
        assert_eq!(report["hints"][0]["kind"], json!("memory"));
        assert_eq!(report["hints"][0]["detail"], json!("Выделено сейчас: 2 ГБ. Тяжёлым сборкам обычно нужно 4–8 ГБ. Пресет Heavy Modpack тоже помогает GC."));
        assert_eq!(report["exit_code"], json!(1));
    }

    #[test]
    fn java_version_hint_with_class_file_major() {
        let report = analyze_crash(
            &lines(&["java.lang.UnsupportedClassVersionError: Foo has been compiled by a more recent version of the Java Runtime (class file version 65.0)"]),
            Some(1),
            None,
            "",
            "",
            "vanilla",
        );
        assert_eq!(report["title"], json!("Несовместимая версия Java"));
        assert_eq!(report["hints"][0]["detail"], json!("Игра требует более новую Java, чем выбранная. Нужна Java 21 или новее."));
    }

    #[test]
    fn mod_conflict_extracts_suspects_and_matches_files() {
        let root = temp_root("mods");
        let mods = root.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("create-fabric-0.3.0.jar"), b"jar").unwrap();
        let log = lines(&[
            "Caused by: org.quiltmc.loader.impl.QuiltLoaderException: net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible!",
            "Mod 'create' 0.3.0 requires fabric-api 0.90.0",
            "missing dependency: indium",
        ]);
        let report = analyze_crash(&log, Some(1), Some(&root), "", "", "quilt");
        assert_eq!(report["title"], json!("Конфликт модов"));
        let suspects = report["suspected_mods"].as_array().unwrap();
        // create найден в mods/ и получил имя файла; indium убран — файла нет.
        assert_eq!(suspects.len(), 1);
        assert_eq!(suspects[0]["name"], json!("create"));
        assert_eq!(suspects[0]["file"], json!("create-fabric-0.3.0.jar"));
    }

    #[test]
    fn simple_titles_and_hints() {
        let port = analyze_crash(&lines(&["[ServerThread/ERROR]: Failed to bind to port 25565"]), Some(1), None, "", "", "vanilla");
        assert_eq!(port["title"], json!("Порт сервера занят"));
        let eula = analyze_crash(&lines(&["You need to agree to the EULA in order to run the server"]), None, None, "", "", "vanilla");
        assert_eq!(eula["title"], json!("Не принято EULA"));
        assert_eq!(eula["exit_code"], Value::Null);
        let gpu = analyze_crash(&lines(&["GLFW error 65543: WGL: OpenGL context creation failed"]), Some(255), None, "", "", "vanilla");
        assert_eq!(gpu["title"], json!("Проблема с графикой"));
        let files = analyze_crash(&lines(&["java.io.IOException: checksum mismatch for assets/xx"]), Some(1), None, "", "", "vanilla");
        assert_eq!(files["title"], json!("Повреждённые файлы"));
        let jvm = analyze_crash(&lines(&["# A fatal error has been detected by SIGSEGV"]), Some(134), None, "", "", "vanilla");
        assert_eq!(jvm["title"], json!("Аварийное завершение JVM"));
    }

    #[test]
    fn jvm_hint_names_hs_err_report() {
        let report = analyze_crash(&lines(&["# There is insufficient memory / hs_err_pid1234.log"]), Some(134), None, "", "", "vanilla");
        assert_eq!(report["title"], json!("Аварийное завершение JVM"));
        assert_eq!(report["hints"][0]["detail"], json!("Уберите кастомные JVM-аргументы или смените пресет GC; при повторении обновите Java. Отчёт JVM: hs_err_pid1234"));
    }

    #[test]
    fn generic_title_uses_exit_code() {
        let with_code = analyze_crash(&lines(&["[main/INFO]: Stopping!"]), Some(13), None, "", "", "vanilla");
        assert_eq!(with_code["title"], json!("Экземпляр завершился с кодом 13"));
        let zero_code = analyze_crash(&lines(&["[main/INFO]: Stopping!"]), Some(0), None, "", "", "vanilla");
        assert_eq!(zero_code["title"], json!("Экземпляр завершился с ошибкой"));
        let no_code = analyze_crash(&lines(&["[main/INFO]: Stopping!"]), None, None, "", "", "vanilla");
        assert_eq!(no_code["title"], json!("Экземпляр завершился с ошибкой"));
    }

    #[test]
    fn excerpt_prefers_error_context_and_falls_back_to_tail() {
        let mut log: Vec<String> = (1..=200).map(|index| format!("line {index}")).collect();
        // поиск идёт только по последним MAX*4 строкам: ошибку вне окна
        // выдержка не видит — честный хвост, как в Python.
        log[49] = "java.lang.Exception: missed".into();
        let tail_only = excerpt(&log);
        assert_eq!(tail_only.len(), 24);
        assert!(!tail_only.contains(&"java.lang.Exception: missed".to_owned()));

        log[150] = "java.lang.Exception: boom".into();
        let picked = excerpt(&log);
        assert_eq!(picked.last().unwrap(), "line 200");
        assert!(picked.contains(&"java.lang.Exception: boom".to_owned()));
        // окно заканчивается не последней строкой — добавлен разделитель.
        assert!(picked.contains(&"…".to_owned()));
        let quiet: Vec<String> = (1..=30).map(|index| format!("ok {index}")).collect();
        let tail = excerpt(&quiet);
        assert_eq!(tail.len(), 24);
        assert_eq!(tail.last().unwrap(), "ok 30");
        assert!(excerpt(&[]).is_empty());
    }

    #[test]
    fn hs_err_report_reads_latest_file_head() {
        let root = temp_root("hs-err");
        fs::write(root.join("hs_err_pid10.log"), "first\nsecond\nthird\n").unwrap();
        let newer = root.join("hs_err_pid11.log");
        fs::write(&newer, "").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        fs::File::options().write(true).open(&newer).unwrap().set_modified(future).unwrap();
        fs::write(root.join("ignored.log"), "nope").unwrap();
        let head = read_hs_err_report(&root, 2);
        assert_eq!(head, Vec::<String>::new());
        fs::write(&newer, "a\nb\nc\n").unwrap();
        fs::File::options().write(true).open(&newer).unwrap().set_modified(future).unwrap();
        assert_eq!(read_hs_err_report(&root, 2), lines(&["a", "b"]));
        assert!(read_hs_err_report(&root.join("missing"), 5).is_empty());
    }
}
