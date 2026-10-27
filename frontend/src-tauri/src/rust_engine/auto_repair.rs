use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::platform;

// дочерние модули (apply/diagnose/scan) обращаются к движку через `super::<module>`
pub(super) use super::accounts;
pub(super) use super::backups;
pub(super) use super::config;
pub(super) use super::events;
pub(super) use super::installer;
pub(super) use super::instance_manifest;
pub(super) use super::instance_workspace;
pub(super) use super::instances;
pub(super) use super::mod_dependencies;
pub(super) use super::mods;
pub(super) use super::net;
pub(super) use super::runtime;
pub(super) use super::security;
pub(super) use super::server;
pub(super) use super::turbo;

mod apply;
mod diagnose;
mod scan;
pub(crate) use scan::{scan_mods, scan_mods_full, wrong_loader, ModEntry};

/// имя файла с локальным состоянием диагностики
pub(crate) const STATE_FILE: &str = ".nivora-auto-repair.json";
const MAX_STATE_JSON_BYTES: u64 = 64 * 1024;
/// максимальное число модов для сканирования
const MAX_MODS: usize = 300;
const MAX_CRASH_REPORT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LOG_LINES: usize = 1500;
const MAX_EVIDENCE: usize = 4;
const MAX_FILES_PER_FIX: usize = 40;
/// каталог резервных копий конфигов внутри экземпляра
pub(crate) const BACKUP_DIRECTORY: &str = ".nivora-repair-backups";

/// находит последний отчёт о сбое, созданный не раньше указанного времени
pub(crate) fn latest_crash_report(instance_path: &Path, since: f64) -> String {
    let directory = instance_path.join("crash-reports");
    let Ok(meta) = fs::symlink_metadata(&directory) else {
        return String::new();
    };
    if meta.is_symlink() || !meta.is_dir() {
        return String::new();
    }
    let Ok(entries) = fs::read_dir(&directory) else {
        return String::new();
    };
    let mut newest: Option<(f64, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        let is_txt = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case("txt"))
            .unwrap_or(false);
        if !is_txt {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        if mtime < since {
            continue;
        }
        if newest.as_ref().map(|(best, _)| mtime > *best).unwrap_or(true) {
            newest = Some((mtime, path));
        }
    }
    let Some((_, path)) = newest else {
        return String::new();
    };
    let mut bytes = Vec::new();
    let Ok(mut handle) = fs::File::open(&path) else {
        return String::new();
    };
    // ограничиваем размер читаемого отчёта
    let mut limited = (&mut handle).take(MAX_CRASH_REPORT_BYTES);
    if limited.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).to_string()
}


/// возвращает пустое состояние, если файла нет, он повреждён или является ссылкой
pub(crate) fn read_state(instance_path: &Path) -> Map<String, Value> {
    let path = instance_path.join(STATE_FILE);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Map::new();
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_STATE_JSON_BYTES {
        return Map::new();
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// запоминает время успешного запуска, чтобы находить недавно добавленные моды
pub(crate) fn record_launch_result(instance_path: Option<&Path>, exit_code: Option<i64>) {
    let Some(instance_path) = instance_path else { return };
    if exit_code != Some(0) {
        return;
    }
    let path = instance_path.join(STATE_FILE);
    if path.is_symlink() {
        return;
    }
    let mut state = read_state(instance_path);
    state.insert("last_success".to_owned(), Value::from(epoch_seconds()));
    if let Ok(mut file) = platform::create_private(&path) {
        let _ = serde_json::to_writer(&mut file, &Value::Object(state));
    }
}

fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}



/// используется для показа проблем в настройках и окне починки
const ISSUE_ORDER: [(&str, &str); 20] = [
    ("account", "Проблема с аккаунтом"),
    ("security_sandbox", "Запуску мешают настройки безопасности"),
    ("loader_broken", "Модлоадер повреждён или не запускается"),
    ("loader_outdated", "Устаревшая версия модлоадера"),
    ("missing_dependencies", "Не хватает зависимостей модов"),
    ("incompatible_mods", "Несовместимые моды"),
    ("wrong_loader_mods", "Моды для другого загрузчика"),
    ("duplicate_mods", "Дубликаты модов"),
    ("mixin_failure", "Ошибка Mixin в моде"),
    ("corrupted_mods", "Повреждённые файлы модов"),
    ("broken_config", "Повреждённые конфиги модов"),
    ("out_of_memory", "Не хватило памяти"),
    ("memory_too_high", "Выделено слишком много памяти"),
    ("java_version", "Неподходящая версия Java"),
    ("jvm_arguments", "Ошибочные аргументы JVM"),
    ("jvm_crash", "Аварийное падение Java"),
    ("game_files", "Повреждены файлы игры или загрузчика"),
    ("graphics", "Проблема с графикой"),
    ("recent_mods", "Недавно добавленные моды"),
    ("unknown_crash", "Неизвестная причина краша"),
];

fn issue_title(kind: &str) -> String {
    ISSUE_ORDER
        .iter()
        .find(|(id, _)| *id == kind)
        .map(|(_, title)| (*title).to_owned())
        .unwrap_or_else(|| kind.to_owned())
}

/// канонический порядок типов проблем для настроек
pub(crate) fn issue_kinds() -> &'static [(&'static str, &'static str)] {
    &ISSUE_ORDER
}

/// идентификаторы самих загрузчиков в манифестах и сообщениях
fn loader_dependency_ids(loader: &str) -> &'static [&'static str] {
    match loader {
        "fabric" => &["fabricloader", "fabric-loader", "fabric_loader"],
        "quilt" => &["quilt_loader", "quilt-loader", "quiltloader"],
        "forge" => &["forge", "fml", "javafml"],
        "neoforge" => &["neoforge", "fml", "javafml"],
        _ => &[],
    }
}

const ALL_LOADER_DEPENDENCY_IDS: [&str; 12] = [
    "fabricloader", "fabric-loader", "fabric_loader",
    "quilt_loader", "quilt-loader", "quiltloader",
    "forge", "fml", "javafml", "neoforge",
    // повторение fml/javafml опущено: проверка через contains
    "fabricloader", "quiltloader",
];

fn is_loader_dependency_id(id: &str) -> bool {
    ALL_LOADER_DEPENDENCY_IDS.contains(&id)
}

fn loader_title(loader: &str) -> String {
    match loader {
        "fabric" => "Fabric".to_owned(),
        "quilt" => "Quilt".to_owned(),
        "forge" => "Forge".to_owned(),
        "neoforge" => "NeoForge".to_owned(),
        "vanilla" => "Vanilla".to_owned(),
        other => other.capitalize_first(),
    }
}

/// разрешения обычной игры, нехватка которых требует настройки, а не проверки модов
const GAME_PERMISSIONS: [&str; 4] = ["network:game", "files:instance", "desktop:display", "desktop:audio"];
const SANDBOXED_PROFILES: [&str; 2] = ["strict", "manual"];

/// пакеты JDK, игры и загрузчиков: в стеке они не указывают на конкретный мод
const FRAMEWORK_PACKAGES: [&str; 31] = [
    "java.", "javax.", "jdk.", "sun.", "com.sun.", "net.minecraft.", "com.mojang.", "net.fabricmc.",
    "org.quiltmc.", "cpw.mods.", "net.minecraftforge.", "net.neoforged.", "org.spongepowered.",
    "org.lwjgl.", "io.netty.", "com.google.", "org.apache.", "it.unimi.", "oshi.", "org.slf4j.",
    "org.objectweb.", "com.llamalad7.", "kotlin.", "scala.", "org.jline.", "joptsimple.", "com.electronwill.",
    "org.openjdk.", "net.jodah.", "com.ibm.", "MC-BOOTSTRAP",
];

const SHADER_MOD_IDS: [&str; 6] = ["iris", "oculus", "optifabric", "optifine", "canvas", "irisshaders"];
const NOISE_IDS: [&str; 15] = [
    "minecraft", "fabricloader", "fabric-loader", "fabric", "quilt_loader", "quilt-loader",
    "quilt_base", "quilted_fabric_api", "forge", "neoforge", "minecraftforge", "java",
    "cloth-config", "cloth_config", "architectury",
];

struct Patterns {
    fabric_requires: Regex,
    fabric_wrong_version: Regex,
    fabric_incompatible: Regex,
    forge_dependency: Regex,
    forge_duplicate: Regex,
    fml_requires: Regex,
    mixin_for_mod: Regex,
    mixin_from_mod: Regex,
    mixin_config: Regex,
    module_conflict: Regex,
    module_reads: Regex,
    crash_mod_file: Regex,
    crash_suspect_id: Regex,
    class_file_version: Regex,
    requires_java: Regex,
    config_path: Regex,
    forge_config_file: Regex,
    oom: Regex,
    memory_too_high: Regex,
    jvm_arguments: Regex,
    jvm_crash: Regex,
    driver_frame: Regex,
    graphics: Regex,
    game_files: Regex,
    loader_broken: Regex,
    account_relogin: Regex,
    account_missing: Regex,
    account_no_license: Regex,
    account_xsts: Regex,
    account_storage: Regex,
    account_multiplayer: Regex,
    sandbox_launcher: Regex,
    sandbox_network: Regex,
    sandbox_files: Regex,
    sandbox_process: Regex,
    sandbox_native: Regex,
    sandbox_display: Regex,
    stack_frame: Regex,
    mixin_handler: Regex,
    corrupted_zip: Regex,
    java_missing: Regex,
    suspect_block: Regex,
    non_latin_path: Regex,
}

fn ci(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}")).expect("regex diagnose")
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        // зависимость в сообщении Fabric: 'Name' (id) | 'id' | id
        fabric_requires: ci(r"Mod '([^']+)' \(([\w.\-]+)\)[^\n]*? requires ([^\n]*?) of (?:mod )?(?:'([^']+)' \(([\w.\-]+)\)|'([\w.\-]+)'|([\w.\-]+)),\s*which is missing"),
        fabric_wrong_version: ci(r"Mod '([^']+)' \(([\w.\-]+)\)[^\n]*? requires ([^\n]*?) of (?:mod )?(?:'([^']+)' \(([\w.\-]+)\)|'([\w.\-]+)'|([\w.\-]+)),\s*but only the wrong version is present"),
        fabric_incompatible: ci(r"Mod '([^']+)' \(([\w.\-]+)\)[^\n]*?is incompatible with [^\n]*?(?:mod )?'([^']+)'(?: \(([\w.\-]+)\))?"),
        forge_dependency: ci(r"Mod ID: '([\w.\-]+)', Requested by: '([\w.\-]+)', Expected range: '([^']*)', Actual version: '([^']*)'"),
        forge_duplicate: ci(r"Mod ID: '([\w.\-]+)' from mod files: ([^\n]+)"),
        // neoForge (FML 4+): "Mod create requires neoforge 21.1.77 or above\nCurrently, neoforge is 21.1.50"
        fml_requires: ci(r"Mod ([\w.\-]+) requires ([\w.\-]+) ([\w.\-+]+) or above\s*(?:\n\s*)?Currently, ([\w.\-]+) is (not installed|[\w.\-+]+)"),
        mixin_for_mod: ci(r"Mixin apply (?:for|of) mod ([\w.\-]+) failed"),
        mixin_from_mod: ci(r"from mod ([\w.\-]+)(?:\]| failed| ->)"),
        mixin_config: ci(r"([\w.\-]+\.mixins?\.json)"),
        module_conflict: ci(r"Modules? ([\w.\-]+) and ([\w.\-]+) export package"),
        module_reads: ci(r"reads package [\w.]+ from both ([\w.\-]+) and ([\w.\-]+)"),
        crash_mod_file: ci(r"Mod File: (?:[^\n]*[/\\])?([^\n/\\]+\.jar)"),
        crash_suspect_id: Regex::new(r"(?im)^\s+[^\n(]+\(([\w.\-]+)\),\s*Version").unwrap(),
        class_file_version: ci(r"class file version (\d+)(?:\.\d+)?"),
        requires_java: ci(r"(?:requires (?:version )?(\d{1,2})(?: or later)? of (?:mod )?'?java'?|requires java (\d{1,2})|требуется Java (\d{1,2})|Java (\d{1,2}) or (?:newer|later) is required)"),
        config_path: ci(r"config[/\\]([\w.\-/\\ ]+?\.(?:toml|json5?|cfg|properties|ya?ml|txt|snbt))"),
        forge_config_file: ci(r"(?:Failed loading config file|Failed to load config|Error loading config|Configuration file) ([\w.\-]+\.(?:toml|json5?|cfg))"),
        oom: ci(r"java\.lang\.OutOfMemoryError|GC overhead limit exceeded|Java heap space"),
        memory_too_high: ci(r"Could not reserve enough space for|Invalid maximum heap size|insufficient memory for the Java Runtime|Native memory allocation \(mmap\) failed|Cannot allocate memory"),
        jvm_arguments: ci(r"Unrecognized VM option|Unrecognized option:|Error occurred during initialization of VM|Invalid initial heap size|Conflicting collector combinations|Multiple garbage collectors selected|Improperly specified VM option|Could not create the Java Virtual Machine"),
        jvm_crash: ci(r"A fatal error has been detected by the Java Runtime|SIGSEGV|SIGBUS|EXCEPTION_ACCESS_VIOLATION|hs_err_pid\d+"),
        driver_frame: ci(r"atio6axx|atioglxx|nvoglv|ig\d+icd|libnvidia|amdvlk|radeonsi|iris_dri|i965_dri|libGLX|opengl32"),
        graphics: ci(r"no OpenGL context|could not create (?:the )?(?:GL|OpenGL)|GLFW error (?:65542|65543|65544|65545)|failed to create window|Pixel format not accelerated|WGL: The driver does not appear to support OpenGL|OpenGL[^\n]{0,40}not supported|GLX: Failed to create context|Failed to create OpenGL context"),
        game_files: ci(r"Could not find or load main class|ClassNotFoundException: (?:net\.fabricmc\.loader|cpw\.mods|net\.minecraftforge\.(?:fml|bootstrap)|net\.neoforged\.(?:fml|bootstrap)|org\.quiltmc\.loader|net\.minecraft\.client\.main|net\.minecraft\.launchwrapper)|NoClassDefFoundError: (?:org/lwjgl|net/fabricmc/loader|cpw/mods|net/minecraftforge/fml/loading|net/neoforged/fml)|no lwjgl[\w]* in java\.library\.path|UnsatisfiedLinkError|Failed to locate library|checksum mismatch|SHA-?1 mismatch|hash mismatch|Failed to download|(?:FileNotFound|NoSuchFile)Exception[^\n]*(?:libraries|versions|assets)[/\\]|zip END header not found[^\n]*(?:libraries|versions)|Файл версии не найден|не найден JSON версии"),
        loader_broken: ci(r"Could not find or load main class (?:net\.fabricmc|org\.quiltmc|cpw\.mods|net\.minecraftforge|net\.neoforged)|ClassNotFoundException: (?:net\.fabricmc\.loader|org\.quiltmc\.loader|cpw\.mods|net\.minecraftforge\.(?:fml|bootstrap)|net\.neoforged\.(?:fml|bootstrap|neoforgespi))|NoClassDefFoundError: (?:net/fabricmc/loader|org/quiltmc/loader|cpw/mods|net/minecraftforge/fml|net/neoforged/fml)|Failed to find system mod|Missing (?:minecraft|forge|neoforge|mcp) (?:jar|installation)|InvalidLauncherSetupException|Failed to find (?:the )?Minecraft (?:client )?jar|Unable to locate (?:the )?minecraft|Could not find required file|client-[\w.]+-(?:srg|extra|slim|official)\.jar|java\.lang\.module\.FindException|(?:forge|neoforge|fabric|quilt) installer (?:завершился с кодом|превысил лимит|не создал version JSON|не запустился)|There is no minecraft launcher profile|There was an error during installation|A problem installing was detected|В экземпляре нет установленной версии|Не удалось прочитать metadata версии|отсутствует mainClass|Metadata версии слишком велика|net\.fabricmc\.loader\.impl\.FormattedException: (?:java\.lang\.RuntimeException: )?(?:Failed to|Could not)|Loader version [^\n]{0,40}is not compatible|Failed to load (?:FML|ModLauncher)"),
        account_relogin: ci(r"Microsoft-токен истёк|Повторите login microsoft|Токены Microsoft-аккаунта не найдены|Не удалось обновить Microsoft-токен|invalid_grant|AADSTS\d+|Microsoft OAuth завершился ошибкой|Время ожидания входа Microsoft истекло|InvalidCredentialsException|Invalid token|Invalid session|Failed to verify username|Bad login|User not premium|(?:sessionserver|authserver|api\.minecraftservices)[^\n]{0,120}\b401\b"),
        account_missing: ci(r"Аккаунт не найден: ?([\w.\-]*)|Microsoft-аккаунт не найден|Debug-аккаунт ещё не создан|Нужен Microsoft-аккаунт|Microsoft-аккаунт больше не доступен"),
        account_no_license: ci(r"не найдено право на Minecraft|does not own (?:Minecraft|the game)|Minecraft Services не вернул корректный id профиля|Minecraft Services не выдал access token|Minecraft Services вернул некорректный profile|NOT_FOUND[^\n]{0,60}profile"),
        account_xsts: ci(r"Xbox XSTS отклонил аккаунт, код (\d+)"),
        account_storage: ci(r"Для Microsoft-токенов нужны libsecret|хранилище паролей недоступно|secret-tool[^\n]{0,80}(?:not found|не найден|failed)|CredWrite|CredRead"),
        account_multiplayer: ci(r"Multiplayer is disabled|multiplayer (?:has been )?disabled|UserBannedException|banned from (?:online play|multiplayer)"),
        sandbox_launcher: ci(r"Security profile (?:strict|manual) не может безопасно выдать разрешения:? ?([\w:, \-]*)|strict profile не выдаёт модам запись в файлы компьютера|files:host-read на Windows не поддерживается в strict|strict profile на Windows требует AppContainer|strict profile требует установленный bubblewrap|strict profile поддерживается только на|Песочница Windows:[^\n]*|Java недоступна из песочницы|bwrap: [^\n]+|Не удалось (?:запустить процесс в AppContainer|создать Job Object|ограничить Job Object|назначить AppContainer|поместить процесс в Job Object)|Security cell (?:является|содержит)"),
        sandbox_network: ci(r"java\.net\.UnknownHostException|Network is unreachable|java\.net\.SocketException: (?:Permission denied|Operation not permitted)|Temporary failure in name resolution|Name or service not known|An attempt was made to access a socket in a way forbidden|No such host is known|Этот хост неизвестен"),
        sandbox_files: ci(r"java\.nio\.file\.AccessDeniedException:? ?([^\n]*)|(?:FileNotFoundException|IOException)[^\n]*\((?:Access is denied|Permission denied|Отказано в доступе)\)|Read-only file system|Отказано в доступе|Access is denied"),
        sandbox_process: ci(r#"Cannot run program \"([^\"]+)\"[^\n]*(?:error=\d+|denied|not permitted)|CreateProcess error=(?:5|1450|1816)\b|java\.io\.IOException: error=(?:1|13),"#),
        sandbox_native: ci(r"Unable to load library '([^']+)'|UnsatisfiedLinkError[^\n]*(?:Access is denied|Permission denied|failed to map segment|Operation not permitted)"),
        sandbox_display: ci(r"DISPLAY environment variable is missing|Failed to open (?:X )?display|No X11 DISPLAY|Can't connect to X11|Wayland: Failed to connect to display|/dev/(?:dri|nvidia)[^\n]{0,80}(?:Permission denied|Operation not permitted)"),
        stack_frame: Regex::new(r"(?m)^\s*at (?:[\w.\-]+(?:@[\w.\-]+)?/)?([\w$]+(?:\.[\w$]+)+)\.[\w$<>]+\(").unwrap(),
        mixin_handler: ci(r"\$zz[a-z]\d{3}\$([a-z][a-z0-9_]{1,63})\$"),
        corrupted_zip: ci(r"zip END header not found|invalid LOC header|error in opening zip file|ZipException|Invalid or corrupt jarfile|BadZipFile"),
        java_missing: ci(r#"Java не найден|Java executable не найден|Java должна быть исполняемым|Cannot run program \"[^\"]*java"#),
        suspect_block: Regex::new(r"(?is)Suspected Mods\?:(.*?)(?:\n\S|\n\n|\z)").unwrap(),
        non_latin_path: Regex::new(r#"[A-Za-z]:\\[^\n]*[^\x00-\x7F]"#).unwrap(),
    })
}

trait Capitalize {
    fn capitalize_first(&self) -> String;
}

impl Capitalize for str {
    fn capitalize_first(&self) -> String {
        let mut characters = self.chars();
        match characters.next() {
            Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
            None => String::new(),
        }
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}

/// строки лога, содержащие признаки причины сбоя
fn evidence_lines(text: &str, needles: &[&str]) -> Vec<String> {
    let needles: Vec<String> = needles.iter().filter(|n| !n.is_empty()).map(|n| n.to_lowercase()).collect();
    let mut result: Vec<String> = Vec::new();
    for line in text.lines() {
        let lowered = line.to_lowercase();
        if needles.iter().any(|needle| lowered.contains(needle)) {
            let stripped = line.trim();
            if !stripped.is_empty() && !result.iter().any(|item| item == stripped) {
                result.push(truncate_chars(stripped, 300));
            }
            if result.len() >= MAX_EVIDENCE {
                break;
            }
        }
    }
    result
}

/// обрезка по границам символов, а не байт
fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn tail_evidence(lines: &[String]) -> Vec<String> {
    let start = lines.len().saturating_sub(200);
    for index in (start..lines.len()).rev() {
        let lowered = lines[index].to_lowercase();
        if lowered.contains("error") || lowered.contains("exception") || lowered.contains("caused by") {
            return lines[index..]
                .iter()
                .flat_map(|line| {
                    let stripped = line.trim();
                    (!stripped.is_empty()).then(|| truncate_chars(stripped, 300))
                })
                .take(MAX_EVIDENCE)
                .collect();
        }
    }
    lines
        .iter()
        .rev()
        .take(MAX_EVIDENCE)
        .flat_map(|line| {
            let stripped = line.trim();
            (!stripped.is_empty()).then(|| truncate_chars(stripped, 300))
        })
        .rev()
        .collect()
}

fn first_int(value: &str) -> i64 {
    let mut current = String::new();
    for character in value.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() && current.len() < 2 {
            current.push(character);
        } else if !current.is_empty() {
            return current.parse().unwrap_or(0);
        }
    }
    0
}

fn first_version(value: &str) -> String {
    // \d+(\.\d+)+([-+][\w.]+)? | \d+
    let characters: Vec<char> = value.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        if !characters[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let mut end = index;
        while end < characters.len() && characters[end].is_ascii_digit() {
            end += 1;
        }
        if end + 1 < characters.len() && characters[end] == '.' && characters[end + 1].is_ascii_digit() {
            loop {
                while end < characters.len() && characters[end].is_ascii_digit() {
                    end += 1;
                }
                if end + 1 < characters.len() && characters[end] == '.' && characters[end + 1].is_ascii_digit() {
                    end += 1;
                } else {
                    break;
                }
            }
            let mut tail = end;
            if tail < characters.len() && (characters[tail] == '-' || characters[tail] == '+') {
                tail += 1;
                while tail < characters.len() && (characters[tail].is_ascii_alphanumeric() || characters[tail] == '.' || characters[tail] == '_') {
                    tail += 1;
                }
                end = tail;
            }
            return characters[start..end].iter().collect();
        }
        return characters[start..end].iter().collect();
    }
    String::new()
}

fn is_prerelease(value: &str) -> bool {
    let lowered = value.to_lowercase();
    ["alpha", "beta", "rc", "pre", "snapshot", "dev"].iter().any(|word| lowered.contains(word))
}

/// ключ сравнения версий загрузчиков: 0.16.10 > 0.16.9, 47.3.0 > 47.2.20
fn version_key(value: &str) -> (Vec<i64>, i64) {
    let core = value.split(['-', '+']).next().unwrap_or("");
    let numbers: Vec<i64> = core
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<i64>().ok())
        .take(6)
        .collect();
    let stable = if is_prerelease(value) { 0 } else { 1 };
    (numbers, stable)
}

fn human_memory(kib: i64) -> String {
    let gib = kib as f64 / (1024.0 * 1024.0);
    if gib >= 1.0 {
        let text = format!("{gib:.1}");
        format!("{} ГБ", text.trim_end_matches('0').trim_end_matches('.'))
    } else {
        format!("{} МБ", kib / 1024)
    }
}

/// путь внутри root без ".." и абсолютных частей; ссылки отвергаются
fn safe_child(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains('\0') {
        return None;
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return None;
    }
    for component in relative_path.components() {
        let text = component.as_os_str().to_string_lossy();
        if text == ".." || text.is_empty() {
            return None;
        }
    }
    let candidate = root.join(relative_path);
    if candidate.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(true) {
        return None;
    }
    Some(candidate)
}

fn path_inside_instance(value: &str, instance_path: Option<&Path>) -> bool {
    let candidate = value.trim().trim_matches(['\'', '"']).replace('\\', "/");
    if candidate.starts_with("/instance/") || !Regex::new(r"^(?:/|[A-Za-z]:/)").unwrap().is_match(&candidate) {
        return true;
    }
    let Some(instance_path) = instance_path else {
        return false;
    };
    let root = instance_path.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/').to_lowercase();
    candidate.to_lowercase().starts_with(&format!("{root}/"))
}


fn disable_steps(files: &[String]) -> Vec<String> {
    let shown = if files.len() > 6 {
        format!("{}, и ещё {}", files[..6].join(", "), files.len() - 6)
    } else {
        files.join(", ")
    };
    vec![
        format!("Переименовать в *.jar.disabled: {shown}"),
        "Файлы не удаляются — включить их обратно можно на вкладке «Моды».".to_owned(),
    ]
}

fn disable_fix(files: &[String], label: &str) -> Option<Value> {
    let files = unique(files.iter().cloned());
    let files = &files[..files.len().min(MAX_FILES_PER_FIX)];
    if files.is_empty() {
        return None;
    }
    Some(json!({
        "action": "disable_mods",
        "label": label,
        "steps": disable_steps(files),
        "params": {"files": files},
    }))
}

fn install_fix(ids: &[String]) -> Option<Value> {
    let ids: Vec<String> = unique(ids.iter().cloned())
        .into_iter()
        .filter(|id| Regex::new(r"^[\w.\-]{2,64}$").unwrap().is_match(id))
        .take(MAX_FILES_PER_FIX)
        .collect();
    if ids.is_empty() {
        return None;
    }
    Some(json!({
        "action": "install_dependencies",
        "label": "Скачать недостающие зависимости",
        "steps": [
            format!("Найти в каталоге Modrinth и установить: {}", ids.iter().take(8).cloned().collect::<Vec<_>>().join(", ")),
            "Берётся версия под Minecraft и загрузчик этого экземпляра; ненайденные зависимости будут перечислены.".to_owned(),
        ],
        "params": {"ids": ids},
    }))
}

fn memory_fix(current_kib: i64, target_kib: i64, label: &str) -> Option<Value> {
    if target_kib <= 0 || target_kib == current_kib {
        return None;
    }
    let target = super::config::format_memory_kib(target_kib);
    Some(json!({
        "action": "set_memory",
        "label": format!("{label} до {}", human_memory(target_kib)),
        "steps": [
            format!("Память экземпляра: {} → {}", human_memory(current_kib), human_memory(target_kib)),
            "Сохраняется в настройках этого экземпляра; глобальные настройки не меняются.".to_owned(),
        ],
        "params": {"memory": target},
    }))
}

fn java_fixes(required: i64, java_path: &str) -> Vec<Value> {
    let mut fixes: Vec<Value> = Vec::new();
    let mut candidates: Vec<(i64, PathBuf)> = Vec::new();
    for path in super::runtime::discover_java_paths() {
        if let Some(major) = super::runtime::java_version(&path) {
            if (required == 0 || major >= required) && path.to_string_lossy() != java_path {
                candidates.push((major, path));
            }
        }
    }
    candidates.sort_by_key(|(major, _)| *major);
    if let Some((major, path)) = if required > 0 { candidates.first() } else { candidates.last() }.cloned() {
        fixes.push(json!({
            "action": "set_java",
            "label": format!("Использовать установленную Java {major}"),
            "steps": [
                format!("Java экземпляра → {} (Java {major})", path.display()),
                "Сохраняется в настройках этого экземпляра.".to_owned(),
            ],
            "params": {"java_path": path.to_string_lossy()},
        }));
    }
    if !java_path.is_empty() {
        fixes.push(json!({
            "action": "set_java",
            "label": "Вернуть автоматический выбор Java",
            "steps": [
                format!("Убрать ручной путь к Java: {java_path}"),
                "Лаунчер сам подберёт системную Java нужной версии или скачает официальную среду Mojang.".to_owned(),
            ],
            "params": {"java_path": ""},
        }));
    } else if candidates.is_empty() && required == 0 {
        fixes.push(json!({
            "action": "set_java",
            "label": "Скачать подходящую Java автоматически",
            "steps": ["Оставить автоматический выбор Java: при следующем запуске лаунчер скачает официальную среду Mojang для этой версии.".to_owned()],
            "params": {"java_path": ""},
        }));
    }
    fixes
}

fn jvm_reset_fix(settings: &Map<String, Value>) -> Value {
    let arguments: Vec<String> = settings
        .get("jvm_arguments")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let preset = settings.get("jvm_preset").and_then(Value::as_str).unwrap_or("none");
    let preset = if preset.is_empty() { "none" } else { preset };
    let mut steps: Vec<String> = Vec::new();
    if !arguments.is_empty() {
        let tail = if arguments.len() > 8 { " …" } else { "" };
        steps.push(format!("Убрать аргументы JVM: {}{tail}", arguments[..8.min(arguments.len())].join(" ")));
    }
    if preset != "none" {
        steps.push(format!("Отключить пресет JVM «{preset}»"));
    }
    steps.push("Старые значения можно вернуть в настройках экземпляра.".to_owned());
    json!({"action": "reset_jvm_arguments", "label": "Сбросить аргументы JVM", "steps": steps, "params": {}})
}

fn repair_fix(loader: &str) -> Value {
    let target = if !loader.is_empty() && loader != "vanilla" {
        format!("версию Minecraft и {}", loader.capitalize_first())
    } else {
        "версию Minecraft".to_owned()
    };
    json!({
        "action": "repair_version",
        "label": "Восстановить файлы версии и загрузчика",
        "steps": [
            format!("Заново скачать и проверить {target}"),
            "Старая версия сохраняется в резервной копии и возвращается, если восстановление не удастся.".to_owned(),
            "Моды, миры, конфиги и скриншоты не затрагиваются.".to_owned(),
        ],
        "params": {},
    })
}

fn update_loader_fix(loader: &str, minimum: &str, current: &str, minecraft_version: &str, reinstall: bool) -> Option<Value> {
    if loader_dependency_ids(loader).is_empty() {
        return None;
    }
    let title = loader_title(loader);
    let target = if minimum.is_empty() { "последнюю стабильную версию".to_owned() } else { format!("{minimum} или новее") };
    let label = if reinstall {
        format!("Переустановить {title} последней стабильной версии")
    } else {
        format!("Обновить {title} до {target}")
    };
    let mut download_line = format!("Скачать {title} {target}");
    if !minecraft_version.is_empty() {
        download_line.push_str(&format!(" для Minecraft {minecraft_version}"));
    }
    if !current.is_empty() {
        download_line.push_str(&format!(" (сейчас {current})"));
    }
    Some(json!({
        "action": "update_loader",
        "label": label,
        "steps": [
            download_line,
            "Старая версия загрузчика удаляется только после успешной установки новой.".to_owned(),
            "Моды, миры, конфиги и скриншоты не затрагиваются.".to_owned(),
        ],
        "params": {"minimum": minimum},
    }))
}

fn config_fix(files: &[String]) -> Option<Value> {
    let files = &unique(files.iter().cloned())[..MAX_FILES_PER_FIX];
    if files.is_empty() {
        return None;
    }
    Some(json!({
        "action": "backup_configs",
        "label": "Сбросить повреждённые конфиги",
        "steps": [
            format!("Перенести в резервную папку: {}", files.iter().take(6).cloned().collect::<Vec<_>>().join(", ")),
            format!("Копии лежат в {BACKUP_DIRECTORY}/ внутри экземпляра — их можно вернуть вручную."),
            "При следующем запуске моды создадут конфиги по умолчанию.".to_owned(),
        ],
        "params": {"files": files},
    }))
}

fn graphics_fixes(settings: &Map<String, Value>, mods: &[ModEntry]) -> Vec<Value> {
    let mut fixes: Vec<Value> = Vec::new();
    let mode = settings.get("graphics_mode").and_then(Value::as_str).unwrap_or("auto");
    let mode = if mode.is_empty() { "auto" } else { mode };
    if mode != "auto" {
        fixes.push(json!({
            "action": "set_graphics_mode",
            "label": "Вернуть автоматический выбор видеокарты",
            "steps": [format!("Режим графики экземпляра: {mode} → auto")],
            "params": {"mode": "auto"},
        }));
    } else if cfg!(target_os = "linux") {
        fixes.push(json!({
            "action": "set_graphics_mode",
            "label": "Запускать через Mesa",
            "steps": [
                "Режим графики экземпляра: auto → mesa (открытый драйвер OpenGL)".to_owned(),
                "Вернуть режим можно в настройках экземпляра.".to_owned(),
            ],
            "params": {"mode": "mesa"},
        }));
    }
    let shaders: Vec<String> = mods
        .iter()
        .filter(|mod_entry| {
            let lowered = mod_entry.file.to_lowercase();
            SHADER_MOD_IDS.contains(&mod_entry.mod_id.as_str())
                || ["iris", "oculus", "optifine", "optifabric"].iter().any(|name| lowered.contains(name))
        })
        .map(|mod_entry| mod_entry.file.clone())
        .collect();
    if let Some(fix) = disable_fix(&shaders, "Отключить моды шейдеров") {
        fixes.push(fix);
    }
    fixes
}

fn recent_fix(mods: &[ModEntry], instance_path: Option<&Path>) -> Option<Value> {
    let instance_path = instance_path?;
    let last_success = read_state(instance_path).get("last_success").and_then(Value::as_f64).unwrap_or(0.0);
    if last_success <= 0.0 {
        return None;
    }
    let recent: Vec<String> = mods
        .iter()
        .filter(|mod_entry| mod_entry.mtime > last_success)
        .map(|mod_entry| mod_entry.file.clone())
        .collect();
    if recent.is_empty() || recent.len() > MAX_FILES_PER_FIX {
        return None;
    }
    disable_fix(&recent, "Отключить моды, добавленные после последнего удачного запуска")
}


fn files_for_ids(ids: &[String], by_id: &BTreeMap<String, usize>, mods: &[ModEntry]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for mod_id in ids {
        let key = mod_id.trim().to_lowercase();
        let index = by_id
            .get(&key)
            .or_else(|| by_id.get(&key.replace('-', "_")))
            .or_else(|| by_id.get(&key.replace('_', "-")));
        if let Some(index) = index {
            files.push(mods[*index].file.clone());
        }
    }
    unique(files)
}

fn files_for_names(names: &[String], mods: &[ModEntry]) -> Vec<String> {
    let strip = |value: &str| -> String {
        value
            .to_lowercase()
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect()
    };
    let mut files: Vec<String> = Vec::new();
    for name in names {
        let needle = strip(name);
        if needle.len() < 3 {
            continue;
        }
        for mod_entry in mods {
            let file_stem = mod_entry.file.to_lowercase().split('-').next().unwrap_or_default().to_owned();
            let candidates = [strip(&mod_entry.mod_id), strip(&file_stem)];
            if candidates.contains(&needle) {
                files.push(mod_entry.file.clone());
                break;
            }
        }
    }
    unique(files)
}

fn provided_ids(mods: &[ModEntry]) -> BTreeSet<String> {
    let mut provided = BTreeSet::new();
    for mod_entry in mods {
        if !mod_entry.mod_id.is_empty() {
            provided.insert(mod_entry.mod_id.clone());
            provided.insert(mod_entry.mod_id.replace('_', "-"));
            provided.insert(mod_entry.mod_id.replace('-', "_"));
        }
    }
    provided
}

/// моды из списка подозреваемых в отчёте о сбое
fn crash_report_suspects(report: &str, mods: &[ModEntry]) -> Vec<String> {
    if report.is_empty() {
        return Vec::new();
    }
    let by_id: BTreeMap<String, usize> = mods
        .iter()
        .enumerate()
        .filter(|(_, mod_entry)| !mod_entry.mod_id.is_empty())
        .map(|(index, mod_entry)| (mod_entry.mod_id.clone(), index))
        .collect();
    let by_file: BTreeMap<String, String> = mods
        .iter()
        .map(|mod_entry| (mod_entry.file.to_lowercase(), mod_entry.file.clone()))
        .collect();
    let mut files: Vec<String> = Vec::new();
    if let Some(block) = patterns().suspect_block.captures(report) {
        let block_text = block.get(1).map(|m| m.as_str()).unwrap_or_default();
        let mut suspect_ids: Vec<String> = patterns()
            .crash_suspect_id
            .captures_iter(block_text)
            .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_lowercase()))
            .collect();
        suspect_ids.extend(
            Regex::new(r"\(([\w.\-]+)\)")
                .unwrap()
                .captures_iter(block_text)
                .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_owned())),
        );
        files.extend(files_for_ids(&suspect_ids, &by_id, mods));
    }
    for captures in patterns().crash_mod_file.captures_iter(report) {
        if let Some(name) = captures.get(1) {
            if let Some(file) = by_file.get(&name.as_str().to_lowercase()) {
                files.push(file.clone());
            }
        }
    }
    let vanilla_prefix = Regex::new(r"^(?:minecraft|client|forge|neoforge|fabric-loader|quilt-loader)[-.]").unwrap();
    unique(files)
        .into_iter()
        .filter(|file| !vanilla_prefix.is_match(&file.to_lowercase()))
        .take(10)
        .collect()
}

fn broken_configs(text: &str, instance_path: Option<&Path>) -> Vec<String> {
    let Some(instance_path) = instance_path else {
        return Vec::new();
    };
    let root = instance_path.join("config");
    if root.symlink_metadata().map(|meta| meta.is_symlink() || !meta.is_dir()).unwrap_or(true) {
        return Vec::new();
    }
    let mut candidates: Vec<String> = patterns()
        .config_path
        .captures_iter(text)
        .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_owned()))
        .collect();
    candidates.extend(
        patterns()
            .forge_config_file
            .captures_iter(text)
            .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_owned())),
    );
    let mut found: Vec<String> = Vec::new();
    for candidate in candidates {
        let relative = candidate.replace('\\', "/");
        let relative = relative.trim_start_matches('/').to_owned();
        let file_name = relative.rsplit('/').next().unwrap_or_default().to_owned();
        let path = match safe_child(&root, &relative)
            .filter(|path| path.is_file())
            .or_else(|| safe_child(&root, &file_name).filter(|path| path.is_file()))
        {
            Some(path) => path,
            None => continue,
        };
        let suffix = path.strip_prefix(&root).unwrap_or(&path);
        found.push(format!("config/{}", suffix.to_string_lossy().replace('\\', "/")));
    }
    unique(found)
}

/// моды, код которых находится в стеке рядом с ошибкой песочницы
fn stack_culprits(text: &str, pattern: &Regex, mods: &[ModEntry]) -> Vec<String> {
    if text.is_empty() || mods.is_empty() {
        return Vec::new();
    }
    let by_id: BTreeMap<String, usize> = mods
        .iter()
        .enumerate()
        .filter(|(_, mod_entry)| !mod_entry.mod_id.is_empty())
        .map(|(index, mod_entry)| (mod_entry.mod_id.clone(), index))
        .collect();
    let lines: Vec<&str> = text.lines().collect();
    let mut files: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !pattern.is_match(line) {
            continue;
        }
        let window: Vec<&str> = lines[index..(index + 60).min(lines.len())].to_vec();
        let window_text = window.join("\n");
        for captures in patterns().mixin_handler.captures_iter(&window_text) {
            if let Some(mod_id) = captures.get(1) {
                files.extend(files_for_ids(&[mod_id.as_str().to_lowercase()], &by_id, mods));
            }
        }
        for captures in patterns().stack_frame.captures_iter(&window_text) {
            let Some(class_name) = captures.get(1).map(|m| m.as_str()) else { continue };
            if FRAMEWORK_PACKAGES.iter().any(|prefix| class_name.starts_with(prefix)) {
                continue;
            }
            let path = class_name.replace('.', "/");
            let parts: Vec<&str> = path.split('/').collect();
            let mut prefixes: Vec<String> = Vec::new();
            if parts.len() > 3 {
                prefixes.push(parts[..3].join("/"));
            }
            if parts.len() > 2 {
                prefixes.push(parts[..2].join("/"));
            }
            for mod_entry in mods {
                if prefixes.iter().any(|prefix| mod_entry.packages.contains(prefix)) {
                    files.push(mod_entry.file.clone());
                    break;
                }
            }
        }
        if unique(files.iter().cloned()).len() >= 4 {
            break;
        }
    }
    let trimmed = unique(files);
    trimmed[..6.min(trimmed.len())].to_vec()
}


#[derive(Default)]
struct IssueSink {
    issues: Vec<(String, Map<String, Value>)>,
}

impl IssueSink {
    fn add(
        &mut self,
        kind: &str,
        detail: &str,
        fixes: Vec<Option<Value>>,
        evidence: Vec<String>,
        title: &str,
        manual: &str,
    ) {
        let fixes: Vec<Value> = fixes.into_iter().flatten().collect();
        if fixes.is_empty() && manual.is_empty() {
            return;
        }
        if let Some(position) = self.issues.iter().position(|(existing, _)| existing == kind) {
            // повторная улика того же типа дополняет запись, а не создаёт новую
            let (_, existing) = &mut self.issues[position];
            let mut merged_evidence: Vec<String> = existing
                .get("evidence")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            merged_evidence.extend(evidence);
            merged_evidence = unique(merged_evidence);
            merged_evidence.truncate(MAX_EVIDENCE);
            existing.insert("evidence".to_owned(), json!(merged_evidence));
            if (kind == "account" || kind == "security_sandbox") && !detail.is_empty() {
                let current_detail = existing.get("detail").and_then(Value::as_str).unwrap_or_default().to_owned();
                if !current_detail.contains(detail) {
                    existing.insert("detail".to_owned(), json!(format!("{current_detail} {detail}")));
                }
                if !manual.is_empty() {
                    let current_manual = existing.get("manual").and_then(Value::as_str).unwrap_or_default().to_owned();
                    if !current_manual.contains(manual) {
                        let joined = format!("{current_manual} {manual}").trim().to_owned();
                        existing.insert("manual".to_owned(), json!(joined));
                    }
                }
            }
            let existing_fixes = existing
                .get_mut("fixes")
                .and_then(Value::as_array_mut)
                .expect("issue всегда содержит fixes");
            for fix in fixes {
                let action = fix.get("action").and_then(Value::as_str).unwrap_or_default().to_owned();
                let params = fix.get("params").cloned();
                let mut position = existing_fixes
                    .iter()
                    .position(|item| item.get("action").and_then(Value::as_str) == Some(&action) && item.get("params") == params.as_ref());
                if position.is_none() && (action == "disable_mods" || action == "install_dependencies") {
                    position = existing_fixes.iter().position(|item| item.get("action").and_then(Value::as_str) == Some(&action));
                }
                match position {
                    None => {
                        let mut fix = fix;
                        fix["id"] = json!(format!("{kind}:{action}:{}", existing_fixes.len() + 1));
                        fix["recommended"] = json!(false);
                        existing_fixes.push(fix);
                    }
                    Some(index) if action == "disable_mods" => {
                        let target = &mut existing_fixes[index];
                        let mut files: Vec<String> = target
                            .pointer("/params/files")
                            .and_then(Value::as_array)
                            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                            .unwrap_or_default();
                        files.extend(
                            fix.pointer("/params/files")
                                .and_then(Value::as_array)
                                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>())
                                .unwrap_or_default(),
                        );
                        files = unique(files);
                        files.truncate(MAX_FILES_PER_FIX);
                        target["params"]["files"] = json!(files);
                        let files_value = target["params"]["files"].clone();
                        let file_names: Vec<String> = files_value
                            .as_array()
                            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                            .unwrap_or_default();
                        target["steps"] = json!(disable_steps(&file_names));
                    }
                    Some(index) if action == "install_dependencies" => {
                        let target = &mut existing_fixes[index];
                        let mut ids: Vec<String> = target
                            .pointer("/params/ids")
                            .and_then(Value::as_array)
                            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                            .unwrap_or_default();
                        ids.extend(
                            fix.pointer("/params/ids")
                                .and_then(Value::as_array)
                                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>())
                                .unwrap_or_default(),
                        );
                        ids = unique(ids);
                        ids.truncate(MAX_FILES_PER_FIX);
                        target["params"]["ids"] = json!(ids);
                        let ids_value = target["params"]["ids"].clone();
                        let id_list: Vec<String> = ids_value
                            .as_array()
                            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                            .unwrap_or_default();
                        let shown = id_list.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
                        target["steps"] = json!([
                            format!("Найти в каталоге Modrinth и установить: {shown}"),
                            "Берётся версия под Minecraft и загрузчик этого экземпляра; ненайденные зависимости будут перечислены.".to_owned(),
                        ]);
                    }
                    Some(_) => {}
                }
            }
            return;
        }
        let mut used_ids: BTreeSet<String> = BTreeSet::new();
        let fixes: Vec<Value> = fixes
            .into_iter()
            .enumerate()
            .map(|(index, mut fix)| {
                // у проблемы могут быть разные починки одного типа, поэтому `id` должен быть уникальным
                let action = fix.get("action").and_then(Value::as_str).unwrap_or_default().to_owned();
                let mut fix_id = format!("{kind}:{action}");
                let mut suffix = 2;
                while used_ids.contains(&fix_id) {
                    fix_id = format!("{kind}:{action}:{suffix}");
                    suffix += 1;
                }
                used_ids.insert(fix_id.clone());
                fix["id"] = json!(fix_id);
                fix["recommended"] = json!(index == 0);
                fix
            })
            .collect();
        let mut issue = Map::new();
        issue.insert("id".to_owned(), json!(kind));
        issue.insert(
            "title".to_owned(),
            json!(if title.is_empty() { issue_title(kind) } else { title.to_owned() }),
        );
        issue.insert("detail".to_owned(), json!(detail));
        let mut trimmed_evidence = unique(evidence);
        trimmed_evidence.truncate(MAX_EVIDENCE);
        issue.insert("evidence".to_owned(), json!(trimmed_evidence));
        issue.insert("fixes".to_owned(), Value::Array(fixes));
        issue.insert("manual".to_owned(), json!(manual));
        self.issues.push((kind.to_owned(), issue));
    }

    fn has(&self, kind: &str) -> bool {
        self.issues.iter().any(|(existing, _)| existing == kind)
    }
}


fn account_label(account: &Value) -> String {
    let name = account
        .get("alias")
        .or_else(|| account.get("username"))
        .or_else(|| account.get("account_id"))
        .and_then(Value::as_str)
        .unwrap_or("аккаунт");
    let kind = if account.get("kind").and_then(Value::as_str) == Some("microsoft") {
        "Microsoft"
    } else {
        "офлайн"
    };
    format!("{name} ({kind})")
}

fn set_account_fix(account: &Value, reason: &str) -> Value {
    let mut steps = vec![format!("Аккаунт по умолчанию → {}", account_label(account))];
    if !reason.is_empty() {
        steps.push(reason.to_owned());
    }
    steps.push("Вернуть прежний можно на вкладке «Аккаунты».".to_owned());
    json!({
        "action": "set_account",
        "label": format!("Запускать от аккаунта {}", account_label(account)),
        "steps": steps,
        "params": {"account_id": account.get("account_id").and_then(Value::as_str).unwrap_or_default()},
    })
}

fn login_fix(label: &str) -> Value {
    json!({
        "action": "login_microsoft",
        "label": label,
        "steps": [
            "Откроется вход Microsoft по коду устройства — пароль вводится только на сайте Microsoft.".to_owned(),
            "Новые токены сохранятся в системном хранилище и заменят устаревшие.".to_owned(),
        ],
        "params": {},
    })
}

fn xsts_hint(code: &str) -> String {
    match code {
        "2148916233" => "У этого Microsoft-аккаунта нет профиля Xbox. Войдите на xbox.com, создайте профиль и повторите вход.".to_owned(),
        "2148916235" => "Xbox Live недоступен в стране аккаунта. Смените регион аккаунта Microsoft или используйте другой аккаунт.".to_owned(),
        "2148916236" | "2148916237" => "Аккаунту нужна проверка возраста на xbox.com — пройдите её и повторите вход.".to_owned(),
        "2148916238" => "Это детский аккаунт: взрослый член семьи Microsoft должен добавить его в семью и разрешить игру.".to_owned(),
        other => other.to_owned(),
    }
}

fn diagnose_account(sink: &mut IssueSink, combined: &str, accounts: &[Value], current_account_id: &str) {
    let known: Vec<&Value> = accounts
        .iter()
        .filter(|item| item.get("account_id").and_then(Value::as_str).map(|id| !id.is_empty()).unwrap_or(false))
        .collect();
    let current = if current_account_id.is_empty() {
        known
            .iter()
            .find(|item| item.get("is_default").and_then(Value::as_bool) == Some(true))
            .copied()
    } else {
        known
            .iter()
            .find(|item| item.get("account_id").and_then(Value::as_str) == Some(current_account_id))
            .copied()
    };
    let current_id = current.and_then(|item| item.get("account_id")).and_then(Value::as_str).unwrap_or_default().to_owned();
    let others: Vec<&Value> = known
        .iter()
        .copied()
        .filter(|item| item.get("account_id").and_then(Value::as_str) != Some(current_id.as_str()))
        .collect();
    let microsoft_others: Vec<&Value> = others
        .iter()
        .copied()
        .filter(|item| item.get("kind").and_then(Value::as_str) == Some("microsoft"))
        .collect();

    if let Some(xsts) = patterns().account_xsts.captures(combined) {
        let code = xsts.get(1).map(|m| m.as_str()).unwrap_or_default();
        let mut fixes: Vec<Option<Value>> = microsoft_others.iter().take(2).map(|item| Some(set_account_fix(item, ""))).collect();
        fixes.push(Some(login_fix("Войти другим аккаунтом Microsoft")));
        sink.add(
            "account",
            &format!("Xbox Live отклонил вход (код {code})."),
            fixes,
            evidence_lines(combined, &["XSTS"]),
            "Xbox Live не пустил аккаунт",
            &xsts_hint(code),
        );
    }
    if patterns().account_no_license.is_match(combined) {
        let mut fixes: Vec<Option<Value>> = microsoft_others.iter().take(2).map(|item| Some(set_account_fix(item, ""))).collect();
        fixes.push(Some(login_fix("Войти аккаунтом, на котором куплена игра")));
        sink.add(
            "account",
            "У аккаунта Microsoft не найдена лицензия Minecraft: Java Edition.",
            fixes,
            evidence_lines(combined, &["право", "own", "profile", "Minecraft Services"]),
            "Нет лицензии Minecraft",
            "Игру нужно купить на minecraft.net тем же аккаунтом Microsoft. Game Pass даёт лицензию, только пока подписка активна.",
        );
    }
    if let Some(missing) = patterns().account_missing.captures(combined) {
        let mut fixes: Vec<Option<Value>> = Vec::new();
        let fallback = microsoft_others
            .first()
            .copied()
            .or_else(|| others.first().copied());
        if let Some(fallback) = fallback {
            fixes.push(Some(set_account_fix(fallback, "Выбранный аккаунт удалён или недоступен — экземпляр получит рабочий.")));
        }
        fixes.push(Some(login_fix("Войти через Microsoft")));
        let named = missing.get(1).map(|m| m.as_str()).filter(|value| !value.is_empty());
        let detail = format!(
            "Аккаунт, выбранный для запуска, не найден{}: его удалили или он ещё не создан.",
            named.map(|name| format!(" ({name})")).unwrap_or_default()
        );
        sink.add(
            "account",
            &detail,
            fixes,
            evidence_lines(combined, &["Аккаунт", "аккаунт"]),
            "Аккаунт для запуска не найден",
            "",
        );
    }
    let storage = patterns().account_storage.is_match(combined);
    if patterns().account_relogin.is_match(combined) || storage {
        let offline_on_server = current
            .map(|item| item.get("kind").and_then(Value::as_str) == Some("offline"))
            .unwrap_or(false)
            && Regex::new(r"(?i)Failed to verify username|Invalid session").unwrap().is_match(combined);
        let mut fixes: Vec<Option<Value>> = Vec::new();
        if offline_on_server {
            fixes.extend(
                microsoft_others
                    .iter()
                    .take(2)
                    .map(|item| Some(set_account_fix(item, "Онлайн-серверы пускают только лицензионные аккаунты."))),
            );
            fixes.push(Some(login_fix("Войти через Microsoft")));
        } else {
            fixes.push(Some(login_fix("Войти в Microsoft заново")));
            fixes.extend(others.iter().take(2).map(|item| Some(set_account_fix(item, ""))));
        }
        let manual = if storage {
            "Токены хранятся в системном хранилище паролей (Credential Manager / gnome-keyring / KWallet). Если оно не запущено или заблокировано, вход придётся повторять — проверьте, что хранилище работает.".to_owned()
        } else if offline_on_server {
            "Офлайн-аккаунт подходит для одиночной игры и серверов с online-mode=false.".to_owned()
        } else {
            String::new()
        };
        let detail = if offline_on_server {
            "Сервер отклонил офлайн-аккаунт: для этого сервера нужна лицензия."
        } else {
            "Сессия Microsoft устарела или токены потеряны — Minecraft не может подтвердить аккаунт."
        };
        sink.add(
            "account",
            detail,
            fixes,
            evidence_lines(combined, &["token", "токен", "session", "verify", "login", "401", "хранилищ", "secret"]),
            if offline_on_server { "Офлайн-аккаунт не пускают на сервер" } else { "Нужно войти в аккаунт заново" },
            &manual,
        );
    }
    if patterns().account_multiplayer.is_match(combined) {
        sink.add(
            "account",
            "Сетевая игра для аккаунта отключена.",
            microsoft_others.iter().take(2).map(|item| Some(set_account_fix(item, ""))).collect(),
            evidence_lines(combined, &["multiplayer", "banned"]),
            "Сетевая игра запрещена для аккаунта",
            "Разрешите сетевую игру в настройках конфиденциальности Xbox (account.xbox.com → Конфиденциальность) или в настройках семьи Microsoft.",
        );
    }
}

fn security_fix(label: &str, steps: &[String], profile: &str, add: &[String], remove: &[String]) -> Value {
    let mut all_steps = steps.to_vec();
    all_steps.push("Изменение касается только этого экземпляра и откатывается в его настройках → «Безопасность».".to_owned());
    json!({
        "action": "set_security",
        "label": label,
        "steps": all_steps,
        "params": {"profile": profile, "add_permissions": add, "remove_permissions": remove},
    })
}

fn profile_fixes(current_profile: &str, include_compat: bool) -> Vec<Value> {
    let mut fixes: Vec<Value> = Vec::new();
    if include_compat && current_profile != "compat" {
        fixes.push(security_fix(
            "Ослабить защиту: профиль Compatibility",
            &[
                format!("Профиль безопасности: {current_profile} → compat"),
                "Игра запускается без песочницы, но с очищенным окружением лаунчера.".to_owned(),
            ],
            "compat",
            &[],
            &[],
        ));
    }
    if current_profile != "off" {
        fixes.push(security_fix(
            "Отключить защиту для экземпляра (Off)",
            &[
                format!("Профиль безопасности: {current_profile} → off"),
                "Только для доверенных сборок: моды получают те же права, что и вы.".to_owned(),
            ],
            "off",
            &[],
            &[],
        ));
    }
    fixes
}

fn diagnose_sandbox(
    sink: &mut IssueSink,
    combined: &str,
    report: &str,
    settings: &Map<String, Value>,
    mods: &[ModEntry],
    instance_path: Option<&Path>,
) {
    let profile = settings
        .get("security_profile")
        .and_then(Value::as_str)
        .unwrap_or("strict");
    let profile = if profile.trim().is_empty() { "strict" } else { profile };
    let profile = profile.trim().to_lowercase();
    let permissions: Vec<String> = settings
        .get("security_permissions")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_else(|| GAME_PERMISSIONS.iter().map(|item| item.to_string()).collect());

    let launcher_error = patterns().sandbox_launcher.captures(combined);
    if let Some(captures) = launcher_error {
        let text = captures.get(0).map(|m| m.as_str()).unwrap_or_default().to_owned();
        let mut fixes: Vec<Option<Value>> = Vec::new();
        let unsupported: Vec<String> = permissions
            .iter()
            .filter(|item| super::security::UNENFORCED_CELL_PERMISSIONS.contains(&item.as_str()))
            .cloned()
            .collect();
        let host: Vec<String> = permissions
            .iter()
            .filter(|item| *item == "files:host-read" || *item == "files:host-write")
            .cloned()
            .collect();
        if text.contains("не может безопасно выдать") && !unsupported.is_empty() {
            fixes.push(Some(security_fix(
                "Убрать разрешения, которые песочница не умеет выдавать",
                &[format!("Снять: {}", unsupported.join(", ")), format!("Профиль {profile} остаётся включённым.")],
                "",
                &[],
                &unsupported,
            )));
        } else if (text.contains("host-read") || text.contains("запись в файлы компьютера")) && !host.is_empty() {
            fixes.push(Some(security_fix(
                "Закрыть модам доступ к файлам компьютера",
                &[format!("Снять: {}", host.join(", ")), format!("Профиль {profile} остаётся включённым.")],
                "",
                &[],
                &host,
            )));
        }
        fixes.extend(profile_fixes(&profile, true).into_iter().map(Some));
        let mut evidence = vec![truncate_chars(text.trim(), 300)];
        evidence.extend(evidence_lines(combined, &["sandbox", "песочниц", "bwrap", "AppContainer"]));
        sink.add(
            "security_sandbox",
            &format!("Песочница профиля «{profile}» не смогла запустить игру с текущими разрешениями."),
            fixes,
            evidence,
            "",
            if text.contains("bubblewrap") { "Для Linux-профиля Strict нужен пакет bubblewrap (bwrap)." } else { "" },
        );
        return;
    }
    if !SANDBOXED_PROFILES.contains(&profile.as_str()) {
        return;
    }

    let culprits = |pattern: &Regex| -> Vec<String> {
        // стек приоритетнее списка подозреваемых из отчёта о сбое
        let from_stack = stack_culprits(combined, pattern, mods);
        if from_stack.is_empty() {
            crash_report_suspects(report, mods)
        } else {
            from_stack
        }
    };
    let disable_with_note = |files: &[String], label: &str| -> Option<Value> {
        disable_fix(files, label).map(|mut fix| {
            if let Some(steps) = fix.get_mut("steps").and_then(Value::as_array_mut) {
                steps.insert(
                    0,
                    json!("Песочница сработала как задумано: мод попытался сделать то, что этому экземпляру запрещено."),
                );
            }
            fix
        })
    };
    let grant = |permission: &str, label: &str, why: &str| -> Value {
        security_fix(
            label,
            &[format!("Выдать разрешение «{permission}» — {why}"), format!("Профиль {profile} остаётся включённым.")],
            "",
            &[permission.to_owned()],
            &[],
        )
    };

    if !permissions.contains(&"network:game".to_owned()) && patterns().sandbox_network.is_match(combined) {
        sink.add(
            "security_sandbox",
            "Сеть экземпляра выключена в настройках безопасности, поэтому игра или мод не смогли подключиться.",
            vec![
                Some(grant("network:game", "Включить сеть для игры", "доступ к серверам и интернету")),
                disable_with_note(&culprits(&patterns().sandbox_network), "Отключить мод, которому нужна сеть"),
            ],
            evidence_lines(combined, &["UnknownHost", "unreachable", "socket", "resolution", "host"]),
            "",
            "",
        );
    }
    if !permissions.contains(&"desktop:display".to_owned()) && patterns().sandbox_display.is_match(combined) {
        let mut fixes = vec![Some(grant("desktop:display", "Разрешить окно игры и GPU", "без него Minecraft не создаёт окно"))];
        fixes.extend(profile_fixes(&profile, true).into_iter().map(Some));
        sink.add(
            "security_sandbox",
            "Песочнице запрещён доступ к экрану и видеокарте — окно игры не может открыться.",
            fixes,
            evidence_lines(combined, &["DISPLAY", "display", "X11", "Wayland", "/dev/"]),
            "",
            "",
        );
    }
    if let Some(files_match) = patterns().sandbox_files.captures(combined) {
        let denied_path = files_match
            .get(1)
            .map(|m| m.as_str().trim())
            .unwrap_or("")
            .to_owned();
        let inside = !denied_path.is_empty() && path_inside_instance(&denied_path, instance_path);
        let files = culprits(&patterns().sandbox_files);
        let has_instance_files = permissions.contains(&"files:instance".to_owned());
        let mut fixes: Vec<Option<Value>> = Vec::new();
        if !has_instance_files {
            fixes.push(Some(grant("files:instance", "Разрешить запись в папку экземпляра", "моды сохраняют конфиги и миры")));
            fixes.push(disable_with_note(&files, "Отключить мод, упёршийся в запрет"));
        } else {
            fixes.push(disable_with_note(&files, "Отключить мод, который лезет за пределы экземпляра"));
            fixes.extend(profile_fixes(&profile, true).into_iter().map(Some));
        }
        let detail = if !has_instance_files {
            "Песочница запретила запись в папку экземпляра.".to_owned()
        } else {
            format!(
                "Мод обратился к файлам, к которым у экземпляра нет доступа{}.",
                if !denied_path.is_empty() && !inside { format!(": {}", truncate_chars(&denied_path, 160)) } else { String::new() }
            )
        };
        sink.add(
            "security_sandbox",
            &detail,
            fixes,
            evidence_lines(combined, &["AccessDenied", "Access is denied", "Permission denied", "Read-only", "Отказано"]),
            "",
            "",
        );
    }
    if let Some(process_match) = patterns().sandbox_process.captures(combined) {
        let program = process_match.get(1).map(|m| m.as_str()).unwrap_or_default();
        let is_java = Regex::new(r#"javaw?(?:\.exe)?"$"#).unwrap().is_match(program);
        if !is_java {
            let mut fixes = vec![disable_with_note(&culprits(&patterns().sandbox_process), "Отключить мод, запускающий программы")];
            fixes.extend(profile_fixes(&profile, true).into_iter().map(Some));
            sink.add(
                "security_sandbox",
                &format!("Мод попытался запустить внешнюю программу — профиль «{profile}» это запрещает."),
                fixes,
                evidence_lines(combined, &["Cannot run program", "CreateProcess", "error="]),
                "",
                "Разрешение «Запуск процессов» внутри песочницы не выдаётся: нужен профиль Compatibility или Off.",
            );
        }
    }
    if patterns().sandbox_native.is_match(combined) {
        let mut fixes = vec![disable_with_note(&culprits(&patterns().sandbox_native), "Отключить мод с нативным кодом")];
        fixes.extend(profile_fixes(&profile, true).into_iter().map(Some));
        sink.add(
            "security_sandbox",
            "Мод не смог загрузить нативную библиотеку из-за ограничений песочницы.",
            fixes,
            evidence_lines(combined, &["load library", "UnsatisfiedLink"]),
            "",
            "Разрешение «Нативный код» доступно только без песочницы (Compatibility или Off).",
        );
    }
}

/// оставляет известные виды проблем в установленном порядке
pub(crate) fn normalize_muted(value: &Value) -> Vec<String> {
    let items: BTreeSet<String> = value
        .as_array()
        .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    ISSUE_ORDER
        .iter()
        .filter(|(kind, _)| items.contains(*kind))
        .map(|(kind, _)| kind.to_string())
        .collect()
}

/// формирует отчёт о починке; пустой результат скрывает окно
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_report(
    config: &super::config::ConfigStore,
    workspace: &super::instance_workspace::InstanceWorkspace,
    info: &super::instances::InstanceInfo,
    lines: Vec<String>,
    exit_code: Option<i64>,
    launch_settings: &Map<String, Value>,
    error: &str,
    since: f64,
    manual: bool,
    account_id_hint: &str,
) -> Option<Map<String, Value>> {
    if !manual && !config.bool_setting("auto_repair_enabled") {
        return None;
    }
    if info.kind != "client" || info.game_edition != "java" {
        return None;
    }
    let instance_settings = workspace
        .settings(&info.name)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    // только JSON со списком: токены из системного хранилища не нужны
    let accounts = super::accounts::public_accounts(&config.paths.config_home).unwrap_or_default();
    let mut account_id = account_id_hint.trim().to_owned();
    if account_id.is_empty() {
        account_id = launch_settings
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if ["auto", "microsoft", "offline"].contains(&account_id.to_lowercase().as_str()) || account_id.is_empty() {
            account_id = config.str_setting("default_account_id");
        }
    }
    let muted: Vec<String> = if manual {
        Vec::new()
    } else {
        config
            .settings_public()
            .get("auto_repair_muted")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    };
    let input = diagnose::DiagnoseInput {
        lines,
        exit_code,
        instance_path: Some(info.path.as_path()),
        instance_settings,
        effective_memory: launch_settings.get("memory").and_then(Value::as_str).unwrap_or_default().to_owned(),
        loader: info.loader.clone(),
        minecraft_version: info.minecraft_version.clone(),
        error: truncate_chars(error, 4000),
        since,
        muted,
        crash_report: None,
        loader_version: info.loader_version.clone(),
        accounts,
        current_account_id: if account_id.eq_ignore_ascii_case("auto") { String::new() } else { account_id },
    };
    let mut report = diagnose::diagnose(&input);
    report.insert("instance_name".to_owned(), json!(info.name));
    report.insert("manual".to_owned(), json!(manual));
    Some(report)
}


pub(crate) fn auto_repair_diagnose(request: &Value) -> Result<Value, String> {
    let name = request.get("instance_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let manual = request.get("manual").and_then(Value::as_bool) == Some(true);
    let config = super::config::ConfigStore::load()?;
    if !manual && !config.bool_setting("auto_repair_enabled") {
        return Ok(json!({
            "instance_name": name,
            "issues": [],
            "muted_issues": [],
            "disabled": true,
        }));
    }
    let store = super::instances::InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    let (lines, running) = super::launcher::session_snapshot(&info.name);
    if running {
        return Err("Экземпляр ещё запущен — диагностика будет доступна после выхода из игры".to_owned());
    }
    let exit_code = request.get("exit_code").and_then(Value::as_i64);
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    let launch_settings = super::launcher::effective_launch_settings(&workspace, &config, Some(&info.name), &json!({}))
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let since = super::launcher::session_started_at(&info.name);
    let error = request.get("error").and_then(Value::as_str).unwrap_or_default();
    let report_lines = if exit_code.is_some() || !error.is_empty() { lines } else { Vec::new() };
    let report = build_report(
        &config,
        &workspace,
        &info,
        report_lines,
        exit_code,
        &launch_settings,
        error,
        since,
        manual,
        "",
    )
    .ok_or_else(|| "Авто-починка доступна только для клиентских Java-экземпляров".to_owned())?;
    Ok(Value::Object(report))
}

pub(crate) fn auto_repair_mute(request: &Value) -> Result<Value, String> {
    let kinds = normalize_muted(request.get("kinds").unwrap_or(&Value::Null));
    let mut config = super::config::ConfigStore::load()?;
    let muted_flag = request.get("muted").and_then(Value::as_bool) != Some(false);
    let current: Vec<String> = config
        .settings_public()
        .get("auto_repair_muted")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let updated: Vec<String> = if muted_flag {
        unique(current.into_iter().chain(kinds))
    } else {
        current.into_iter().filter(|kind| !kinds.contains(kind)).collect()
    };
    config.set_values(&json!({"auto_repair_muted": updated}))?;
    Ok(json!({"settings": config.settings_public()}))
}

#[cfg(test)]
use apply::{pick_loader_version, string_list};

/// применяет выбранные исправления к остановленному экземпляру Java
pub fn auto_repair_apply(request: &Value) -> Result<Value, String> {
    apply::auto_repair_apply(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// минимальный jar-мод: манифест + класс + конфиг миксина
    fn sample_mod(path: &Path, mod_id: &str) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("fabric.mod.json", options).unwrap();
        writer
            .write_all(format!("{{\"id\": \"{mod_id}\", \"name\": \"Mod {mod_id}\", \"version\": \"1.0.0\"}}").as_bytes())
            .unwrap();
        writer.start_file("com/example/some/Mod.class", options).unwrap();
        writer.write_all(b"\xca\xfe\xba\xbe").unwrap();
        writer.start_file(format!("{mod_id}.mixins.json"), options).unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap();
    }

    fn scan_root(tag: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("nivora-scan-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("mods")).unwrap();
        directory
    }

    #[test]
    fn scan_mods_reads_loaders_mixins_and_state() {
        let root = scan_root("full");
        sample_mod(&root.join("mods/Sodium.jar"), "sodium");
        sample_mod(&root.join("mods/terralith.jar.disabled"), "terralith");
        fs::write(root.join("mods/broken.jar"), b"not a zip at all").unwrap();
        fs::write(root.join("mods/readme.txt"), b"skip me").unwrap();
        let mods = scan_mods(&root);
        assert_eq!(mods.len(), 3, "txt и подпапки не считаются модами: {mods:?}");
        let sodium = mods.iter().find(|item| item.file == "Sodium.jar").unwrap();
        assert!(sodium.enabled);
        assert_eq!(sodium.mod_id, "sodium");
        assert!(sodium.loaders.contains("fabric"));
        assert!(sodium.mixins.contains("sodium.mixins.json"));
        assert!(sodium.packages.contains("com/example/some"));
        assert!(!sodium.corrupted);
        let disabled = mods.iter().find(|item| item.file == "terralith.jar.disabled").unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.mod_id, "", "выключенные моды без manifest-полей");
        let broken = mods.iter().find(|item| item.file == "broken.jar").unwrap();
        assert!(broken.corrupted);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_mods_handles_missing_directory() {
        assert!(scan_mods(Path::new("/definitely-not-here")).is_empty());
    }

    #[test]
    fn wrong_loader_respects_connector_exception() {
        let mut fabric_mod = ModEntry {
            file: "fabricmod.jar".to_owned(),
            mod_id: "x".to_owned(),
            enabled: true,
            loaders: BTreeSet::from(["fabric".to_owned()]),
            mixins: BTreeSet::new(),
            packages: BTreeSet::new(),
            dependencies: Vec::new(),
            mtime: 0.0,
            corrupted: false,
        };
        assert!(wrong_loader(&fabric_mod, "forge", false));
        assert!(!wrong_loader(&fabric_mod, "forge", true), "Connector спасает Fabric-моды на Forge");
        assert!(!wrong_loader(&fabric_mod, "quilt", false), "Quilt запускает Fabric-моды");
        assert!(!wrong_loader(&fabric_mod, "fabric", false));
        assert!(!wrong_loader(&fabric_mod, "vanilla", false));
        fabric_mod.loaders.clear();
        assert!(!wrong_loader(&fabric_mod, "fabric", false), "без манифеста приговор не выносим");
        fabric_mod.corrupted = true;
        assert!(!wrong_loader(&fabric_mod, "fabric", false));
    }

    #[test]
    fn crash_report_takes_newest_txt() {
        let root = scan_root("crash");
        fs::create_dir_all(root.join("crash-reports")).unwrap();
        fs::write(root.join("crash-reports/old.txt"), b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(root.join("crash-reports/new.txt"), b"newest report").unwrap();
        fs::write(root.join("crash-reports/ignored.log"), b"no").unwrap();
        let report = latest_crash_report(&root, 0.0);
        assert_eq!(report, "newest report");
        assert_eq!(latest_crash_report(&root, f64::MAX - 1.0), "");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_state_reads_as_empty() {
        assert!(read_state(Path::new("/definitely-not-here")).is_empty());
    }

    #[test]
    fn launch_result_is_recorded_only_for_success() {
        let directory = std::env::temp_dir().join(format!("nivora-repair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        record_launch_result(Some(&directory), Some(1));
        assert!(!directory.join(STATE_FILE).exists());
        record_launch_result(Some(&directory), None);
        assert!(!directory.join(STATE_FILE).exists());
        record_launch_result(Some(&directory), Some(0));
        let state = read_state(&directory);
        let last_success = state.get("last_success").and_then(Value::as_f64).unwrap_or(0.0);
        assert!(last_success > 0.0, "last_success не записан: {state:?}");
        let path = directory.join(STATE_FILE);
        let mut manual = read_state(&directory);
        manual.insert("custom".to_owned(), Value::from(42));
        fs::write(&path, serde_json::to_string(&Value::Object(manual)).unwrap()).unwrap();
        record_launch_result(Some(&directory), Some(0));
        let state = read_state(&directory);
        assert_eq!(state.get("custom"), Some(&Value::from(42)));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn symlinked_state_is_never_touched() {
        let directory = std::env::temp_dir().join(format!("nivora-repair-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        #[cfg(unix)]
        {
            let outside = directory.join("outside.json");
            fs::write(&outside, "{}").unwrap();
            std::os::unix::fs::symlink(&outside, directory.join(STATE_FILE)).unwrap();
            record_launch_result(Some(&directory), Some(0));
            assert_eq!(fs::read_to_string(&outside).unwrap(), "{}");
        }
        let _ = fs::remove_dir_all(&directory);
    }

    fn diagnose_input(lines: &[&str], loader: &str) -> diagnose::DiagnoseInput<'static> {
        diagnose::DiagnoseInput {
            lines: lines.iter().map(|line| line.to_string()).collect(),
            exit_code: Some(1),
            instance_path: None,
            instance_settings: Map::new(),
            effective_memory: "2048M".to_owned(),
            loader: loader.to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            error: String::new(),
            since: 0.0,
            muted: Vec::new(),
            crash_report: Some(String::new()),
            loader_version: String::new(),
            accounts: Vec::new(),
            current_account_id: String::new(),
        }
    }

    fn issue_kinds(report: &Map<String, Value>) -> Vec<String> {
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["id"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn diagnose_reports_fabric_missing_dependency() {
        let input = diagnose_input(
            &["Mod 'Create' (create) 1.20.1 requires ^0.92.0+1.20.1 of mod 'Fabric API' (fabric-api), which is missing"],
            "fabric",
        );
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["missing_dependencies"]);
        let issue = &report["issues"][0];
        assert!(issue["detail"].as_str().unwrap().contains("fabric-api"));
        assert!(issue["detail"].as_str().unwrap().contains("create"));
        let install = &issue["fixes"][0];
        assert_eq!(install["action"], "install_dependencies");
        assert_eq!(install["params"]["ids"], json!(["fabric-api"]));
        assert_eq!(install["id"], "missing_dependencies:install_dependencies");
        assert_eq!(install["recommended"], true);
        assert_eq!(report["exit_code"], json!(1));
    }

    #[test]
    fn diagnose_reports_outdated_loader_with_minimum() {
        let input = diagnose_input(
            &["Mod 'Create' (create) requires 0.16.10 of fabricloader, but only the wrong version is present"],
            "fabric",
        );
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["loader_outdated"]);
        let issue = &report["issues"][0];
        assert!(issue["detail"].as_str().unwrap().contains("Fabric 0.16.10"));
        let update = &issue["fixes"][0];
        assert_eq!(update["action"], "update_loader");
        assert_eq!(update["params"]["minimum"], "0.16.10");
    }

    #[test]
    fn diagnose_out_of_memory_suggests_more_memory() {
        let mut input = diagnose_input(&["java.lang.OutOfMemoryError: Java heap space"], "vanilla");
        input.effective_memory = "1024M".to_owned();
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["out_of_memory"]);
        let issue = &report["issues"][0];
        assert!(issue["detail"].as_str().unwrap().contains("не хватило"));
        let ceiling = super::super::config::system_memory_kib() * 3 / 4;
        match issue["fixes"][0].as_object() {
            Some(fix) => {
                assert_eq!(fix["action"], "set_memory");
                let target = super::super::config::parse_memory_kib(fix["params"]["memory"].as_str().unwrap()).unwrap();
                // Предлагаемая память не должна превышать 3/4 доступной RAM
                assert!(target <= ceiling.max(1024 * 1024), "нельзя предлагать больше доступной памяти");
                assert!(target > 1024 * 1024, "память должна вырасти, а не уменьшиться");
            }
            None => {
                // Памяти в системе не хватает на рост: должна быть ручная инструкция
                let available = super::super::config::system_memory_kib();
                let target_floor = 4 * 1024 * 1024;
                assert!(
                    ceiling < target_floor || available < 2 * 1024 * 1024,
                    "фикс обязан быть, если память есть (доступно {available} KiB)"
                );
                assert!(!issue["manual"].as_str().unwrap_or_default().is_empty());
            }
        }
    }

    #[test]
    fn diagnose_memory_too_high_cuts_back() {
        let mut input = diagnose_input(&["Error occurred during initialization of VM", "Could not reserve enough space for 16384MiB heap"], "vanilla");
        input.effective_memory = "16384M".to_owned();
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["memory_too_high"]);
        let fix = &report["issues"][0]["fixes"][0];
        let target = super::super::config::parse_memory_kib(fix["params"]["memory"].as_str().unwrap()).unwrap();
        assert!(target < 16384 * 1024);
    }

    #[test]
    fn diagnose_account_relogin_offers_login() {
        let mut input = diagnose_input(&["Microsoft-токен истёк: Повторите login microsoft"], "vanilla");
        input.accounts = vec![json!({"account_id": "ms-1", "kind": "microsoft", "username": "Steve", "alias": "Steve", "is_default": true})];
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["account"]);
        let actions: Vec<&str> = report["issues"][0]["fixes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|fix| fix["action"].as_str().unwrap())
            .collect();
        assert_eq!(actions, ["login_microsoft"]);
        assert!(report["issues"][0]["title"].as_str().unwrap().contains("войти"));
    }

    #[test]
    fn diagnose_sandbox_network_grants_permission() {
        let mut input = diagnose_input(&["java.net.UnknownHostException: api.minecraftservices.net"], "vanilla");
        input
            .instance_settings
            .insert("security_profile".to_owned(), json!("strict"));
        input
            .instance_settings
            .insert("security_permissions".to_owned(), json!(["files:instance"]));
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["security_sandbox"]);
        let grant = &report["issues"][0]["fixes"][0];
        assert_eq!(grant["action"], "set_security");
        assert_eq!(grant["params"]["add_permissions"], json!(["network:game"]));
        assert_eq!(grant["params"]["profile"], "");
    }

    #[test]
    fn diagnose_mixin_failure_names_mod_from_folder() {
        let root = scan_root("mixin-diag");
        sample_mod(&root.join("mods").join("abc.jar"), "abc");
        let mut input = diagnose_input(
            &["Mixed: com.example.some.Mod failed", "Mixin apply for mod abc failed", "Critical injection failure"],
            "fabric",
        );
        input.instance_path = Some(root.as_path());
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["mixin_failure"]);
        let issue = &report["issues"][0];
        assert!(issue["detail"].as_str().unwrap().contains("Виновник: abc.jar"));
        let disable = &issue["fixes"][0];
        assert_eq!(disable["action"], "disable_mods");
        assert_eq!(disable["params"]["files"], json!(["abc.jar"]));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnose_unknown_crash_falls_back_to_repair() {
        let input = diagnose_input(
            &["[15:01:01] [main/INFO]: Backend library: LWJGL version 3.3.1", "java.lang.IllegalStateException: boom"],
            "vanilla",
        );
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), ["unknown_crash"]);
        let fix = &report["issues"][0]["fixes"][0];
        assert_eq!(fix["action"], "repair_version");
        let evidence = report["issues"][0]["evidence"].as_array().unwrap();
        assert!(evidence.iter().any(|line| line.as_str().unwrap().contains("IllegalStateException")));
    }

    #[test]
    fn diagnose_muted_issues_move_to_separate_list() {
        let mut input = diagnose_input(&["java.lang.OutOfMemoryError: Java heap space"], "vanilla");
        input.effective_memory = "1024M".to_owned();
        input.muted = vec!["out_of_memory".to_owned()];
        let report = diagnose::diagnose(&input);
        assert_eq!(issue_kinds(&report), Vec::<String>::new());
        assert_eq!(report["muted_issues"], json!(["out_of_memory"]));
    }

    #[test]
    fn normalize_muted_keeps_canonical_order() {
        assert_eq!(
            normalize_muted(&json!(["unknown_crash", "account", "bogus"])),
            ["account", "unknown_crash"]
        );
        assert!(normalize_muted(&json!("not-a-list")).is_empty());
    }

    #[test]
    fn issue_sink_merges_disable_fixes_of_same_kind() {
        let mut sink = IssueSink::default();
        sink.add(
            "incompatible_mods",
            "первая улика",
            vec![disable_fix(&["a.jar".to_owned()], "Отключить")],
            vec!["line-a".to_owned()],
            "",
            "",
        );
        sink.add(
            "incompatible_mods",
            "вторая улика",
            vec![disable_fix(&["b.jar".to_owned()], "Отключить")],
            vec!["line-b".to_owned()],
            "",
            "",
        );
        let issue = &sink.issues[0].1;
        let fixes = issue["fixes"].as_array().unwrap();
        assert_eq!(fixes.len(), 1, "починки одного типа сливаются");
        assert_eq!(fixes[0]["params"]["files"], json!(["a.jar", "b.jar"]));
        assert!(fixes[0]["steps"][0].as_str().unwrap().contains("b.jar"));
        let evidence = issue["evidence"].as_array().unwrap();
        assert_eq!(evidence.len(), 2);
    }

    #[test]
    fn pick_loader_version_prefers_stable_and_minimum() {
        assert_eq!(
            pick_loader_version(&["0.16.9".into(), "0.17.0-beta".into(), "0.16.10".into()], ""),
            "0.16.10"
        );
        assert_eq!(
            pick_loader_version(&["0.16.9".into(), "0.17.0-beta".into()], "0.17"),
            "0.17.0-beta"
        );
        assert_eq!(pick_loader_version(&[], ""), "");
        assert_eq!(pick_loader_version(&["0.15.0".into()], "0.16"), "");
        assert_eq!(
            pick_loader_version(&["47.2.20".into(), "47.3.0".into()], ""),
            "47.3.0"
        );
    }

    #[test]
    fn version_key_compares_numbers_then_stability() {
        assert!(version_key("0.16.10") > version_key("0.16.9"));
        assert!(version_key("47.3.0") > version_key("47.2.20"));
        assert!(version_key("0.16.10") > version_key("0.16.10-rc.1"));
        assert_eq!(version_key("0.16.10").0, version_key("0.16.10-rc.1").0);
        assert!(is_prerelease("1.20.4-RC1"));
        assert!(!is_prerelease("1.20.4"));
    }

    #[test]
    fn apply_validates_request_shape() {
        assert_eq!(
            auto_repair_apply(&json!({ "fixes": [] })).unwrap_err(),
            "Сначала выберите экземпляр"
        );
    }

    #[test]
    fn string_list_rejects_non_arrays_and_caps_length() {
        assert_eq!(
            string_list(Some(&json!("x")), MAX_FILES_PER_FIX).unwrap_err(),
            "Параметры починки имеют неожиданный формат"
        );
        let items: Vec<Value> = (0..60).map(|index| json!(format!("f{index}.jar"))).collect();
        assert_eq!(string_list(Some(&Value::Array(items)), MAX_FILES_PER_FIX).unwrap().len(), MAX_FILES_PER_FIX);
    }
}
