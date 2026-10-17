// Minecraft Bedrock для всех поддерживаемых ОС — целиком в Rust.
//
// Linux   — встроенная среда BedrockOnLinux (профили Wine/GDK-Proton):
//           тот же протокол команд, что раньше жил в nivora_launcher/bedrock.py.
// Windows — ОФИЦИАЛЬНЫЙ «Minecraft for Windows» из Microsoft Store:
//           установка через winget с открытием страницы Store (платные
//           Xbox-продукты лицензирует только сам Store), запуск по
//           протоколу minecraft://, контент — прямо в com.mojang
//           установленного пакета. BedrockOnLinux/Wine не используется.
// macOS   — официальный Bedrock не выпускается: честный отказ с пояснением.
//
// Все операции регистрируются в NATIVE_OPS и обслуживаются без Python-воркера.
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::config::ConfigStore;
use super::instances::{InstanceInfo, InstanceStore, INSTANCE_METADATA_FILE};
use super::operations::CancellationToken;
use super::platform_support::{is_reparse_point, process_exists};
use super::process_guard::InstanceProcessLock;
use super::state::{launcher_roots, read_bounded_json, safe_instance_name, settings_game_dir};

// ─────────────────────────────────── Константы ─────────────────────────────

const BEDROCK_ON_LINUX_URL: &str = "https://github.com/Wyze3306/BedrockOnLinux";
const BEDROCK_ON_LINUX_BUNDLED_VERSION: &str = "2.2.9";
const BEDROCK_ON_LINUX_BUNDLED_SHA256: &str =
    "bb894a86ed19575635d63f11cef466b70da2b99a12a3bda8468386630c40ed5c";
const BUNDLED_PYZ_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Минимальная поддержка версии среды (2.2.0).
const MIN_BOL_VERSION: (u32, u32, u32) = (2, 2, 0);
/// glibc, ниже которого xodus-вход в Microsoft Store не работает.
const MIN_GLIBC_VERSION: (u32, u32, u32) = (2, 39, 0);

pub(crate) const MAX_CONTENT_UPLOAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_CONTENT_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_CONTENT_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_JOB_LOG_LINES: usize = 200;
const MAX_SESSION_LOG_LINES: usize = 500;
const MAX_LOG_ARCHIVE_BYTES: u64 = 5 * 1024 * 1024;

/// Внутрипрофильный путь BoL к данным игры (относительно профиля).
const BOL_CONTENT_ROOT_PARTS: &[&str] = &[
    "compatdata", "pfx", "drive_c", "users", "steamuser", "AppData", "Roaming",
    "Minecraft Bedrock", "Users",
];
const CONTENT_DIRECTORIES: &[(&str, &[&str])] = &[
    ("addons", &["behavior_packs"]),
    ("resourcepacks", &["resource_packs"]),
    ("worlds", &["minecraftWorlds", "world_templates"]),
    ("skins", &["skin_packs", "custom_skins"]),
];
const IMPORT_EXTENSIONS: &[&str] = &[".mcpack", ".mcaddon", ".mcworld", ".mctemplate", ".mcskin"];

/// Windows: официальные идентификаторы продуктов Microsoft Store.
/// ProductId — страница Store; PackageFamilyName — каталог данных игры.
struct WindowsChannel {
    product_id: &'static str,
    package_family: &'static str,
    package_name: &'static str,
    protocol: &'static str,
    aumid: &'static str,
    title: &'static str,
}

const WIN_CHANNEL_RELEASE: WindowsChannel = WindowsChannel {
    product_id: "9NBLGGH2JHXJ",
    package_family: "Microsoft.MinecraftUWP_8wekyb3d8bbwe",
    package_name: "Microsoft.MinecraftUWP",
    protocol: "minecraft://",
    aumid: "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App",
    title: "Minecraft for Windows",
};
const WIN_CHANNEL_PREVIEW: WindowsChannel = WindowsChannel {
    product_id: "9P5X4QVLC2XR",
    package_family: "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe",
    package_name: "Microsoft.MinecraftWindowsBeta",
    protocol: "minecraft-preview://",
    aumid: "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe!App",
    title: "Minecraft Preview for Windows",
};

fn win_channel(edition: &str) -> &'static WindowsChannel {
    if edition.eq_ignore_ascii_case("preview") {
        &WIN_CHANNEL_PREVIEW
    } else {
        &WIN_CHANNEL_RELEASE
    }
}

const STORE_PAGE_LINK_RU: &str = "https://apps.microsoft.com/detail/9NBLGGH2JHXJ";

fn version_re() -> &'static Regex {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bBedrockOnLinux\s+(\d+\.\d+(?:\.\d+)?)\b").unwrap());
    &RE
}
fn game_version_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9][A-Za-z0-9._+-]{0,63}\z").unwrap());
    &RE
}
fn version_heading_re() -> &'static Regex {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\(--mc\s+(release|preview)\)\s*\z").unwrap());
    &RE
}
fn version_line_re() -> &'static Regex {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*([0-9][A-Za-z0-9._+-]{0,63})(?:\s+(installed))?\s*$").unwrap());
    &RE
}
fn secret_line_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(access[_ -]?token|refresh[_ -]?token|client[_ -]?secret|authorization|device[_ -]?code)\b\s*[:=]\s*\S+",
        )
        .unwrap()
    });
    &RE
}
/// BedrockOnLinux пишет уровень в начало строки («error …»); среда может
/// завершиться с кодом 0 даже после собственных «error».
fn bol_error_line_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:error|critical|fatal)\b").unwrap());
    &RE
}
fn profile_name_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^nivora-[0-9a-f]{16}\z").unwrap());
    &RE
}
fn glibc_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d+\.\d+)\b").unwrap());
    &RE
}

// ─────────────────────────────────── Платформа ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BedrockPlatform {
    LinuxBol,
    WindowsOfficial,
    Unsupported,
}

pub fn bedrock_platform() -> BedrockPlatform {
    if cfg!(target_os = "linux") {
        BedrockPlatform::LinuxBol
    } else if cfg!(target_os = "windows") {
        BedrockPlatform::WindowsOfficial
    } else {
        BedrockPlatform::Unsupported
    }
}

fn platform_id() -> &'static str {
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => "linux-bedrockonlinux",
        BedrockPlatform::WindowsOfficial => "windows-official",
        BedrockPlatform::Unsupported => "unsupported",
    }
}

/// Версия «latest» допустима только для официального канала Windows:
/// Store всегда ставит последний выпуск, номер заранее неизвестен.
fn valid_game_version(version: &str) -> bool {
    game_version_re().is_match(version)
        || (bedrock_platform() == BedrockPlatform::WindowsOfficial && version == "latest")
}

// ─────────────────────────────────── Контекст ──────────────────────────────

struct Ctx {
    game_dir: PathBuf,
    data_home: PathBuf,
    cache_home: PathBuf,
}

fn context() -> Result<Ctx, String> {
    let roots = launcher_roots();
    let game_dir = settings_game_dir(&roots.config_home, &roots.data_home);
    Ok(Ctx { game_dir, data_home: roots.data_home, cache_home: roots.cache_home })
}

fn instance_store() -> Result<InstanceStore, String> {
    let config = ConfigStore::load()?;
    InstanceStore::new(&config)
}

fn bedrock_instance(store: &InstanceStore, name: &str) -> Result<InstanceInfo, String> {
    let info = store.get(name)?;
    if info.kind != "client" || info.game_edition != "bedrock" {
        return Err("Экземпляр не является Minecraft Bedrock".to_owned());
    }
    Ok(info)
}

// ─────────────────────────────── Мелкие хелперы ────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_secs_f64() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0)
}

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_millis()).unwrap_or(0)
}

fn random_hex(byte_count: usize) -> String {
    let mut buffer = vec![0u8; byte_count];
    #[cfg(unix)]
    if let Ok(mut file) = fs::File::open("/dev/urandom") {
        if file.read_exact(&mut buffer).is_ok() {
            return buffer.iter().map(|byte| format!("{byte:02x}")).collect();
        }
    }
    // Фолбэк: pid + время через SHA-256 (на Windows urandom недоступен напрямую).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.subsec_nanos())
        .unwrap_or(0);
    let seed = format!("{}:{}:{nanos}", std::process::id(), now_millis());
    let digest = Sha256::digest(seed.as_bytes());
    digest.iter().take(byte_count).map(|byte| format!("{byte:02x}")).collect()
}

fn redact_line(value: &str) -> String {
    let replaced = secret_line_re().replace_all(value, "${1}=[redacted]");
    replaced.chars().take(4000).collect()
}

fn version_tuple(value: &str) -> (u32, u32, u32) {
    let mut parts = value.split('.');
    let mut next = move || parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    let (a, b, c) = (next(), next(), next());
    (a, b, c)
}

fn write_private_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать каталог {}: {error}", path.display()))?;
    }
    if is_reparse_point(path) {
        return Err(format!("Путь {} является символьной ссылкой", path.display()));
    }
    let text = serde_json::to_string(value).map_err(|error| error.to_string())?;
    super::platform::create_private(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| format!("Не удалось записать {}: {error}", path.display()))?;
    super::platform::restrict_file(path);
    Ok(())
}

fn is_symlink_or_missing_dir(path: &Path) -> bool {
    is_reparse_point(path) || !path.is_dir()
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_value = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_value) {
        let candidate = directory.join(name);
        if let Ok(meta) = fs::metadata(&candidate) {
            if meta.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn request_field<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    let value = request
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if value.is_empty() && matches!(name, "name" | "edition" | "version" | "file_name") {
        return Err(format!("Поле «{name}» обязательно для этой операции"));
    }
    Ok(value)
}

/// CREATE_NO_WINDOW: вспомогательные утилиты Windows не должны мелькать консолью.
fn prepare_command(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Запуск с общим таймаутом; возвращает (код, stdout+stderr). None — таймаут.
fn run_timed(command: &mut Command, timeout: Duration) -> Option<(i32, String)> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut child = child;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let out_handle = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stdout).read_to_string(&mut text);
        text
    });
    let err_handle = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = format!(
                    "{}\n{}",
                    out_handle.join().unwrap_or_default(),
                    err_handle.join().unwrap_or_default()
                );
                return Some((status.code().unwrap_or(-1), output));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

// ───────────────────────────── Фоновые задания ─────────────────────────────
// Установка версии и вход в Store занимают минуты; они живут в фоновых нитях,
// а интерфейс опрашивает bedrock_setup_status / bedrock_store_login_status.

struct JobState {
    running: bool,
    started: bool,
    finished: bool,
    cancelled: bool,
    started_at: f64,
    operation_id: String,
    error: String,
    logs: Vec<String>,
    result: Option<Value>,
}

static JOBS: LazyLock<Mutex<HashMap<String, Arc<Mutex<JobState>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn job_key(kind: &str, name: &str) -> String {
    format!("{kind}:{name}")
}

fn job_snapshot(kind: &str, name: &str) -> Value {
    let key = job_key(kind, name);
    let map = JOBS.lock().unwrap();
    let Some(job) = map.get(&key) else {
        return json!({ "running": false, "finished": false, "started": false });
    };
    let state = job.lock().unwrap();
    let tail_start = state.logs.len().saturating_sub(40);
    json!({
        "kind": kind,
        "instance_name": name,
        "running": state.running,
        "started": state.started,
        "finished": state.finished,
        "cancelled": state.cancelled,
        "started_at": state.started_at,
        "error": state.error,
        "result": state.result.clone(),
        "logs_tail": state.logs[tail_start..],
    })
}

fn cancel_job(kind: &str, name: &str) -> bool {
    let key = job_key(kind, name);
    let operation_id = {
        let map = JOBS.lock().unwrap();
        let Some(job) = map.get(&key) else { return false };
        let state = job.lock().unwrap();
        if !state.running {
            return false;
        }
        state.operation_id.clone()
    };
    super::operations::request_cancel(&operation_id)
}

fn spawn_job<F>(kind: &str, name: &str, runner: F) -> Result<Value, String>
where
    F: FnOnce(Arc<Mutex<JobState>>, &CancellationToken) + Send + 'static,
{
    let key = job_key(kind, name);
    let job = {
        let mut map = JOBS.lock().unwrap();
        if let Some(existing) = map.get(&key) {
            if existing.lock().unwrap().running {
                return Ok(json!({ "started": true, "already_running": true }));
            }
        }
        let job = Arc::new(Mutex::new(JobState {
            running: true,
            started: true,
            finished: false,
            cancelled: false,
            started_at: now_secs_f64(),
            operation_id: format!("bedrock-{kind}-{}", random_hex(6)),
            error: String::new(),
            logs: Vec::new(),
            result: None,
        }));
        map.insert(key, Arc::clone(&job));
        job
    };
    std::thread::Builder::new()
        .name(format!("nivora-bedrock-{kind}"))
        .spawn(move || {
            let operation_id = job.lock().unwrap().operation_id.clone();
            let cancellation = CancellationToken::new(&operation_id);
            runner(Arc::clone(&job), &cancellation);
            cancellation.cleanup();
        })
        .map_err(|error| format!("Не удалось запустить фоновое задание: {error}"))?;
    Ok(json!({ "started": true, "already_running": false }))
}

fn job_log(job: &Arc<Mutex<JobState>>, line: &str) {
    let mut state = job.lock().unwrap();
    state.logs.push(line.to_owned());
    if state.logs.len() > MAX_JOB_LOG_LINES {
        let drop = state.logs.len() - MAX_JOB_LOG_LINES;
        state.logs.drain(..drop);
    }
}

fn job_logs(job: &Arc<Mutex<JobState>>) -> Vec<String> {
    job.lock().unwrap().logs.clone()
}

fn job_finish(job: &Arc<Mutex<JobState>>, result: Value) {
    let mut state = job.lock().unwrap();
    state.running = false;
    state.finished = true;
    state.result = Some(result);
}

fn job_fail(job: &Arc<Mutex<JobState>>, error: &str, cancelled: bool) {
    let mut state = job.lock().unwrap();
    state.running = false;
    state.finished = true;
    state.cancelled = cancelled;
    state.error = error.to_owned();
}

// ─────────────────────── Среда запуска Linux (BedrockOnLinux) ──────────────

struct BolRuntime {
    /// Готовые к запуску команды (системная среда и/или встроенный pyz).
    commands: Vec<Vec<String>>,
    setup_error: String,
}

struct ProbeStatus {
    supported: bool,
    version: String,
    message: String,
}

/// Имя файла встроенной среды: версия и имя держатся в синхроне.
fn bundled_pyz_name() -> &'static str {
    static NAME: LazyLock<String> =
        LazyLock::new(|| format!("bedrock-on-linux-{BEDROCK_ON_LINUX_BUNDLED_VERSION}.pyz"));
    &NAME
}

fn bundled_pyz_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("NIVORA_BEDROCK_ON_LINUX_PYZ") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            candidates.push(dir.join("vendor").join(bundled_pyz_name()));
            candidates.push(dir.join("nivora_launcher").join("vendor").join(bundled_pyz_name()));
            candidates.push(dir.join("../share/nivora-launcher/vendor").join(bundled_pyz_name()));
            candidates.push(dir.join("../Resources/vendor").join(bundled_pyz_name()));
            // Режим разработки: binary лежит в target/debug, pyz — в
            // src-tauri/resources/vendor.
            candidates.push(dir.join("../../resources/vendor").join(bundled_pyz_name()));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("nivora_launcher").join("vendor").join(bundled_pyz_name()));
    }
    candidates.push(launcher_roots().data_home.join("vendor").join(bundled_pyz_name()));
    candidates
}

fn python_for_zipapp() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NIVORA_PYTHON") {
        return Some(PathBuf::from(path));
    }
    for name in ["python3", "python"] {
        if let Some(found) = find_in_path(name) {
            return Some(found);
        }
    }
    None
}

fn bol_runtime() -> BolRuntime {
    let mut commands: Vec<Vec<String>> = Vec::new();
    if let Some(executable) = find_in_path("bedrock-on-linux") {
        commands.push(vec![executable.to_string_lossy().into_owned()]);
    }
    let mut bundled_error = String::new();
    let mut bundled: Option<PathBuf> = None;
    for candidate in bundled_pyz_candidates() {
        if is_reparse_point(&candidate) {
            bundled_error = "Среда запуска Bedrock повреждена. Переустановите Nivora.".to_owned();
            break;
        }
        match fs::metadata(&candidate) {
            Ok(meta) if meta.is_file() => {
                if meta.len() > BUNDLED_PYZ_MAX_BYTES {
                    bundled_error =
                        "Размер среды запуска Bedrock не прошёл проверку. Переустановите приложение."
                            .to_owned();
                    break;
                }
                match fs::read(&candidate) {
                    Ok(bytes) => {
                        if sha256_hex(&bytes) != BEDROCK_ON_LINUX_BUNDLED_SHA256 {
                            bundled_error =
                                "Проверка среды запуска Bedrock не пройдена. Переустановите Nivora."
                                    .to_owned();
                            break;
                        }
                        bundled = Some(candidate);
                    }
                    Err(error) => {
                        bundled_error = format!("Не удалось проверить среду запуска Bedrock: {error}");
                        break;
                    }
                }
            }
            _ => continue,
        }
    }
    match &bundled {
        Some(path) => match python_for_zipapp() {
            Some(python) => {
                commands.push(vec![python.to_string_lossy().into_owned(), path.to_string_lossy().into_owned()])
            }
            None => {
                bundled_error = "Для среды запуска Bedrock нужен Python 3; переустановите Nivora с зависимостями."
                    .to_owned()
            }
        },
        None => {
            if bundled_error.is_empty() {
                bundled_error =
                    "Среда запуска Bedrock отсутствует в пакете Nivora. Переустановите приложение.".to_owned();
            }
        }
    }
    BolRuntime { commands, setup_error: bundled_error }
}

/// glibc для store-login BedrockOnLinux (xodus требует 2.39+).
fn glibc_version() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let ldd = find_in_path("ldd")?;
    let output = run_timed(prepare_command(Command::new(ldd).arg("--version")), Duration::from_secs(10))?;
    glibc_re()
        .captures(&output.1)
        .and_then(|captures| captures.get(1).map(|value| value.as_str().to_owned()))
}

fn bol_probe(command: &[String], home: &Path) -> ProbeStatus {
    let unsupported = |version: String, message: &str| ProbeStatus {
        supported: false,
        version,
        message: message.to_owned(),
    };
    let mut process = Command::new(&command[0]);
    process.args(&command[1..]).arg("--help");
    if home.is_dir() {
        process.current_dir(home);
    }
    let mut environment: HashMap<String, String> = std::env::vars().collect();
    environment.insert("BOL_HOME".to_owned(), home.to_string_lossy().into_owned());
    process.envs(&environment);
    let Some((code, output)) = run_timed(prepare_command(&mut process), Duration::from_secs(15)) else {
        return unsupported(
            String::new(),
            "Не удалось проверить среду запуска Bedrock: время ожидания истекло",
        );
    };
    let Some(captures) = version_re().captures(&output) else {
        return unsupported(
            String::new(),
            "Не удалось проверить совместимость среды запуска Bedrock. Переустановите приложение.",
        );
    };
    let version = captures.get(1).map(|value| value.as_str().to_owned()).unwrap_or_default();
    if code != 0 {
        return unsupported(
            version,
            "Не удалось проверить совместимость среды запуска Bedrock. Переустановите приложение.",
        );
    }
    let supported = version_tuple(&version) >= MIN_BOL_VERSION;
    ProbeStatus {
        supported,
        version,
        message: if supported {
            "Среда запуска Bedrock готова к работе.".to_owned()
        } else {
            "Среда запуска Bedrock устарела. Обновите Nivora до актуальной версии.".to_owned()
        },
    }
}

fn bol_select_supported(commands: &[Vec<String>], home: &Path) -> (Vec<String>, ProbeStatus) {
    let mut last = ProbeStatus {
        supported: false,
        version: String::new(),
        message: "Не удалось проверить совместимость среды запуска Bedrock. Переустановите приложение."
            .to_owned(),
    };
    for command in commands {
        last = bol_probe(command, home);
        if last.supported {
            return (command.clone(), last);
        }
    }
    (commands.last().cloned().unwrap_or_default(), last)
}

fn bol_require_runtime(home: &Path) -> Result<(Vec<String>, ProbeStatus), String> {
    let runtime = bol_runtime();
    if runtime.commands.is_empty() {
        return Err(runtime.setup_error);
    }
    let selected = bol_select_supported(&runtime.commands, home);
    if !selected.1.supported {
        return Err(selected.1.message);
    }
    Ok(selected)
}

// ── реестр активных процессов среды (остановка запусков) ──

static ACTIVE_PROCESSES: LazyLock<Mutex<HashMap<String, Arc<Mutex<Child>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn register_process(key: &str, child: Child) -> Arc<Mutex<Child>> {
    let shared = Arc::new(Mutex::new(child));
    ACTIVE_PROCESSES.lock().unwrap().insert(key.to_owned(), Arc::clone(&shared));
    shared
}

fn unregister_process(key: &str, shared: &Arc<Mutex<Child>>) {
    let mut map = ACTIVE_PROCESSES.lock().unwrap();
    if map.get(key).is_some_and(|existing| Arc::ptr_eq(existing, shared)) {
        map.remove(key);
    }
}

fn stop_registered_process(key: &str) -> bool {
    let shared = {
        let map = ACTIVE_PROCESSES.lock().unwrap();
        map.get(key).cloned()
    };
    let Some(shared) = shared else { return false };
    let mut child = shared.lock().unwrap();
    if child.try_wait().ok().flatten().is_some() {
        return false;
    }
    terminate_process_tree(&mut child, false);
    true
}

fn terminate_process_tree(child: &mut Child, force: bool) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        terminate_linux_tree(child, force);
    }
    #[cfg(not(target_os = "linux"))]
    {
        terminate_generic(child, force);
    }
}

/// Гашение дерева процессов через /proc: store-login запускает xodus-cli
/// в новой сессии — вне группы процесса, поэтому одного kill() мало.
#[cfg(target_os = "linux")]
fn terminate_linux_tree(child: &mut Child, force: bool) {
    let pid = child.id() as i32;
    if pid <= 0 {
        terminate_generic(child, force);
        return;
    }
    let tree = linux_process_tree(pid);
    if tree.is_empty() {
        terminate_generic(child, force);
        return;
    }
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    for item in tree.iter().rev() {
        unsafe {
            libc::kill(*item, signal);
        }
    }
    if force {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if !tree.iter().any(|pid| linux_process_alive(*pid)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for item in tree.iter().rev() {
        unsafe {
            libc::kill(*item, libc::SIGKILL);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_process_tree(root_pid: i32) -> Vec<i32> {
    let mut pending = vec![root_pid];
    let mut seen: Vec<i32> = Vec::new();
    let mut ordered = Vec::new();
    while let Some(pid) = pending.pop() {
        if seen.contains(&pid) {
            continue;
        }
        seen.push(pid);
        ordered.push(pid);
        let children_path = format!("/proc/{pid}/task/{pid}/children");
        if let Ok(text) = fs::read_to_string(children_path) {
            for token in text.split_whitespace() {
                if let Ok(child) = token.parse::<i32>() {
                    pending.push(child);
                }
            }
        }
    }
    ordered
}

#[cfg(target_os = "linux")]
fn linux_process_alive(pid: i32) -> bool {
    let Ok(value) = fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    let tail = &value[value.rfind(')').map(|index| index + 1).unwrap_or(0)..];
    !tail.trim_start().starts_with('Z')
}

fn terminate_generic(child: &mut Child, _force: bool) {
    #[cfg(windows)]
    {
        if let Some(taskkill) = find_in_path("taskkill.exe").or_else(|| find_in_path("taskkill")) {
            let _ = Command::new(taskkill)
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Окружение BoL: BOL_HOME + подмена notify-send (Nivora сам решает, когда
/// отправлять системные уведомления) + опциональный XDG_DATA_HOME для ярлыков.
fn bol_environment(home: &Path, xdg_data_home: Option<&Path>) -> HashMap<String, String> {
    let mut environment: HashMap<String, String> = std::env::vars().collect();
    environment.insert("BOL_HOME".to_owned(), home.to_string_lossy().into_owned());
    if let Some(xdg) = xdg_data_home {
        environment.insert("XDG_DATA_HOME".to_owned(), xdg.to_string_lossy().into_owned());
    }
    if let Some(stub_dir) = notification_stub_dir(home) {
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = environment.get("PATH").cloned().unwrap_or_default();
        environment.insert("PATH".to_owned(), format!("{stub_dir}{separator}{path}"));
    }
    environment
}

fn notification_stub_dir(home: &Path) -> Option<String> {
    let stub_dir = home.join(".nivora-bin");
    let result = (|| -> std::io::Result<()> {
        fs::create_dir_all(&stub_dir)?;
        let stub = stub_dir.join("notify-send");
        if !stub.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::write(&stub, "#!/bin/sh\nexit 0\n")?;
                fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))?;
            }
            #[cfg(not(unix))]
            fs::write(&stub, "")?;
        }
        Ok(())
    })();
    match result {
        Ok(()) if stub_dir.join("notify-send").is_file() => Some(stub_dir.to_string_lossy().into_owned()),
        _ => None,
    }
}

fn bol_run(
    argv: &[String],
    home: &Path,
    process_key: &str,
    cancellation: Option<&CancellationToken>,
    mut on_line: Option<&mut dyn FnMut(&str)>,
    stall_timeout: Duration,
    max_lines: usize,
    quiet: bool,
    xdg_data_home: Option<&Path>,
    on_start: Option<&dyn Fn(u32)>,
    mut watchdog: Option<&mut DownloadWatchdog>,
) -> Result<(i32, Vec<String>), String> {
    let (command, status) = {
        let runtime = bol_runtime();
        if runtime.commands.is_empty() {
            return Err(runtime.setup_error);
        }
        bol_select_supported(&runtime.commands, home)
    };
    if !status.supported {
        return Err(status.message);
    }
    if let Some(token) = cancellation {
        token.check()?;
    }
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .args(argv)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(bol_environment(home, xdg_data_home));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    prepare_command(&mut process);
    let mut child = process
        .spawn()
        .map_err(|error| format!("Не удалось запустить Minecraft Bedrock: {error}"))?;
    if let Some(started) = on_start {
        started(child.id());
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (sender, receiver) = mpsc::channel::<Option<String>>();
    spawn_line_reader(stdout, sender.clone());
    spawn_line_reader(stderr, sender);
    let shared = register_process(process_key, child);
    let mut lines: Vec<String> = Vec::new();
    let mut stalled = false;
    let mut last_output = Instant::now();
    let mut exit_code: Option<i32> = None;
    let mut reader_done = false;
    while !reader_done || exit_code.is_none() {
        if let Some(token) = cancellation {
            if token.cancelled() {
                if let Ok(mut child) = shared.try_lock() {
                    terminate_process_tree(&mut child, false);
                }
                unregister_process(process_key, &shared);
                return Err("Операция остановлена пользователем".to_owned());
            }
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(line)) => {
                last_output = Instant::now();
                if !quiet {
                    let safe = redact_line(&line);
                    lines.push(safe.clone());
                    if lines.len() > max_lines {
                        let drop = lines.len() - max_lines;
                        lines.drain(..drop);
                    }
                    if let Some(callback) = on_line.as_deref_mut() {
                        callback(&safe);
                    }
                }
            }
            Ok(None) => reader_done = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !stall_timeout.is_zero() && last_output.elapsed() > stall_timeout {
                    // Молчание stdout ≠ зависание: CLI-путь do_setup в
                    // BedrockOnLinux качает пакет игры (2+ ГиБ), не печатая
                    // ни строки. Пока дерево профиля растёт — это живое
                    // скачивание: продлеваем ожидание и подаём heartbeat.
                    let verdict = watchdog.as_deref_mut().map(|state| state.check());
                    match verdict {
                        Some(WatchdogVerdict::Active(bytes)) => {
                            last_output = Instant::now();
                            if let Some(callback) = on_line.as_deref_mut() {
                                callback(&format!(
                                    "Скачивание продолжается: {} данных получено",
                                    human_bytes(bytes)
                                ));
                            }
                        }
                        Some(WatchdogVerdict::Pending) => {}
                        _ => {
                            stalled = true;
                            break;
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => reader_done = true,
        }
        if exit_code.is_none() {
            if let Ok(mut child) = shared.try_lock() {
                if let Some(code) = child.try_wait().ok().flatten() {
                    exit_code = code.code();
                }
            }
        }
    }
    if stalled {
        if let Ok(mut child) = shared.try_lock() {
            terminate_process_tree(&mut child, false);
        }
        unregister_process(process_key, &shared);
        return Err(format!(
            "Среда запуска Bedrock перестала отвечать ({} сек без вывода) и была остановлена. Проверьте подключение к сети и повторите попытку.",
            stall_timeout.as_secs()
        ));
    }
    let final_code = match exit_code {
        Some(code) => code,
        None => match shared.lock() {
            Ok(mut child) => child.wait().map(|status| status.code().unwrap_or(-1)).unwrap_or(-1),
            Err(_) => -1,
        },
    };
    unregister_process(process_key, &shared);
    Ok((final_code, lines))
}

/// Читатель stdout/stderr процесса: строки в канал, None в конце потока.
/// Читатель stdout/stderr процесса: строки в канал, None в конце потока.
///
/// Границей строки считается не только `\n`, но и `\r`: инструменты скачивания
/// (winget, индикаторы BedrockOnLinux) перерисовывают прогресс возвратом
/// каретки, и «настоящий» читатель строк молчал бы всю загрузку — а таймаут
/// простоя считал живое скачивание зависшим и убивал его на середине.
fn spawn_line_reader<S: Read + Send + 'static>(stream: Option<S>, sender: mpsc::Sender<Option<String>>) {
    if let Some(stream) = stream {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut pending: Vec<u8> = Vec::with_capacity(256);
            loop {
                let (length, lines): (usize, Vec<String>) = {
                    let available = match reader.fill_buf() {
                        Ok(slice) => slice,
                        Err(_) => break,
                    };
                    if available.is_empty() {
                        break;
                    }
                    let mut lines: Vec<String> = Vec::new();
                    let length = available.len();
                    for &byte in available.iter() {
                        if byte == b'\n' || byte == b'\r' {
                            if !pending.is_empty() {
                                if let Some(line) = clean_process_line(&pending) {
                                    lines.push(line);
                                }
                                pending.clear();
                            }
                        } else if pending.len() < 8192 {
                            pending.push(byte);
                        }
                    }
                    (length, lines)
                };
                reader.consume(length);
                let mut closed = false;
                for line in lines {
                    if sender.send(Some(line)).is_err() {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    return;
                }
            }
            if !pending.is_empty() {
                if let Some(line) = clean_process_line(&pending) {
                    let _ = sender.send(Some(line));
                }
            }
            let _ = sender.send(None);
        });
    }
}

/// Чистка сырой строки процесса: ANSI-последовательности (прогресс-бары
/// winget и BedrockOnLinux перерисовывают вывод курсорами), NUL-добивки
/// псевдотерминала и хвостовые пробелы. Пустое после чистки — None: это шум
/// перерисовки, а не строка.
fn clean_process_line(raw: &[u8]) -> Option<String> {
    let mut cleaned: Vec<u8> = Vec::with_capacity(raw.len().min(1024));
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        // ESC [ … конечный байт 0x40..=0x7E — CSI-последовательность (цвет, курсор).
        // ESC ] … BEL — OSC-последовательность (заголовок окна).
        if byte == 0x1b && index + 1 < raw.len() {
            match raw[index + 1] {
                b'[' => {
                    let mut cursor = index + 2;
                    while cursor < raw.len() && !(0x40..=0x7e).contains(&raw[cursor]) {
                        cursor += 1;
                    }
                    index = (cursor + 1).min(raw.len());
                    continue;
                }
                b']' => {
                    let mut cursor = index + 2;
                    while cursor < raw.len() && raw[cursor] != 0x07 {
                        cursor += 1;
                    }
                    index = (cursor + 1).min(raw.len());
                    continue;
                }
                _ => {
                    index += 2;
                    continue;
                }
            }
        }
        if byte != 0 && byte != 0x08 {
            cleaned.push(byte);
        }
        index += 1;
    }
    let text = String::from_utf8_lossy(&cleaned).trim_end().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Размер в человекочитаемом виде для heartbeat-строк скачивания.
fn human_bytes(bytes: u64) -> String {
    const GIB: f64 = 1_073_741_824.0;
    const MIB: f64 = 1_048_576.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{value:.2} ГиБ")
    } else if value >= MIB {
        format!("{value:.1} МиБ")
    } else {
        format!("{} КиБ", bytes / 1024)
    }
}

/// Суммарный размер файлов в дереве (по статистике, без чтения). Пределы
/// защищают от симлинк-циклов и патологических деревьев: здоровый профиль
/// Bedrock укладывается в десятки тысяч записей.
fn walk_tree_size(root: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut entries: usize = 0;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            entries += 1;
            if entries > 200_000 {
                return total;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if stack.len() < 64 {
                    stack.push(entry.path());
                }
            } else {
                total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Вердикт watchdog'а для фазы, когда процесс молчит по stdout.
#[derive(Debug, PartialEq)]
enum WatchdogVerdict {
    /// Снимка ещё нет / прошёл слишком мало времени — решения нет.
    Pending,
    /// Дерево выросло — скачивание или распаковка живы.
    Active(u64),
    /// Дерево не меняется — процесс действительно завис.
    Silent,
}

/// Немой инструмент скачивания — не приговор: BedrockOnLinux при установке
/// игры пишет пакет на диск, не печатая ничего в stdout (CLI-путь do_setup не
/// передаёт progress-колбэк). Рост дерева профиля — единственный честный
/// признак работы; старый код убивал живое скачивание по таймауту тишины,
/// и каждая повторная попытка начинала 2+ ГиБ с нуля.
struct DownloadWatchdog {
    root: PathBuf,
    snapshot: u64,
    have_snapshot: bool,
    last_walk: Option<Instant>,
    interval: Duration,
}

impl DownloadWatchdog {
    fn new(root: &Path) -> Self {
        Self::with_interval(root, Duration::from_secs(20))
    }

    fn with_interval(root: &Path, interval: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            snapshot: 0,
            have_snapshot: false,
            last_walk: None,
            interval,
        }
    }

    /// Вызывается только когда stdout молчит дольше stall-таймаута:
    /// частые обходы не нужны, решает разница между двумя снимками.
    fn check(&mut self) -> WatchdogVerdict {
        if let Some(last) = self.last_walk {
            if last.elapsed() < self.interval {
                return WatchdogVerdict::Pending;
            }
        }
        let total = walk_tree_size(&self.root);
        if !self.have_snapshot {
            self.snapshot = total;
            self.have_snapshot = true;
            self.last_walk = Some(Instant::now());
            return WatchdogVerdict::Pending;
        }
        let grew = total > self.snapshot;
        self.snapshot = total;
        self.last_walk = Some(Instant::now());
        if grew {
            WatchdogVerdict::Active(total)
        } else {
            WatchdogVerdict::Silent
        }
    }
}

// ── профили BoL (Linux) ──

fn profile_name(instance_name: &str) -> String {
    format!("nivora-{}", &sha256_hex(instance_name.as_bytes())[..16])
}

struct ProfilePaths {
    root: PathBuf,
    profiles_root: PathBuf,
}

fn profile_paths(ctx: &Ctx) -> Result<ProfilePaths, String> {
    let root = ctx.game_dir.join("bedrock");
    let profiles = root.join("profiles");
    if is_reparse_point(&root) || is_reparse_point(&profiles) {
        return Err("Каталог Bedrock является символьной ссылкой".to_owned());
    }
    fs::create_dir_all(&profiles).map_err(|_| "Каталог Bedrock недоступен".to_owned())?;
    if is_symlink_or_missing_dir(&root) || is_symlink_or_missing_dir(&profiles) {
        return Err("Каталог Bedrock недоступен".to_owned());
    }
    super::platform::restrict_dir(&root);
    super::platform::restrict_dir(&profiles);
    Ok(ProfilePaths { root, profiles_root: profiles })
}

fn validate_profile_path(paths: &ProfilePaths, profile_name: &str) -> Result<PathBuf, String> {
    if !profile_name_re().is_match(profile_name) {
        return Err("Недопустимый идентификатор профиля Bedrock".to_owned());
    }
    let profile = paths.profiles_root.join(profile_name);
    if is_reparse_point(&profile) || !profile.is_dir() {
        return Err("Профиль Bedrock не найден".to_owned());
    }
    match (profile.canonicalize(), paths.profiles_root.canonicalize()) {
        (Ok(resolved), Ok(root)) if resolved.starts_with(&root) => {}
        _ => return Err("Путь профиля Bedrock выходит за пределы хранилища".to_owned()),
    }
    let metadata = profile.join("profile.json");
    if is_reparse_point(&metadata) || !metadata.is_file() {
        return Err("Метаданные профиля Bedrock не найдены".to_owned());
    }
    let metadata =
        read_bounded_json(&metadata, 64 * 1024).ok_or_else(|| "Не удалось прочитать профиль Bedrock".to_owned())?;
    if metadata.get("slug").and_then(Value::as_str) != Some(profile_name) {
        return Err("Идентификатор профиля Bedrock не совпадает".to_owned());
    }
    Ok(profile)
}

fn profile_settings(profile: &Path) -> Value {
    read_bounded_json(&profile.join("settings.json"), 1024 * 1024).unwrap_or(Value::Null)
}

/// Выбранная версия игры из настроек профиля.
fn selected_version(profile: &Path) -> (String, String) {
    let settings = profile_settings(profile);
    if !settings.is_object() {
        return (String::new(), String::new());
    }
    let edition = settings
        .get("mc_edition")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let version = settings.get("mc_version").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let valid_edition = edition == "release" || edition == "preview";
    if !valid_edition || !game_version_re().is_match(&version) {
        return (String::new(), String::new());
    }
    (edition, version)
}

fn engine_ready(profile: &Path) -> bool {
    // BoL пишет выбранный движок (GDK-Proton/WineGDK) в settings.json;
    // «play» без него умирал с «GDK-Proton missing».
    let proton = profile_settings(profile)
        .get("proton")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if proton.is_empty() {
        return false;
    }
    Path::new(&proton).is_dir()
}

fn store_status(profile: &Path) -> (bool, String) {
    let mut signed_in = false;
    let mut gamertag = String::new();
    if let Some(token) = read_bounded_json(&profile.join("msa").join("token.json"), 1024 * 1024) {
        if token.get("refresh_token").is_some() {
            signed_in = true;
        }
    }
    if let Some(device) = read_bounded_json(&profile.join("winegdk-preauth").join("device.json"), 1024 * 1024) {
        if let Some(tag) = device.get("xbl_gamertag").and_then(Value::as_str) {
            gamertag = tag.to_owned();
        }
    }
    (signed_in, gamertag)
}

// ───────────────────── Официальный Bedrock Windows ─────────────────────────

fn win_local_appdata() -> Option<PathBuf> {
    let value = std::env::var_os("LOCALAPPDATA")?;
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

fn win_package_dir(channel: &WindowsChannel) -> Option<PathBuf> {
    win_local_appdata().map(|base| base.join("Packages").join(channel.package_family))
}

/// Каталог контента официальной игры: …\Packages\<family>\LocalState\games\com.mojang
fn win_content_root(edition: &str) -> Result<PathBuf, String> {
    let channel = win_channel(edition);
    let package = win_package_dir(channel)
        .ok_or_else(|| "Не удалось определить каталог данных Minecraft (LOCALAPPDATA)".to_owned())?;
    Ok(package.join("LocalState").join("games").join("com.mojang"))
}

#[derive(Clone)]
struct WinPackageState {
    installed: bool,
    version: String,
}

/// Кэш состояния пакетов Store: статус среды, список версий и установка
/// спрашивают одно и то же, а PowerShell на машинах с антивирусом стартует
/// секунды — раньше каждый вызов запускал свой процесс на каждый выпуск.
struct WinPackageCache {
    at: Instant,
    release: WinPackageState,
    preview: WinPackageState,
}

static WIN_PACKAGE_CACHE: LazyLock<Mutex<Option<WinPackageCache>>> =
    LazyLock::new(|| Mutex::new(None));

/// Сброс кэша состояния пакетов: winget или страница Store могли только что
/// установить игру — свежая проверка обязана увидеть новый каталог.
fn invalidate_windows_package_cache() {
    *WIN_PACKAGE_CACHE.lock().unwrap() = None;
}

fn windows_package_state(edition: &str) -> WinPackageState {
    let (release, preview) = windows_package_states();
    if edition.eq_ignore_ascii_case("preview") {
        preview
    } else {
        release
    }
}

/// Оба выпуска одним процессом PowerShell и общим кэшем на 30 секунд:
/// заход на вкладку «Версия» раньше мог стоить 5–7 запусков PowerShell
/// (до 20 секунд каждый на заблокированной системе) — интерфейс выглядел
/// «зависшим», а список версий не загружался.
fn windows_package_states() -> (WinPackageState, WinPackageState) {
    if let Some(cache) = WIN_PACKAGE_CACHE.lock().unwrap().as_ref() {
        if cache.at.elapsed() < Duration::from_secs(30) {
            return (cache.release.clone(), cache.preview.clone());
        }
    }
    let pair = query_windows_package_states();
    *WIN_PACKAGE_CACHE.lock().unwrap() = Some(WinPackageCache {
        at: Instant::now(),
        release: pair.0.clone(),
        preview: pair.1.clone(),
    });
    pair
}

/// Наличие пакета — по каталогу данных (дёшево, без PowerShell); версия —
/// одним процессом PowerShell на оба выпуска, и только когда хоть один
/// каталог есть. PowerShell недоступен/заблокирован → версия пустая,
/// интерфейс продолжает работать (Store-поток ставит «latest»).
fn query_windows_package_states() -> (WinPackageState, WinPackageState) {
    let release = dir_based_state("release");
    let preview = dir_based_state("preview");
    let mut pair = (release, preview);
    if !pair.0.installed && !pair.1.installed {
        return pair;
    }
    let script = format!(
        "(Get-AppxPackage -Name {}).Version\n\"@@NIVORA-CHANNEL@@\"\n(Get-AppxPackage -Name {}).Version",
        WIN_CHANNEL_RELEASE.package_name, WIN_CHANNEL_PREVIEW.package_name
    );
    let powershell = find_in_path("powershell.exe")
        .or_else(|| find_in_path("powershell"))
        .unwrap_or_else(|| PathBuf::from("powershell"));
    let mut command = Command::new(powershell);
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    if let Some((_, output)) = run_timed(prepare_command(&mut command), Duration::from_secs(8)) {
        let sections: Vec<&str> = output.split("@@NIVORA-CHANNEL@@").collect();
        if sections.len() == 2 {
            pair.0.version = first_version_line(sections[0]);
            pair.1.version = first_version_line(sections[1]);
        }
    }
    pair
}

fn first_version_line(section: &str) -> String {
    section
        .lines()
        .map(str::trim)
        .find(|line| line.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("")
        .to_owned()
}

fn dir_based_state(edition: &str) -> WinPackageState {
    let channel = win_channel(edition);
    let Some(package_dir) = win_package_dir(channel) else {
        return WinPackageState { installed: false, version: String::new() };
    };
    if is_reparse_point(&package_dir) || !package_dir.is_dir() {
        return WinPackageState { installed: false, version: String::new() };
    }
    WinPackageState { installed: true, version: String::new() }
}

fn winget_executable() -> Option<PathBuf> {
    if let Some(found) = find_in_path("winget.exe").or_else(|| find_in_path("winget")) {
        return Some(found);
    }
    win_local_appdata()
        .map(|base| base.join("Microsoft").join("WindowsApps").join("winget.exe"))
        .filter(|path| path.is_file())
}

/// Открыть официальную страницу продукта в Microsoft Store.
fn open_store_product(edition: &str) -> bool {
    let channel = win_channel(edition);
    let uri = format!("ms-windowsstore://pdp/?ProductId={}", channel.product_id);
    let mut command = Command::new("cmd.exe");
    command.args(["/C", "start", "", &uri]);
    prepare_command(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Запуск официальной игры по протоколу (minecraft:// / minecraft-preview://).
fn launch_official(edition: &str) -> Result<(), String> {
    let channel = win_channel(edition);
    let mut command = Command::new("cmd.exe");
    command.args(["/C", "start", "", channel.protocol]);
    let started = prepare_command(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if started.map(|status| status.success()).unwrap_or(false) {
        return Ok(());
    }
    // Запасной путь — AUMID через AppsFolder.
    let mut fallback = Command::new("explorer.exe");
    fallback.arg(format!("shell:AppsFolder\\{}", channel.aumid));
    let fallback_ok = prepare_command(&mut fallback)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if fallback_ok {
        return Ok(());
    }
    Err(format!(
        "Не удалось запустить {}. Проверьте, что игра установлена из Microsoft Store.",
        channel.title
    ))
}

/// PID игры по образу процесса (tasklist есть на любой Windows).
fn windows_game_pids() -> Vec<u32> {
    let mut command = Command::new("tasklist.exe");
    command.args(["/FO", "CSV", "/NH"]);
    let Some((_, text)) = run_timed(prepare_command(&mut command), Duration::from_secs(15)) else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split("\",\"").collect();
        if parts.len() < 2 {
            continue;
        }
        let image = parts[0].trim_matches('"');
        if !image.to_ascii_lowercase().starts_with("minecraft") {
            continue;
        }
        if let Ok(pid) = parts[1].trim_matches('"').parse::<u32>() {
            pids.push(pid);
        }
    }
    pids
}

fn windows_kill_pid(pid: u32) -> bool {
    let Some(taskkill) = find_in_path("taskkill.exe").or_else(|| find_in_path("taskkill")) else {
        return false;
    };
    Command::new(taskkill)
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// ─────────────────────── Метаданные экземпляра Bedrock ─────────────────────

fn read_instance_metadata(path: &Path) -> Value {
    // Python-воркер исторически пишет .trial-launcher-instance.json
    // (обновление профиля), Rust-движок — .nivora-launcher-instance.json.
    // Читаем оба, предпочитаем «свой»: в нём актуальный выбор канала
    // официальной версии (bedrock_edition/bedrock_version).
    for name in [
        INSTANCE_METADATA_FILE,
        ".trial-launcher-instance.json",
    ] {
        if let Some(value) = read_bounded_json(&path.join(name), 1024 * 1024) {
            if value.is_object() {
                return value;
            }
        }
    }
    json!({})
}

fn write_instance_metadata(path: &Path, metadata: &Value) -> Result<(), String> {
    write_private_json(&path.join(INSTANCE_METADATA_FILE), metadata)
}

/// Выбранный канал официальной версии хранится в метаданных экземпляра.
fn windows_selected(instance_path: &Path) -> (String, String) {
    let metadata = read_instance_metadata(instance_path);
    let edition = metadata
        .get("bedrock_edition")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let version = metadata.get("bedrock_version").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let valid_edition = edition == "release" || edition == "preview";
    if !valid_edition || !(game_version_re().is_match(&version) || version == "latest") {
        return (String::new(), String::new());
    }
    (edition, version)
}

fn persist_windows_selection(instance_path: &Path, edition: &str, version: &str) -> Result<(), String> {
    let mut metadata = read_instance_metadata(instance_path);
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| "Метаданные экземпляра повреждены".to_owned())?;
    object.insert("bedrock_edition".into(), json!(edition));
    object.insert("bedrock_version".into(), json!(version));
    write_instance_metadata(instance_path, &metadata)
}

// ──────────────────────── Живое состояние и логи ───────────────────────────

/// Python-воркер читает .trial-launcher-live.json, Rust-движок —
/// .nivora-launcher-live.json; пишем оба, чтобы «запущено» видели все.
fn write_live_state(
    instance_path: &Path,
    running: bool,
    pid: u32,
    exit_code: Option<i64>,
    started_at: f64,
    log_id: &str,
) {
    let value = json!({
        "running": running,
        "pid": pid,
        "exit_code": exit_code,
        "started_at": started_at,
        "log_id": log_id,
    });
    for name in [".trial-launcher-live.json", ".nivora-launcher-live.json"] {
        let path = instance_path.join(name);
        if is_reparse_point(&path) {
            continue;
        }
        if let Ok(mut file) = super::platform::create_private(&path) {
            let _ = serde_json::to_writer(&mut file, &value);
            super::platform::restrict_file(&path);
        }
    }
}

/// Идентификатор журнала, совместимый с парсером Python (instance_files.py):
/// <имя>-<sha24>-bedrock-<мс>-<hex10>.log
fn new_log_id(instance_name: &str) -> String {
    let component: String = instance_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect::<String>()
        .trim_matches(|c| matches!(c, '.' | '_' | '-'))
        .chars()
        .take(80)
        .collect();
    let component = if component.is_empty() { "launcher".to_owned() } else { component };
    format!(
        "{}-{}-bedrock-{}-{}.log",
        component,
        &sha256_hex(instance_name.as_bytes())[..24],
        now_millis(),
        random_hex(5)
    )
}

struct SessionLog {
    path: PathBuf,
}

impl SessionLog {
    fn create(data_home: &Path, log_id: &str) -> SessionLog {
        let dir = data_home.join("logs");
        let _ = fs::create_dir_all(&dir);
        super::platform::restrict_dir(&dir);
        SessionLog { path: dir.join(log_id) }
    }

    fn append(&self, line: &str) {
        if let Ok(meta) = fs::metadata(&self.path) {
            if meta.len() > MAX_LOG_ARCHIVE_BYTES {
                return;
            }
        }
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

// ───────────────────────────── Парсинг версий ──────────────────────────────

fn parse_game_versions(lines: &[String]) -> Vec<Value> {
    let mut edition = String::new();
    let mut versions: Vec<Value> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for line in lines {
        if let Some(captures) = version_heading_re().captures(line) {
            edition = captures.get(1).map(|value| value.as_str().to_owned()).unwrap_or_default();
            continue;
        }
        if edition.is_empty() {
            continue;
        }
        let Some(captures) = version_line_re().captures(line) else { continue };
        let version = captures.get(1).map(|value| value.as_str().to_owned()).unwrap_or_default();
        let key = (edition.clone(), version.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        versions.push(json!({
            "edition": edition,
            "version": version,
            "installed": captures.get(2).is_some(),
        }));
    }
    versions
}

// ───────────────────────── Валидация архивов ───────────────────────────────

fn validate_content_archive(content: &[u8]) -> Result<zip::ZipArchive<std::io::Cursor<&[u8]>>, String> {
    if content.is_empty() || content.len() > MAX_CONTENT_UPLOAD_BYTES {
        return Err("Файл Bedrock пуст или превышает лимит 128 MiB".to_owned());
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(content))
        .map_err(|_| "Файл не является корректным ZIP-контентом Minecraft Bedrock".to_owned())?;
    if archive.len() == 0 || archive.len() as usize > MAX_CONTENT_ARCHIVE_ENTRIES {
        return Err("Архив Bedrock пуст или содержит слишком много файлов".to_owned());
    }
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| "Файл не является корректным ZIP-контентом Minecraft Bedrock".to_owned())?;
        total += entry.size();
        if total > MAX_CONTENT_UNPACKED_BYTES {
            return Err("Распакованный файл Bedrock превышает лимит 1 GiB".to_owned());
        }
        let name = entry.name().replace('\\', "/");
        let is_symlink = entry
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false);
        if name.is_empty()
            || name.starts_with('/')
            || (name.len() > 1 && name.as_bytes()[1] == b':')
            || name.split('/').any(|part| part == ".." || part.is_empty())
            || name.contains('\0')
            || is_symlink
        {
            return Err("Архив Bedrock содержит небезопасный путь или ссылку".to_owned());
        }
    }
    Ok(archive)
}

// ─────────────────────────── Операции движка ───────────────────────────────

pub fn runtime_status(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name").unwrap_or_default().to_owned();
    let ctx = context()?;
    let mut status = match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_runtime_status(&ctx, &name)?,
        BedrockPlatform::WindowsOfficial => windows_runtime_status(&ctx, &name)?,
        BedrockPlatform::Unsupported => json!({
            "available": false,
            "supported": false,
            "version": "",
            "message": "Официальный Minecraft Bedrock не выпускается для этой ОС: доступен в Windows (Microsoft Store) и в Linux-сборке Nivora.",
        }),
    };
    if let Some(object) = status.as_object_mut() {
        object.insert("platform".into(), json!(platform_id()));
        object.insert("official".into(), json!(bedrock_platform() == BedrockPlatform::WindowsOfficial));
    }
    Ok(json!({ "status": status }))
}

fn dependency_report() -> Vec<Value> {
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let mut dependencies = Vec::new();
            let python_ok = python_for_zipapp().is_some() || find_in_path("bedrock-on-linux").is_some();
            dependencies.push(json!({
                "name": "Python 3",
                "ok": python_ok,
                "detail": if python_ok {
                    "для встроенной среды BedrockOnLinux"
                } else {
                    "не найден; установите python3 или пакет bedrock-on-linux"
                },
            }));
            if let Some(version) = glibc_version() {
                let ok = version_tuple(&version) >= MIN_GLIBC_VERSION;
                dependencies.push(json!({
                    "name": "glibc",
                    "ok": ok,
                    "detail": if ok {
                        format!("{version} — вход в Microsoft Store поддерживается")
                    } else {
                        format!("{version} — для входа в Microsoft Store нужен glibc 2.39+; обновите систему")
                    },
                }));
            }
            dependencies
        }
        BedrockPlatform::WindowsOfficial => {
            let winget = winget_executable().is_some();
            vec![
                json!({
                    "name": "Microsoft Store",
                    "ok": true,
                    "detail": "официальная установка и лицензирование игры",
                }),
                json!({
                    "name": "winget (App Installer)",
                    "ok": winget,
                    "detail": if winget {
                        "автоматическая установка из Store"
                    } else {
                        "не найден — установка откроет страницу Microsoft Store"
                    },
                }),
            ]
        }
        BedrockPlatform::Unsupported => Vec::new(),
    }
}

fn linux_runtime_status(ctx: &Ctx, name: &str) -> Result<Value, String> {
    let home = if name.is_empty() {
        profile_paths(ctx)?.root
    } else {
        let store = instance_store()?;
        let info = bedrock_instance(&store, &name)?;
        linux_profile_path(ctx, &info)?
    };
    let runtime = bol_runtime();
    if runtime.commands.is_empty() {
        return Ok(json!({
            "available": false,
            "supported": false,
            "version": "",
            "message": format!("{} Среда: {BEDROCK_ON_LINUX_URL}.", runtime.setup_error),
            "dependencies": dependency_report(),
        }));
    }
    let (_, probe) = bol_select_supported(&runtime.commands, &home);
    let mut status = json!({
        "available": true,
        "supported": probe.supported,
        "version": probe.version,
        "message": probe.message,
        "dependencies": dependency_report(),
    });
    if !name.is_empty() {
        let store = instance_store()?;
        let info = bedrock_instance(&store, name)?;
        let profile = linux_profile_path(ctx, &info)?;
        let (signed_in, gamertag) = store_status(&profile);
        let (edition, version) = selected_version(&profile);
        if let Some(object) = status.as_object_mut() {
            object.insert("store_signed_in".into(), json!(signed_in));
            object.insert("gamertag".into(), json!(gamertag));
            object.insert("engine_ready".into(), json!(engine_ready(&profile)));
            object.insert("current_edition".into(), json!(edition));
            object.insert("current_version".into(), json!(version));
        }
    }
    Ok(status)
}

fn windows_runtime_status(ctx: &Ctx, name: &str) -> Result<Value, String> {
    let _ = ctx;
    let release = windows_package_state("release");
    let preview = windows_package_state("preview");
    let installed = release.installed || preview.installed;
    let version = if release.installed { release.version.clone() } else { preview.version.clone() };
    let (current_edition, current_version) = if name.is_empty() {
        (String::new(), String::new())
    } else {
        let store = instance_store()?;
        let info = bedrock_instance(&store, name)?;
        windows_selected(&info.path)
    };
    let message = if installed {
        format!(
            "Официальный {} установлен из Microsoft Store{}.",
            if release.installed { "Minecraft for Windows" } else { "Minecraft Preview for Windows" },
            if version.is_empty() { String::new() } else { format!(" (версия {version})") }
        )
    } else {
        "Официальный Minecraft Bedrock ещё не установлен: выберите версию на вкладке «Версия» — Nivora откроет официальную установку из Microsoft Store."
            .to_owned()
    };
    Ok(json!({
        "available": true,
        "supported": true,
        "version": version,
        "message": message,
        "dependencies": dependency_report(),
        "engine_ready": installed,
        "store_signed_in": installed,
        "gamertag": "",
        "current_edition": current_edition,
        "current_version": current_version,
    }))
}

pub fn versions(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name").unwrap_or_default().to_owned();
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_versions(&name, request.get("refresh").and_then(Value::as_bool).unwrap_or(false)),
        BedrockPlatform::WindowsOfficial => windows_versions(&name),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn linux_versions(name: &str, refresh: bool) -> Result<Value, String> {
    let ctx = context()?;
    let (profile, selected) = if name.is_empty() {
        (profile_paths(&ctx)?.root, (String::new(), String::new()))
    } else {
        let store = instance_store()?;
        let info = bedrock_instance(&store, &name)?;
        let profile = linux_profile_path(&ctx, &info)?;
        let selected = selected_version(&profile);
        (profile, selected)
    };
    let mut argv = vec!["versions".to_owned(), "--beta".to_owned()];
    if refresh {
        argv.push("--refresh".to_owned());
    }
    // Свежий индекс версий тоже скачивается молча: stall-таймаут с watchdog'ом
    // не убивает медленную сеть (файлы индекса растут на диске).
    let mut watchdog = DownloadWatchdog::new(&profile);
    let (code, lines) =
        bol_run(&argv, &profile, "versions", None, None, Duration::from_secs(300), 10_000, false, None, None, Some(&mut watchdog))
            .map_err(|error| format!("Не удалось загрузить список версий Minecraft: {error}"))?;
    if code != 0 {
        let detail = lines.last().cloned().unwrap_or_else(|| format!("код {code}"));
        return Err(format!("Не удалось загрузить список версий Minecraft: {detail}"));
    }
    let mut parsed = parse_game_versions(&lines);
    // Выбранная в профиле версия тоже установленная — иначе после успешного
    // setup список показывал «Будет загружена».
    if !selected.0.is_empty() && !selected.1.is_empty() {
        for item in parsed.iter_mut() {
            if item.get("edition").and_then(Value::as_str) == Some(&selected.0)
                && item.get("version").and_then(Value::as_str) == Some(&selected.1)
            {
                item["installed"] = json!(true);
            }
        }
    }
    Ok(json!({
        "versions": parsed,
        "current_edition": selected.0,
        "current_version": selected.1,
    }))
}

fn windows_versions(name: &str) -> Result<Value, String> {
    let mut list: Vec<Value> = Vec::new();
    for edition in ["release", "preview"] {
        let state = windows_package_state(edition);
        let entry = if state.installed && !state.version.is_empty() {
            json!({ "edition": edition, "version": state.version, "installed": true })
        } else {
            // Официальный Store всегда ставит последний выпуск; конкретную
            // версию для Windows выбрать нельзя — ставим «latest».
            json!({ "edition": edition, "version": "latest", "installed": false })
        };
        list.push(entry);
    }
    let (current_edition, current_version) = if name.is_empty() {
        (String::new(), String::new())
    } else {
        let store = instance_store()?;
        let info = bedrock_instance(&store, name)?;
        windows_selected(&info.path)
    };
    Ok(json!({
        "versions": list,
        "current_edition": current_edition,
        "current_version": current_version,
    }))
}

// ── установка версии ──

pub fn setup_version(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let edition = request_field(request, "edition")?.trim().to_lowercase();
    let version = request_field(request, "version")?.trim().to_owned();
    if !["release", "preview"].contains(&edition.as_str()) || !valid_game_version(&version) {
        return Err("Выберите доступную редакцию и версию Minecraft Bedrock".to_owned());
    }
    let store = instance_store()?;
    let info = bedrock_instance(&store, &name)?;
    if info.running {
        return Err("Сначала закройте Minecraft Bedrock".to_owned());
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let job_name = name.clone();
            spawn_job("setup", &name, move |job, cancellation| {
                match linux_setup(&job_name, &edition, &version, Some(cancellation), &job) {
                    Ok(value) => job_finish(&job, value),
                    Err(error) => job_fail(&job, &error, cancellation.cancelled()),
                }
            })
        }
        BedrockPlatform::WindowsOfficial => {
            let job_name = name.clone();
            spawn_job("setup", &name, move |job, cancellation| {
                match windows_setup(&job_name, &edition, &version, Some(cancellation), &job) {
                    Ok(value) => job_finish(&job, value),
                    Err(error) => job_fail(&job, &error, cancellation.cancelled()),
                }
            })
        }
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

const LOGIN_ERROR_MARKERS: &[&str] = &[
    "sign in",
    "sign-in",
    "store-login",
    "not signed in",
    "login required",
    "authentication required",
    "authentication failed",
    "account required",
    "not logged in",
    "xodus",
    "msa",
];

fn linux_setup(
    name: &str,
    edition: &str,
    version: &str,
    cancellation: Option<&CancellationToken>,
    job: &Arc<Mutex<JobState>>,
) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut logger = |line: &str| job_log(job, line);
    let argv = vec![
        "setup".to_owned(),
        "--mc".to_owned(),
        edition.to_owned(),
        "--version".to_owned(),
        version.to_owned(),
        "--beta".to_owned(),
    ];
    // Игра (2+ ГиБ) скачивается почти без вывода в stdout: без watchdog'а
    // stall-таймаут убивал живую загрузку, а BoL каждый ретрай начинал с нуля.
    let mut watchdog = DownloadWatchdog::new(&profile);
    let (code, logs) =
        bol_run(&argv, &profile, "setup", cancellation, Some(&mut logger), Duration::from_secs(600), 500, false, None, None, Some(&mut watchdog))?;
    let (selected_edition, selected_version) = selected_version(&profile);
    // BoL ПАДАЕТ С КОДОМ 0: источник правды — строки «error» и settings.json.
    let error_lines: Vec<String> = logs
        .iter()
        .filter(|line| bol_error_line_re().is_match(line.trim()))
        .cloned()
        .collect();
    let login_scan: Vec<String> = if !error_lines.is_empty() {
        error_lines.clone()
    } else {
        logs.iter()
            .filter(|line| {
                let lower = line.to_lowercase();
                lower.contains("sign in with") || lower.contains("store-login")
            })
            .cloned()
            .collect()
    };
    let requires_login = login_scan.iter().any(|line| {
        let lower = line.to_lowercase();
        LOGIN_ERROR_MARKERS.iter().any(|marker| lower.contains(marker))
    });
    let mut error_hint = String::new();
    if !error_lines.is_empty() || code != 0 {
        let markers = ["error", "failed", "fail ", "невозможно", "не удалось", "denied", "refused", "timeout"];
        error_hint = logs
            .iter()
            .rev()
            .map(|line| line.trim())
            .find(|line| {
                !line.is_empty()
                    && (bol_error_line_re().is_match(line)
                        || markers.iter().any(|marker| line.to_lowercase().contains(marker)))
            })
            .unwrap_or("")
            .chars()
            .take(300)
            .collect();
    }
    let installed = selected_edition == edition && selected_version == version;
    let failed = !error_lines.is_empty() || code != 0;
    let tail_start = logs.len().saturating_sub(40);
    Ok(json!({
        "exit_code": code,
        "logs": logs.clone(),
        "logs_tail": logs[tail_start..],
        "error_hint": error_hint,
        "requires_login": requires_login,
        "installed": installed,
        "failed": failed,
        "current_edition": selected_edition,
        "current_version": selected_version,
    }))
}

fn windows_setup(
    name: &str,
    edition: &str,
    version: &str,
    cancellation: Option<&CancellationToken>,
    job: &Arc<Mutex<JobState>>,
) -> Result<Value, String> {
    let channel = win_channel(edition);
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    job_log(job, &format!("Официальная установка: {} (Microsoft Store).", channel.title));
    let mut state = windows_package_state(edition);
    if !state.installed {
        // 1) Автоматическая попытка winget: срабатывает не для всех учётных
        //    записей — платные/Xbox-продукты Store лицензирует только сам
        //    Store под аккаунтом пользователя.
        if let Some(winget) = winget_executable() {
            job_log(job, &format!("Пробую установку через winget ({}):", channel.product_id));
            let mut command = Command::new(&winget);
            command
                .args([
                    "install",
                    "--id",
                    channel.product_id,
                    "--exact",
                    "--source",
                    "msstore",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            prepare_command(&mut command);
            let log_job = Arc::clone(job);
            let result = run_job_process(
                &mut command,
                cancellation,
                &mut move |line: &str| job_log(&log_job, line),
                // Игра весит гигабайты: медленная сеть легально качается часами.
                // Потолок — защита от вечного winget, тишина 10 мин — от тупика.
                Duration::from_secs(3 * 60 * 60),
                Duration::from_secs(10 * 60),
            );
            match result {
                Ok(0) => job_log(job, "winget завершил установку."),
                Ok(code) => job_log(
                    job,
                    &format!("winget сообщил код {code} (платные продукты Store ставит сам Store)."),
                ),
                Err(message) => job_log(job, &format!("winget: {message}")),
            }
        } else {
            job_log(job, "winget (App Installer) не найден — используется Microsoft Store.");
        }
        invalidate_windows_package_cache();
        state = windows_package_state(edition);
    }
    if !state.installed {
        // 2) Официальная точка входа: страница продукта в Microsoft Store.
        job_log(job, "Открываю страницу Minecraft в Microsoft Store — нажмите «Установить».");
        let _ = open_store_product(edition);
        invalidate_windows_package_cache();
        state = windows_package_state(edition);
        let logs = job_logs(job);
        let tail_start = logs.len().saturating_sub(40);
        let hint = format!(
            "Установите {} на открытой странице Microsoft Store (или вручную: {}), затем повторите установку в Nivora — все зависимости ставит Store автоматически.",
            channel.title, STORE_PAGE_LINK_RU
        );
        if !state.installed {
            return Ok(json!({
                "exit_code": 0,
                "logs": logs.clone(),
                "logs_tail": logs[tail_start..],
                "error_hint": hint,
                "requires_login": false,
                "installed": false,
                "failed": false,
                "current_edition": "",
                "current_version": "",
            }));
        }
    }
    let version_value = if state.version.is_empty() { version.to_owned() } else { state.version.clone() };
    persist_windows_selection(&info.path, edition, &version_value)
        .map_err(|error| format!("Не удалось сохранить выбранную версию: {error}"))?;
    job_log(job, &format!("{} установлен: {version_value}.", channel.title));
    let logs = job_logs(job);
    let tail_start = logs.len().saturating_sub(40);
    Ok(json!({
        "exit_code": 0,
        "logs": logs.clone(),
        "logs_tail": logs[tail_start..],
        "error_hint": "",
        "requires_login": false,
        "installed": true,
        "failed": false,
        "current_edition": edition,
        "current_version": version_value,
    }))
}

/// Запуск процесса с потоковым логом в задание, жёстким потолком времени и
/// детекцией зависания: медленное скачивание winget может идти часами
/// (прогресс перерисовывается `\r` и теперь считается выводом), а вот полная
/// тишина дольше stall-таймаута — признак тупика.
#[allow(clippy::too_many_arguments)]
fn run_job_process(
    command: &mut Command,
    cancellation: Option<&CancellationToken>,
    log: &mut dyn FnMut(&str),
    hard_timeout: Duration,
    stall_timeout: Duration,
) -> Result<i32, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("не удалось запустить: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (sender, receiver) = mpsc::channel::<Option<String>>();
    spawn_line_reader(stdout, sender.clone());
    spawn_line_reader(stderr, sender);
    let deadline = Instant::now() + hard_timeout;
    let mut last_output = Instant::now();
    loop {
        while let Ok(Some(line)) = receiver.try_recv() {
            last_output = Instant::now();
            log(&line);
        }
        if let Some(token) = cancellation {
            if token.cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("операция остановлена пользователем".to_owned());
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                while let Ok(Some(line)) = receiver.try_recv() {
                    log(&line);
                }
                return Ok(status.code().unwrap_or(-1));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("превышено время ожидания установки".to_owned());
                }
                if !stall_timeout.is_zero() && last_output.elapsed() > stall_timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "установщик перестал отвечать ({} мин без прогресса) и был остановлен",
                        stall_timeout.as_secs() / 60
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

pub fn setup_status(request: &Value) -> Result<Value, String> {
    Ok(job_snapshot("setup", request_field(request, "name")?))
}

pub fn setup_cancel(request: &Value) -> Result<Value, String> {
    Ok(json!({ "stopped": cancel_job("setup", request_field(request, "name")?) }))
}

// ── вход в Microsoft Store ──

pub fn store_login(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let store = instance_store()?;
    let info = bedrock_instance(&store, &name)?;
    if info.running {
        return Err("Сначала закройте Minecraft Bedrock".to_owned());
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let job_name = name.clone();
            spawn_job("login", &name, move |job, cancellation| {
                match linux_store_login(&job_name, Some(cancellation), &job) {
                    Ok(value) => job_finish(&job, value),
                    Err(error) => job_fail(&job, &error, cancellation.cancelled()),
                }
            })
        }
        BedrockPlatform::WindowsOfficial => spawn_job("login", &name, move |job, _cancellation| {
            // Официальная версия подписывается в самой игре (Xbox Live);
            // отдельного входа через лаунчер не требуется.
            job_log(&job, "Официальный Minecraft for Windows выполняет вход внутри игры:");
            job_log(&job, "запустите игру и войдите в аккаунт Microsoft/Xbox — Nivora ничего не хранит.");
            job_finish(
                &job,
                json!({
                    "exit_code": 0,
                    "logs": [],
                    "logs_tail": [],
                    "error_hint": "",
                    "failed": false,
                }),
            )
        }),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn linux_store_login(
    name: &str,
    cancellation: Option<&CancellationToken>,
    job: &Arc<Mutex<JobState>>,
) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut logger = |line: &str| job_log(job, line);
    let (code, logs) = bol_run(
        &["store-login".to_owned()],
        &profile,
        &format!("bedrock-login:{name}"),
        cancellation,
        Some(&mut logger),
        Duration::ZERO,
        500,
        false,
        None,
        None,
        None,
    )?;
    let error_hint: String = logs
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty() && bol_error_line_re().is_match(line))
        .unwrap_or("")
        .chars()
        .take(300)
        .collect();
    let tail_start = logs.len().saturating_sub(40);
    Ok(json!({
        "exit_code": code,
        "logs": logs.clone(),
        "logs_tail": logs[tail_start..],
        "error_hint": error_hint,
        "failed": !error_hint.is_empty() || code != 0,
    }))
}

pub fn store_login_status(request: &Value) -> Result<Value, String> {
    Ok(job_snapshot("login", request_field(request, "name")?))
}

/// Подключён ли аккаунт Microsoft Store к профилю BedrockOnLinux.
/// Зеркалит xodus.signed_in(): лицензия xodus живёт в RON-ключнице
/// <профиль>/xodus-home/.xodus-keyring.ron, вход выполнен, когда там есть
/// и «user-tokens», и «user-DA» (одни девайс-записи — это ещё не вход).
/// Файл маленький, но читаем с потолком — как все файлы состояния.
fn bol_profile_signed_in(profile: &Path) -> bool {
    keyring_file_signed_in(&profile.join("xodus-home").join(".xodus-keyring.ron"))
}

fn keyring_file_signed_in(keyring: &Path) -> bool {
    const MAX_KEYRING_BYTES: u64 = 1024 * 1024;
    let Ok(meta) = fs::metadata(keyring) else { return false };
    if !meta.is_file() || meta.len() > MAX_KEYRING_BYTES {
        return false;
    }
    let Ok(blob) = fs::read(keyring) else { return false };
    let contains = |marker: &[u8]| blob.windows(marker.len()).any(|window| window == marker);
    contains(b"\"user-tokens\"") && contains(b"\"user-DA\"")
}

/// Общая ключница xodus для ВСЕХ профилей Bedrock.
///
/// Лицензия xodus хранится в <профиль>/xodus-home/.xodus-keyring.ron, а
/// профиль у каждого экземпляра свой — раньше вход приходилось выполнять
/// в каждом профиле отдельно, и каждый вход регистрировал Microsoft Store
/// очередное устройство (лимит аккаунта ограничен, дальше «Device group is
/// full»); единственный вход мог «застрять» в сломанном экземпляре. Теперь
/// все профили ссылаются на один общий <root>/xodus-home: вход выполняется
/// один раз и виден каждому экземпляру.
///
/// Первый запуск поднимает в общий дом найденный вход — в том числе из
/// профиля, чей экземпляр уже не используется: после этого такой экземпляр
/// можно удалять, лицензия остаётся в общем доме. Локальные копии не
/// удаляются, а переименовываются в xodus-home.local (rmtree профиля
/// симлинки не следует — общий дом переживёт удаление любого профиля).
fn share_xodus_keyring(paths: &ProfilePaths) {
    let shared = paths.root.join("xodus-home");
    if xodus_is_symlink(&shared) || fs::create_dir_all(&shared).is_err() {
        return;
    }
    let shared_keyring = shared.join(".xodus-keyring.ron");
    let mut shared_signed = keyring_file_signed_in(&shared_keyring);
    let Ok(entries) = fs::read_dir(&paths.profiles_root) else { return };
    for entry in entries.flatten() {
        let profile = entry.path();
        if xodus_is_symlink(&profile) || !profile.is_dir() {
            continue;
        }
        let local = profile.join("xodus-home");
        if xodus_is_symlink(&local) {
            continue; // уже на общем доме
        }
        if !local.try_exists().unwrap_or(false) {
            let _ = link_shared_xodus(&shared, &local);
            continue;
        }
        if !local.is_dir() {
            continue;
        }
        let local_keyring = local.join(".xodus-keyring.ron");
        // Поднимаем первый найденный вход в общий дом. Если нашлось
        // несколько (разные аккаунты в разных профилях) — побеждает
        // первый, остальные остаются в xodus-home.local для ручного
        // восстановления.
        if !shared_signed && keyring_file_signed_in(&local_keyring) {
            if fs::rename(&local_keyring, &shared_keyring).is_ok() {
                shared_signed = true;
            }
        }
        let retired = profile.join("xodus-home.local");
        if !retired.exists() {
            let _ = fs::rename(&local, &retired);
        } else {
            let _ = fs::remove_dir_all(&local);
        }
        let _ = link_shared_xodus(&shared, &local);
    }
}

fn link_shared_xodus(shared: &Path, local: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(shared, local)
    }
    #[cfg(not(unix))]
    {
        let _ = (shared, local);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "символические ссылки недоступны"))
    }
}

/// Символическая ссылка (Unix) или точка повторной обработки (Windows).
fn xodus_is_symlink(path: &Path) -> bool {
    #[cfg(unix)]
    {
        fs::symlink_metadata(path).map(|meta| meta.file_type().is_symlink()).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        is_reparse_point(path)
    }
}

/// Сообщение-действие: игра зашифрована лицензией аккаунта-владельца,
/// без повторной привязки xodus не стартует (NotSignedIn в BoL).
const BEDROCK_LOGIN_REQUIRED_MESSAGE: &str = "Для запуска Bedrock нужен подключённый аккаунт Microsoft, которому принадлежит игра. Нажмите «Подключить аккаунт» в карточке «Лицензия Microsoft Store» на вкладке «Обзор» и запустите игру снова.";

/// Текущее состояние привязки аккаунта Microsoft для экземпляра Bedrock.
/// Интерфейс показывает его в карточке лицензии всегда, а не только после
/// попытки входа: без привязки игра не стартует вовсе.
pub fn account_status(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?;
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let ctx = context()?;
            let store = instance_store()?;
            let info = bedrock_instance(&store, &name)?;
            let profile = linux_profile_path(&ctx, &info)?;
            Ok(json!({
                "signed_in": bol_profile_signed_in(&profile),
                "platform": "linux-bol",
            }))
        }
        BedrockPlatform::WindowsOfficial => Ok(json!({
            // Официальная версия подписывается в самой игре (Xbox Live).
            "signed_in": true,
            "platform": "windows-official",
        })),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

pub fn store_login_cancel(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let stopped_job = cancel_job("login", &name);
    let stopped_process = stop_registered_process(&format!("bedrock-login:{name}"));
    Ok(json!({ "stopped": stopped_job || stopped_process }))
}

// ── создание экземпляра ──

pub fn create_instance(request: &Value) -> Result<Value, String> {
    let raw_name = request_field(request, "name")?;
    let display_name = request_field(request, "display_name").unwrap_or_default().trim().to_owned();
    let instance_name = safe_instance_name(raw_name);
    if instance_name.is_empty() || instance_name == "." || instance_name == ".." {
        return Err("Укажите имя экземпляра Bedrock".to_owned());
    }
    let ctx = context()?;
    let store = instance_store()?;
    let instance_path = store.root().join(&instance_name);
    if instance_path.exists() {
        return Err(format!("Экземпляр уже существует: {instance_name}"));
    }
    let pretty_name = if display_name.is_empty() { instance_name.clone() } else { display_name.chars().take(80).collect() };
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let paths = profile_paths(&ctx)?;
            bol_require_runtime(&paths.root)?;
            // Новый экземпляр сразу получает доступ к общему входу Microsoft.
            share_xodus_keyring(&paths);
            let profile = profile_name(&instance_name);
            let profile_path = paths.profiles_root.join(&profile);
            if profile_path.exists() {
                return Err("Для этого имени уже существует профиль Bedrock; сначала удалите его в библиотеке".to_owned());
            }
            let shortcut_storage = paths.root.join(".nivora-xdg-data");
            fs::create_dir_all(&shortcut_storage).map_err(|_| "Каталог ярлыков Bedrock недоступен".to_owned())?;
            let (code, _logs) = bol_run(
                &["profiles".to_owned(), "create".to_owned(), profile.clone()],
                &paths.root,
                "profiles-create",
                None,
                None,
                Duration::from_secs(120),
                100,
                true,
                Some(&shortcut_storage),
                None,
                None,
            )?;
            if code != 0 {
                return Err(format!("Не удалось создать профиль Bedrock (код {code})"));
            }
            if let Err(error) = validate_profile_path(&paths, &profile) {
                let _ = fs::remove_dir_all(&profile_path);
                return Err(error);
            }
            let metadata = json!({
                "kind": "client",
                "game_edition": "bedrock",
                "bedrock_profile": profile,
                "display_name": pretty_name,
                "description": "Minecraft Bedrock Edition",
                "group": "",
                "played_seconds": 0,
            });
            fs::create_dir_all(&instance_path).map_err(|_| "Не удалось создать каталог экземпляра".to_owned())?;
            super::platform::restrict_dir(&instance_path);
            if let Err(error) = write_instance_metadata(&instance_path, &metadata) {
                let _ = fs::remove_dir_all(&instance_path);
                let _ = fs::remove_dir_all(&profile_path);
                return Err(error);
            }
        }
        BedrockPlatform::WindowsOfficial => {
            // Официальной версии профиль среды не нужен — только экземпляр.
            let metadata = json!({
                "kind": "client",
                "game_edition": "bedrock",
                "bedrock_profile": "",
                "display_name": pretty_name,
                "description": "Minecraft Bedrock Edition (Microsoft Store)",
                "group": "",
                "played_seconds": 0,
            });
            fs::create_dir_all(&instance_path).map_err(|_| "Не удалось создать каталог экземпляра".to_owned())?;
            super::platform::restrict_dir(&instance_path);
            write_instance_metadata(&instance_path, &metadata)?;
        }
        BedrockPlatform::Unsupported => return Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
    let refreshed = instance_store()?;
    let info = bedrock_instance(&refreshed, &instance_name)?;
    Ok(json!({
        "instance": info.public(),
        "instances": refreshed.list().iter().map(|item| item.public()).collect::<Vec<Value>>(),
    }))
}

/// Путь профиля BoL для экземпляра (только Linux).
fn linux_profile_path(ctx: &Ctx, info: &InstanceInfo) -> Result<PathBuf, String> {
    let expected = profile_name(&info.name);
    if info.bedrock_profile != expected {
        return Err("Профиль Bedrock не совпадает с экземпляром".to_owned());
    }
    let paths = profile_paths(ctx)?;
    // Один общий вход Microsoft для всех экземпляров: консолидация
    // идемпотентна и дёшева (обход профилей), поэтому — на каждом resolve.
    share_xodus_keyring(&paths);
    validate_profile_path(&paths, &expected)
}

// ── контент ──

pub fn content_list(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?;
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_content_list(name),
        BedrockPlatform::WindowsOfficial => windows_content_list(name),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn scope_tail(scope: &str) -> String {
    let chars: Vec<char> = scope.chars().collect();
    if chars.len() > 4 {
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("…{tail}")
    } else {
        scope.to_owned()
    }
}

/// Общий обход каталогов контента; roots — пары (scope, games/com.mojang).
fn scan_content(roots: &[(String, PathBuf)]) -> Result<Vec<Value>, String> {
    let mut values: Vec<Value> = Vec::new();
    for (scope, content_root) in roots {
        if is_reparse_point(content_root) || !content_root.is_dir() {
            continue;
        }
        for (category, directories) in CONTENT_DIRECTORIES {
            for directory_name in *directories {
                let directory = content_root.join(directory_name);
                if is_reparse_point(&directory) || !directory.is_dir() {
                    continue;
                }
                let mut entries: Vec<_> = match fs::read_dir(&directory) {
                    Ok(iter) => iter.filter_map(|entry| entry.ok()).collect(),
                    Err(_) => continue,
                };
                entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
                for entry in entries {
                    let path = entry.path();
                    if is_reparse_point(&path) {
                        continue;
                    }
                    let Ok(meta) = entry.metadata() else { continue };
                    let is_dir = meta.is_dir();
                    values.push(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "scope": scope,
                        "scope_label": if scope == "LocalState" {
                            "Данные игры".to_owned()
                        } else {
                            format!("Профиль аккаунта {}", scope_tail(scope))
                        },
                        "category": category,
                        "directory": directory_name,
                        "item_kind": if is_dir { "directory" } else { "file" },
                        "size": if is_dir { 0 } else { meta.len() },
                        "modified_at": meta
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|value| value.as_secs_f64())
                            .unwrap_or(0.0),
                    }));
                    if values.len() >= 5000 {
                        return Ok(values);
                    }
                }
            }
        }
    }
    Ok(values)
}

fn linux_content_list(name: &str) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut roots: Vec<(String, PathBuf)> = Vec::new();
    let users_root = safe_profile_path(&profile, BOL_CONTENT_ROOT_PARTS)?;
    if users_root.is_dir() {
        if let Ok(entries) = fs::read_dir(&users_root) {
            let mut scopes: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| !is_reparse_point(path) && path.is_dir())
                .collect();
            scopes.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
            for scope in scopes {
                let scope_name =
                    scope.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
                if !scope_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    continue;
                }
                let mut parts = BOL_CONTENT_ROOT_PARTS.to_vec();
                parts.extend_from_slice(&[scope_name.as_str(), "games", "com.mojang"]);
                let content_root = safe_profile_path(&profile, &parts)?;
                roots.push((scope_name, content_root));
            }
        }
    }
    Ok(json!({ "items": scan_content(&roots)? }))
}

fn windows_content_list(name: &str) -> Result<Value, String> {
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let (edition, _) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release" } else { edition.as_str() };
    let content_root = win_content_root(edition)?;
    Ok(json!({ "items": scan_content(&[("LocalState".to_owned(), content_root)])? }))
}

/// Скриншоты Bedrock: игра кладёт снимки (камера/Win+G) в com.mojang/
/// minecraftScreenshots. Возвращаем записи с путём ОТНОСИТЕЛЬНО корня
/// файловых операций экземпляра (Linux — профиль BoL, Windows — com.mojang),
/// чтобы фронтенд читал их штатным instance_file_read(base64).
pub fn screenshots_list(request: &Value) -> Result<Value, String> {
    const MAX_SCREENSHOTS: usize = 500;
    let name = request_field(request, "name")?.to_owned();
    let mut items: Vec<Value> = Vec::new();

    let mut scan = |label: String, content_root: &Path, read_prefix: &[String], items: &mut Vec<Value>| -> Result<(), String> {
        let directory = content_root.join("minecraftScreenshots");
        if !directory.is_dir() || is_reparse_point(&directory) {
            return Ok(());
        }
        let entries = fs::read_dir(&directory).map_err(|_| "Не удалось прочитать каталог скриншотов Bedrock".to_owned())?;
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if reserved_screenshot_name(&file_name) || is_reparse_point(&entry.path()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else { continue };
            if !metadata.is_file() {
                continue;
            }
            let lower = file_name.to_lowercase();
            if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
                continue;
            }
            let mut parts = read_prefix.to_vec();
            parts.push("minecraftScreenshots".to_owned());
            parts.push(file_name.clone());
            items.push(json!({
                "name": file_name,
                "read_path": parts.join("/"),
                "scope_label": label,
                "size": metadata.len() as i64,
                "modified_at": metadata.modified().ok().and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok()).map(|value| value.as_secs() as i64).unwrap_or(0),
            }));
            if items.len() >= MAX_SCREENSHOTS {
                break;
            }
        }
        Ok(())
    };

    if cfg!(windows) {
        let store = instance_store()?;
        let info = bedrock_instance(&store, &name)?;
        let (edition, _) = windows_selected(&info.path);
        let edition = if edition.is_empty() { "release" } else { edition.as_str() };
        let content_root = win_content_root(edition)?;
        scan("Скриншоты игры".to_owned(), &content_root, &[], &mut items)?;
    } else {
        let ctx = context()?;
        let store = instance_store()?;
        let info = bedrock_instance(&store, &name)?;
        let profile = linux_profile_path(&ctx, &info)?;
        let users_root = safe_profile_path(&profile, BOL_CONTENT_ROOT_PARTS)?;
        if users_root.is_dir() {
            if let Ok(entries) = fs::read_dir(&users_root) {
                let mut scopes: Vec<PathBuf> = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| !is_reparse_point(path) && path.is_dir())
                    .collect();
                scopes.sort_by_key(|path| path.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default());
                for scope in scopes {
                    let scope_name = scope.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
                    if !scope_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                        continue;
                    }
                    let mut parts = BOL_CONTENT_ROOT_PARTS.to_vec();
                    parts.push(scope_name.as_str());
                    let mut prefix: Vec<String> = parts.iter().map(|value| value.to_string()).collect();
                    prefix.extend_from_slice(&["games".to_owned(), "com.mojang".to_owned()]);
                    let mut content_root = profile.to_path_buf();
                    for part in &prefix {
                        content_root = content_root.join(part);
                    }
                    scan(format!("Профиль аккаунта {}", scope_tail(&scope_name)), &content_root, &prefix, &mut items)?;
                    if items.len() >= MAX_SCREENSHOTS {
                        break;
                    }
                }
            }
        }
    }
    items.sort_by(|left, right| right["modified_at"].as_i64().cmp(&left["modified_at"].as_i64()));
    Ok(json!({ "items": items }))
}

/// Служебные и скрытые имена в каталоге скриншотов не показываем.
fn reserved_screenshot_name(name: &str) -> bool {
    name.starts_with('.') || name == "Thumbs.db" || name.is_empty()
}

fn safe_profile_path(profile: &Path, parts: &[&str]) -> Result<PathBuf, String> {
    if is_reparse_point(profile) {
        return Err("Путь контента Bedrock содержит символьную ссылку".to_owned());
    }
    let root = profile.canonicalize().unwrap_or_else(|_| profile.to_path_buf());
    let mut path = profile.to_path_buf();
    for part in parts {
        if part.is_empty() || *part == "." || *part == ".." || part.contains('/') || part.contains('\\') || part.contains('\0') {
            return Err("Недопустимый путь контента Bedrock".to_owned());
        }
        path = path.join(part);
        if is_reparse_point(&path) {
            return Err("Путь контента Bedrock содержит символьную ссылку".to_owned());
        }
        if let Ok(resolved) = path.canonicalize() {
            if !resolved.starts_with(&root) {
                return Err("Путь контента Bedrock выходит за пределы профиля".to_owned());
            }
        }
    }
    Ok(path)
}

pub fn content_import(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let file_name = request_field(request, "file_name")?.to_owned();
    let data = request_field(request, "data").unwrap_or_default();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| "Файл Bedrock не является корректными base64-данными".to_owned())?;
    if decoded.is_empty() || decoded.len() > MAX_CONTENT_UPLOAD_BYTES {
        return Err("Файл Bedrock пуст или превышает лимит 128 MiB".to_owned());
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_content_import(&name, &file_name, decoded),
        BedrockPlatform::WindowsOfficial => windows_content_import(&name, &file_name, decoded),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn import_extension(file_name: &str) -> Result<String, String> {
    let suffix = Path::new(file_name)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if IMPORT_EXTENSIONS.contains(&suffix.as_str()) {
        Ok(suffix)
    } else {
        Err("Bedrock поддерживает .mcpack, .mcaddon, .mcworld, .mctemplate и .mcskin".to_owned())
    }
}

fn linux_content_import(name: &str, file_name: &str, content: Vec<u8>) -> Result<Value, String> {
    let suffix = import_extension(file_name)?;
    validate_content_archive(&content)?;
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    bol_require_runtime(&profile)?;
    let temp_root = ctx.cache_home.join("bedrock-imports");
    if is_reparse_point(&temp_root) {
        return Err("Временный каталог Bedrock является символьной ссылкой".to_owned());
    }
    fs::create_dir_all(&temp_root).map_err(|_| "Временный каталог Bedrock недоступен".to_owned())?;
    if is_symlink_or_missing_dir(&temp_root) {
        return Err("Временный каталог Bedrock недоступен".to_owned());
    }
    let staged = temp_root.join(format!(".nivora-bedrock-{}{suffix}", random_hex(6)));
    {
        let mut file = super::platform::create_private(&staged)
            .map_err(|_| "Не удалось подготовить файл импорта".to_owned())?;
        file.write_all(&content)
            .map_err(|_| "Не удалось подготовить файл импорта".to_owned())?;
    }
    let result = bol_run(
        &["import".to_owned(), staged.to_string_lossy().into_owned()],
        &profile,
        "import",
        None,
        None,
        Duration::from_secs(600),
        500,
        false,
        None,
        None,
        None,
    );
    let _ = fs::remove_file(&staged);
    let (code, logs) = result.map_err(|error| format!("Не удалось импортировать контент Bedrock: {error}"))?;
    if code != 0 {
        let detail = logs.last().cloned().unwrap_or_else(|| format!("код {code}"));
        return Err(format!("Не удалось импортировать контент Bedrock: {detail}"));
    }
    Ok(json!({
        "items": linux_content_list(name)?.get("items").cloned().unwrap_or_default(),
        "logs": logs,
    }))
}

/// Классификация архива по манифесту: resource/behavior/skin/world.
/// Возвращает пары (целевой каталог, корень внутри архива).
fn classify_archive(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Result<Vec<(String, String)>, String> {
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| archive.by_index(index).ok().map(|entry| entry.name().replace('\\', "/")))
        .collect();
    let has_manifest = |prefix: &str| {
        names.iter().any(|name| {
            name.as_str() == format!("{prefix}manifest.json") || name.as_str() == format!("{prefix}/manifest.json")
        })
    };
    let has_world_files = |prefix: &str| {
        names.iter().any(|name| {
            name.as_str() == format!("{prefix}levelname.txt")
                || name.as_str() == format!("{prefix}level.dat")
                || name.as_str() == format!("{prefix}/levelname.txt")
                || name.as_str() == format!("{prefix}/level.dat")
        })
    };
    if has_manifest("") || has_world_files("") {
        let mut roots = Vec::new();
        classify_single_root(archive, "", &mut roots)?;
        return Ok(roots);
    }
    // .mcaddon: несколько паков в подкаталогах.
    let mut directories: Vec<String> = Vec::new();
    for name in &names {
        if let Some(index) = name.find('/') {
            let directory = name[..index].to_owned();
            if !directory.is_empty() && !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    directories.sort();
    let mut roots: Vec<(String, String)> = Vec::new();
    let mut found_any = false;
    for directory in &directories {
        if has_manifest(directory) || has_world_files(directory) {
            found_any = true;
            classify_single_root(archive, directory, &mut roots)?;
        }
    }
    if !found_any {
        return Err("В архиве не найден manifest.json ни данные мира (levelname.txt/level.dat)".to_owned());
    }
    Ok(roots)
}

fn classify_single_root(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    prefix: &str,
    roots: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let prefix_slash = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
    if let Some(manifest) = read_manifest_value(archive, &prefix_slash) {
        let mut target = "resource_packs";
        if let Some(modules) = manifest.get("modules").and_then(Value::as_array) {
            for module in modules {
                let kind = module.get("type").and_then(Value::as_str).unwrap_or("");
                if kind == "data" {
                    target = "behavior_packs";
                    break;
                }
                if kind == "skin_pack" {
                    target = "skin_packs";
                    break;
                }
                if kind == "resources" {
                    target = "resource_packs";
                }
            }
        }
        roots.push((target.to_owned(), prefix.to_owned()));
        return Ok(());
    }
    // Мир без манифеста: levelname.txt / level.dat.
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| archive.by_index(index).ok().map(|entry| entry.name().replace('\\', "/")))
        .collect();
    if names
        .iter()
        .any(|name| name.as_str() == format!("{prefix_slash}levelname.txt") || name.as_str() == format!("{prefix_slash}level.dat"))
    {
        roots.push(("minecraftWorlds".to_owned(), prefix.to_owned()));
        return Ok(());
    }
    Err("В архиве не найден manifest.json ни данные мира".to_owned())
}

fn read_manifest_value(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, prefix_slash: &str) -> Option<Value> {
    let candidates = [
        format!("{prefix_slash}manifest.json"),
        format!("{}.manifest.json", prefix_slash.trim_end_matches('/')),
    ];
    for candidate in candidates {
        if let Ok(mut entry) = archive.by_name(&candidate) {
            let mut text = String::new();
            if entry.read_to_string(&mut text).is_ok() {
                if let Ok(value) = serde_json::from_str(&text) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn windows_content_import(name: &str, file_name: &str, content: Vec<u8>) -> Result<Value, String> {
    let suffix = import_extension(file_name)?;
    let mut archive = validate_content_archive(&content)?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let (edition, _) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release" } else { edition.as_str() };
    let content_root = win_content_root(edition)?;
    // Быстрая классификация по расширению, точная — по манифесту.
    let targets: Vec<(String, String)> = match suffix.as_str() {
        ".mcworld" | ".mctemplate" => vec![("minecraftWorlds".to_owned(), String::new())],
        ".mcskin" => vec![("skin_packs".to_owned(), String::new())],
        _ => classify_archive(&mut archive)?,
    };
    let pack_name = Path::new(file_name)
        .file_stem()
        .map(|value| sanitize_content_name(&value.to_string_lossy()))
        .unwrap_or_else(|| format!("pack-{}", random_hex(3)));
    let mut imported: Vec<String> = Vec::new();
    for (target_directory, archive_prefix) in &targets {
        let destination = content_root.join(target_directory).join(&pack_name);
        if is_reparse_point(&destination) {
            continue;
        }
        if destination.is_dir() {
            fs::remove_dir_all(&destination).map_err(|error| format!("Не удалось заменить контент: {error}"))?;
        } else if destination.is_file() {
            let _ = fs::remove_file(&destination);
        }
        fs::create_dir_all(&destination).map_err(|error| format!("Не удалось создать каталог контента: {error}"))?;
        super::platform::restrict_dir(&destination);
        let prefix_slash = if archive_prefix.is_empty() { String::new() } else { format!("{archive_prefix}/") };
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| format!("Не удалось прочитать архив: {error}"))?;
            if entry.is_dir() {
                continue;
            }
            let entry_name = entry.name().replace('\\', "/");
            let Some(relative) = entry_name.strip_prefix(&prefix_slash) else { continue };
            if relative.is_empty() || relative.split('/').any(|part| part == ".." || part.is_empty()) {
                continue;
            }
            let target_path = destination.join(relative);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог контента: {error}"))?;
            }
            if is_reparse_point(&target_path) {
                continue;
            }
            let mut file = super::platform::create_private(&target_path)
                .map_err(|error| format!("Не удалось записать контент: {error}"))?;
            std::io::copy(&mut entry, &mut file).map_err(|error| format!("Не удалось записать контент: {error}"))?;
        }
        imported.push(format!("{target_directory}/{pack_name}"));
    }
    Ok(json!({
        "items": windows_content_list(name)?.get("items").cloned().unwrap_or_default(),
        "logs": [format!("Импортировано в официальную версию: {}", imported.join(", "))],
    }))
}

fn sanitize_content_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '_' || c == '-' || c == '.' { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let result: String = trimmed.chars().take(96).collect();
    if result.is_empty() {
        format!("pack-{}", random_hex(3))
    } else {
        result
    }
}

pub fn content_remove(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let scope = request_field(request, "scope").unwrap_or_default().to_owned();
    let directory_name = request_field(request, "directory")?.to_owned();
    let item_name = request_field(request, "item_name")?.to_owned();
    let known: Vec<&str> = CONTENT_DIRECTORIES.iter().flat_map(|(_, dirs)| dirs.iter().copied()).collect();
    if !known.contains(&directory_name.as_str()) {
        return Err("Неизвестный каталог Bedrock".to_owned());
    }
    for value in [&item_name, &scope] {
        let invalid = value.is_empty()
            || *value == "."
            || *value == ".."
            || value.contains('/')
            || value.contains('\\')
            || value.contains('\0')
            || value.chars().any(|c| (c as u32) < 32)
            || value.len() > 255;
        if invalid {
            return Err("Недопустимое имя ресурса Bedrock".to_owned());
        }
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_content_remove(&name, &scope, &directory_name, &item_name),
        BedrockPlatform::WindowsOfficial => windows_content_remove(&name, &directory_name, &item_name),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn linux_content_remove(name: &str, scope: &str, directory_name: &str, item_name: &str) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut parts = BOL_CONTENT_ROOT_PARTS.to_vec();
    parts.extend_from_slice(&[scope, "games", "com.mojang", directory_name, item_name]);
    let target = safe_profile_path(&profile, &parts)?;
    if is_reparse_point(&target) || !target.exists() {
        return Err("Ресурс Bedrock не найден".to_owned());
    }
    if target.is_dir() {
        fs::remove_dir_all(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    } else if target.is_file() {
        fs::remove_file(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    } else {
        return Err("Можно удалить только файл или папку Bedrock".to_owned());
    }
    Ok(json!({ "items": linux_content_list(name)?.get("items").cloned().unwrap_or_default() }))
}

fn windows_content_remove(name: &str, directory_name: &str, item_name: &str) -> Result<Value, String> {
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let (edition, _) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release" } else { edition.as_str() };
    let content_root = win_content_root(edition)?;
    let target = content_root.join(directory_name).join(item_name);
    if is_reparse_point(&target) || !target.exists() {
        return Err("Ресурс Bedrock не найден".to_owned());
    }
    if target.is_dir() {
        fs::remove_dir_all(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    } else if target.is_file() {
        fs::remove_file(&target).map_err(|error| format!("Не удалось удалить контент: {error}"))?;
    }
    Ok(json!({ "items": windows_content_list(name)?.get("items").cloned().unwrap_or_default() }))
}

// ── остановка ──

pub fn stop(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let store = instance_store()?;
    let info = bedrock_instance(&store, &name)?;
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => Ok(json!({ "stopped": stop_registered_process(&info.name) })),
        BedrockPlatform::WindowsOfficial => {
            let stopped = windows_game_pids().into_iter().filter(|pid| windows_kill_pid(*pid)).count();
            Ok(json!({ "stopped": stopped > 0 }))
        }
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

// ── запуск игры ──

/// Быстрая проверка (без побочных эффектов): запускает ли этот запрос
/// Bedrock-сессию, которую ведёт Rust-движок.
pub fn should_intercept_launch(request: &Value) -> bool {
    let operation = request.get("operation").and_then(Value::as_str).unwrap_or_default();
    if !matches!(operation, "launch" | "launch_instance") {
        return false;
    }
    let name = request
        .get("instance_name")
        .or_else(|| request.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if name.is_empty() {
        return false;
    }
    let Ok(store) = instance_store() else { return false };
    let Ok(info) = store.get(name) else { return false };
    info.kind == "client" && info.game_edition == "bedrock"
}

/// Провести Bedrock-сессию до выхода игры. Вызывается только после
/// validate_backend_request: allowlist обязан отсеять запрос ДО запуска.
pub fn launch_intercepted(request: &Value) -> Result<Value, String> {
    let operation = request.get("operation").and_then(Value::as_str).unwrap_or("launch");
    let name = request
        .get("instance_name")
        .or_else(|| request.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err("Не указано имя экземпляра Bedrock".to_owned());
    }
    launch_session(&name, operation)
}

fn launch_session(name: &str, operation: &str) -> Result<Value, String> {
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => linux_launch(name, operation),
        BedrockPlatform::WindowsOfficial => windows_launch(name, operation),
        BedrockPlatform::Unsupported => {
            Err("Официальный Minecraft Bedrock не выпускается для этой ОС".to_owned())
        }
    }
}

fn linux_launch(name: &str, operation: &str) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, &name)?;
    if info.running {
        return Err("Экземпляр Bedrock уже запущен".to_owned());
    }
    let profile = linux_profile_path(&ctx, &info)?;
    bol_require_runtime(&profile)?;
    // Лицензия xodus обязательна на каждом запуске (игра зашифрована под
    // аккаунт-владелец). Проверяем ДО старта: раньше «play» поднимал WineGDK,
    // печатал портянку на английском и умирал — теперь сразу понятное
    // действие, а не лог с рекомендацией «bedrock-on-linux store-login».
    if !bol_profile_signed_in(&profile) {
        return Err(BEDROCK_LOGIN_REQUIRED_MESSAGE.to_owned());
    }
    // Блокировка экземпляра живёт всю игровую сессию — как в Python-версии.
    let mut session_lock = InstanceProcessLock::new(&ctx.data_home, &info.name);
    session_lock.acquire()?;
    // Если установке версии не хватило сети/времени на движок, «play» умирал
    // с «GDK-Proton missing» — доустанавливаем сами (setup идемпотентен).
    if !engine_ready(&profile) {
        let (edition, version) = selected_version(&profile);
        let mut argv = vec!["setup".to_owned(), "--beta".to_owned()];
        if !edition.is_empty() && !version.is_empty() {
            argv.push("--mc".to_owned());
            argv.push(edition.clone());
            argv.push("--version".to_owned());
            argv.push(version.clone());
        }
        let mut watchdog = DownloadWatchdog::new(&profile);
        let (code, logs) =
            bol_run(&argv, &profile, "setup", None, None, Duration::from_secs(600), 100, false, None, None, Some(&mut watchdog))?;
        if !engine_ready(&profile) {
            let joined = logs.join("\n").to_lowercase();
            if LOGIN_ERROR_MARKERS.iter().any(|marker| joined.contains(marker)) {
                return Err(
                    "Для установки окружения Bedrock нужен вход в Microsoft Store: подключите его на вкладке «Обзор» и запустите игру снова"
                        .to_owned(),
                );
            }
            let hint: String = logs
                .iter()
                .rev()
                .map(|line| line.trim())
                .find(|line| {
                    let lower = line.to_lowercase();
                    lower.contains("error") || lower.contains("failed")
                })
                .unwrap_or(format!("код {code}").as_str())
                .chars()
                .take(300)
                .collect();
            return Err(format!("Не удалось доустановить окружение Bedrock: {hint}"));
        }
    }
    let log_id = new_log_id(&info.name);
    let session_log = SessionLog::create(&ctx.data_home, &log_id);
    let started_at = now_secs_f64();
    write_live_state(&info.path, true, 0, None, started_at, &log_id);
    let start = Instant::now();
    let mut logs: Vec<String> = Vec::new();
    let play_result = {
        let mut logger = |line: &str| {
            logs.push(line.to_owned());
            if logs.len() > MAX_SESSION_LOG_LINES {
                let drop = logs.len() - MAX_SESSION_LOG_LINES;
                logs.drain(..drop);
            }
            session_log.append(line);
        };
        // pid процесса BoL нужен метрикам и оверлею: они считают дерево
        // процессов (Wine + игра) от этого корня.
        let session_pid = |pid: u32| {
            write_live_state(&info.path, true, pid, None, started_at, &log_id);
        };
        bol_run(
            &["play".to_owned()],
            &profile,
            &info.name,
            None,
            Some(&mut logger),
            Duration::ZERO,
            MAX_SESSION_LOG_LINES,
            false,
            None,
            Some(&session_pid),
            None,
        )
    };
    let exit_code = match play_result {
        Ok((code, _)) => code,
        Err(error) => {
            write_live_state(&info.path, false, 0, Some(-1), started_at, &log_id);
            let _ = store.add_played_seconds(&info.path, start.elapsed().as_secs() as i64);
            return Err(error);
        }
    };
    write_live_state(&info.path, false, 0, Some(exit_code as i64), started_at, &log_id);
    let _ = store.add_played_seconds(&info.path, start.elapsed().as_secs() as i64);
    // BoL умеет падать с кодом 0/1 без старта игры, когда привязка аккаунта
    // потерялась между проверкой выше и запуском: источник правды — строки
    // NotSignedIn в логе сессии. Возвращаем действие, а не «завершился с кодом».
    if exit_code != 0 {
        let joined = logs.join("\n").to_lowercase();
        let not_signed_in = joined.contains("cannot start until that account is linked again")
            || joined.contains("decrypted with the microsoft account")
            || joined.contains("notsignedin");
        if not_signed_in {
            return Err(BEDROCK_LOGIN_REQUIRED_MESSAGE.to_owned());
        }
    }
    Ok(json!({
        "ok": true,
        "operation": operation,
        "data": { "exit_code": exit_code, "logs": logs, "version_id": "bedrock" },
    }))
}

fn windows_launch(name: &str, operation: &str) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, &name)?;
    if info.running {
        return Err("Экземпляр Bedrock уже запущен".to_owned());
    }
    let (edition, _version) = windows_selected(&info.path);
    let edition = if edition.is_empty() { "release".to_owned() } else { edition };
    let state = windows_package_state(&edition);
    if !state.installed {
        return Err(
            "Официальный Minecraft Bedrock не установлен. Установите его на вкладке «Версия» — Nivora откроет Microsoft Store."
                .to_owned(),
        );
    }
    let log_id = new_log_id(&info.name);
    let session_log = SessionLog::create(&ctx.data_home, &log_id);
    let started_at = now_secs_f64();
    launch_official(&edition)?;
    session_log.append(&format!(
        "Запущен официальный Minecraft Bedrock ({edition}) по протоколу {}.",
        win_channel(&edition).protocol
    ));
    // Ждём появление процесса игры, затем сопровождаем сессию до выхода.
    let mut game_pid: u32 = 0;
    let discover_deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < discover_deadline {
        if let Some(pid) = windows_game_pids().first() {
            game_pid = *pid;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    write_live_state(&info.path, true, game_pid, None, started_at, &log_id);
    let start = Instant::now();
    if game_pid > 0 {
        while process_exists(game_pid as i32) {
            std::thread::sleep(Duration::from_millis(1000));
        }
    } else {
        // Процесс не найден (tasklist недоступен) — не блокируем сессию навсегда.
        std::thread::sleep(Duration::from_secs(5));
    }
    write_live_state(&info.path, false, game_pid, Some(0), started_at, &log_id);
    let _ = store.add_played_seconds(&info.path, start.elapsed().as_secs() as i64);
    session_log.append("Официальный Minecraft Bedrock завершил работу.");
    Ok(json!({
        "ok": true,
        "operation": operation,
        "data": {
            "exit_code": 0,
            "logs": [format!("Официальный Minecraft Bedrock ({edition}) запущен и завершил работу.")],
            "version_id": "bedrock",
        },
    }))
}

// ──────────────────────────────── Тесты ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rust_engine::state::LAUNCHER_DIR;

    fn temp_profile(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("nivora-bol-keyring-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("xodus-home")).unwrap();
        directory
    }

    #[test]
    fn bol_keyring_markers_decide_signed_in() {
        // Полная ключница: вход выполнен
        let profile = temp_profile("full");
        fs::write(
            profile.join("xodus-home").join(".xodus-keyring.ron"),
            "(device: \"dev_license\", user: \"user-tokens\", account: \"user-DA\")",
        )
        .unwrap();
        assert!(bol_profile_signed_in(&profile), "user-tokens + user-DA = вход выполнен");
        let _ = fs::remove_dir_all(&profile);

        // Только девайс-записи: xodus провизию устройства делает сам, это НЕ вход
        let profile = temp_profile("device-only");
        fs::write(
            profile.join("xodus-home").join(".xodus-keyring.ron"),
            "(device: \"dev_license\")",
        )
        .unwrap();
        assert!(!bol_profile_signed_in(&profile), "девайс-запись без пользовательских токенов — входа нет");

        // Нет файла вовсе
        let profile = temp_profile("missing");
        assert!(!bol_profile_signed_in(&profile));
        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn shared_xodus_keyring_promotes_sign_in_across_profiles() {
        let root = temp_profile("share");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let paths = ProfilePaths { root: root.clone(), profiles_root: root.join("profiles") };
        let prof_a = paths.profiles_root.join("prof-a");
        let prof_b = paths.profiles_root.join("prof-b");
        let prof_c = paths.profiles_root.join("prof-c");
        for dir in [&prof_a, &prof_b, &prof_c] {
            fs::create_dir_all(dir.join("xodus-home")).unwrap();
            fs::write(dir.join("profile.json"), "{}").unwrap();
        }
        // Единственный вход живёт в профиле A (экземпляр которого «сломан»);
        // в B — только девайс-записи; в C — пусто.
        fs::write(
            prof_a.join("xodus-home").join(".xodus-keyring.ron"),
            "(device: \"dev_license\", user: \"user-tokens\", account: \"user-DA\")",
        )
        .unwrap();
        fs::write(
            prof_b.join("xodus-home").join(".xodus-keyring.ron"),
            "(device: \"dev_license\")",
        )
        .unwrap();

        share_xodus_keyring(&paths);

        let shared_keyring = root.join("xodus-home").join(".xodus-keyring.ron");
        assert!(shared_keyring.is_file(), "вход поднят в общий дом");
        assert!(keyring_file_signed_in(&shared_keyring), "общий вход выполнен");
        for (label, dir) in [("A", &prof_a), ("B", &prof_b), ("C", &prof_c)] {
            assert!(dir.join("xodus-home").is_symlink(), "профиль {label} ссылается на общий дом");
            assert!(bol_profile_signed_in(dir), "вход виден из профиля {label}");
        }
        // Локальные копии не удалены, а retiring-переименованы
        assert!(prof_a.join("xodus-home.local").is_dir() || prof_b.join("xodus-home.local").is_dir());
        assert!(!prof_a.join("xodus-home.local").join(".xodus-keyring.ron").exists(), "ключница из A переехала, копии не осталось");

        // Повторный запуск — no-op: ссылки на месте, ничего не сломано
        share_xodus_keyring(&paths);
        assert!(shared_keyring.is_file());
        assert!(prof_b.join("xodus-home").is_symlink());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shared_xodus_keyring_keeps_existing_shared_sign_in() {
        let root = temp_profile("share-keep");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("xodus-home")).unwrap();
        fs::write(
            root.join("xodus-home").join(".xodus-keyring.ron"),
            "(shared: \"user-tokens\", account: \"user-DA\")",
        )
        .unwrap();
        let paths = ProfilePaths { root: root.clone(), profiles_root: root.join("profiles") };
        let prof = paths.profiles_root.join("prof-x");
        fs::create_dir_all(prof.join("xodus-home")).unwrap();
        fs::write(prof.join("profile.json"), "{}").unwrap();
        fs::write(
            prof.join("xodus-home").join(".xodus-keyring.ron"),
            "(local: \"user-tokens\", account: \"user-DA\")",
        )
        .unwrap();

        share_xodus_keyring(&paths);

        let blob = fs::read(root.join("xodus-home").join(".xodus-keyring.ron")).unwrap();
        assert!(blob.starts_with(b"(shared:"), "общий вход не затёрт локальным");
        assert!(prof.join("xodus-home.local").join(".xodus-keyring.ron").exists(), "локальная копия сохранена в retired");
        let _ = fs::remove_dir_all(&root);
    }

    fn archive_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            for (name, content) in entries {
                zip.start_file(name.to_owned(), zip::write::SimpleFileOptions::default()).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn version_parser_ignores_unrecognized_lines() {
        let lines: Vec<String> = [
            "notice: cached catalog",
            "Minecraft for Windows  (--mc release)",
            "    not-a-version",
            "    1.26.1 installed",
            "Minecraft Preview  (--mc preview)",
            "    1.27.0-beta+2",
        ]
        .into_iter()
        .map(|line| line.to_owned())
        .collect();
        assert_eq!(
            parse_game_versions(&lines),
            vec![
                json!({ "edition": "release", "version": "1.26.1", "installed": true }),
                json!({ "edition": "preview", "version": "1.27.0-beta+2", "installed": false }),
            ]
        );
    }

    #[test]
    fn game_version_regex_matches_versions_only() {
        assert!(game_version_re().is_match("1.26.0"));
        assert!(game_version_re().is_match("1.27.0-preview.1"));
        assert!(!game_version_re().is_match("release;echo bad"));
        assert!(!game_version_re().is_match(""));
        assert!(!game_version_re().is_match("latest"));
    }

    #[test]
    fn profile_names_are_stable_sha_prefixes() {
        assert_eq!(
            profile_name("bedrock-one"),
            format!("nivora-{}", &sha256_hex(b"bedrock-one")[..16])
        );
        assert!(profile_name_re().is_match(&profile_name("мир с символами!")));
    }

    #[test]
    fn log_ids_match_python_log_parser_contract() {
        // Python: [A-Za-z0-9._-]{1,240}\.log, хвост «-bedrock-…» после
        // <имя>-<sha24>-, и версия в хвосте [A-Za-z0-9._-]+.
        let log_id = new_log_id("bedrock-one");
        assert!(log_id.ends_with(".log"), "{log_id}");
        assert!(log_id.contains("-bedrock-"), "{log_id}");
        let ok_id = Regex::new(r"^[A-Za-z0-9._-]{1,240}\.log\z").unwrap().is_match(&log_id);
        assert!(ok_id, "{log_id}");
        let hash = sha256_hex(b"bedrock-one");
        assert!(log_id.starts_with(&format!("bedrock-one-{}-bedrock-", &hash[..24])), "{log_id}");
    }

    #[test]
    fn secret_lines_are_redacted() {
        let redacted = redact_line("info token: access_token=ABC.DEF.GHI ok");
        assert!(redacted.contains("access_token=[redacted]"), "{redacted}");
        assert!(!redacted.contains("ABC.DEF.GHI"));
    }

    #[test]
    fn bol_error_lines_are_detected_at_line_start() {
        assert!(bol_error_line_re().is_match("error  Minecraft is downloaded from Microsoft"));
        assert!(bol_error_line_re().is_match("FATAL: boot failed"));
        assert!(!bol_error_line_re().is_match("info   no error here"));
    }

    #[test]
    fn archive_validation_rejects_traversal_and_empty_archives() {
        let bad = archive_bytes(&[("../outside.txt", "bad")]);
        assert!(validate_content_archive(&bad).is_err());
        let empty: Vec<u8> = {
            let mut buffer = std::io::Cursor::new(Vec::new());
            let mut zip = zip::ZipWriter::new(&mut buffer);
            zip.finish().unwrap();
            buffer.into_inner()
        };
        assert!(validate_content_archive(&empty).is_err());
        let good = archive_bytes(&[("manifest.json", "{}"), ("pack/data.txt", "data")]);
        assert!(validate_content_archive(&good).is_ok());
    }

    #[test]
    fn windows_channels_map_to_official_products() {
        assert_eq!(WIN_CHANNEL_RELEASE.product_id, "9NBLGGH2JHXJ");
        assert_eq!(WIN_CHANNEL_RELEASE.package_family, "Microsoft.MinecraftUWP_8wekyb3d8bbwe");
        assert_eq!(WIN_CHANNEL_RELEASE.protocol, "minecraft://");
        assert_eq!(WIN_CHANNEL_PREVIEW.product_id, "9P5X4QVLC2XR");
        assert_eq!(
            WIN_CHANNEL_PREVIEW.package_family,
            "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe"
        );
        assert_eq!(win_channel("preview").product_id, WIN_CHANNEL_PREVIEW.product_id);
        assert_eq!(win_channel("release").product_id, WIN_CHANNEL_RELEASE.product_id);
        assert_eq!(win_channel("RELEASE").product_id, WIN_CHANNEL_RELEASE.product_id);
    }

    #[test]
    fn windows_content_root_uses_localappdata_packages() {
        if let Some(appdata) = win_local_appdata() {
            let root = win_content_root("release").unwrap();
            assert!(root.starts_with(appdata.join("Packages").join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")));
            assert!(root.ends_with("LocalState/games/com.mojang"));
        }
    }

    #[test]
    fn bundled_runtime_constants_are_consistent() {
        assert_eq!(BEDROCK_ON_LINUX_BUNDLED_VERSION, "2.2.9");
        assert_eq!(BEDROCK_ON_LINUX_BUNDLED_SHA256.len(), 64);
        assert!(version_tuple("2.2.9") >= MIN_BOL_VERSION);
        assert!(version_tuple("2.1.9") < MIN_BOL_VERSION);
    }

    #[test]
    fn version_probe_parses_bol_help_output() {
        let re = version_re();
        assert_eq!(
            re.captures("BedrockOnLinux 2.2.9 — Minecraft Bedrock on Linux")
                .and_then(|c| c.get(1).map(|m| m.as_str().to_owned())),
            Some("2.2.9".to_owned())
        );
        assert!(re.captures("some other tool 1.0").is_none());
    }

    #[test]
    fn unsupported_platform_reports_official_channels() {
        if bedrock_platform() == BedrockPlatform::Unsupported {
            let status = runtime_status(&json!({})).unwrap();
            let message = status["status"]["message"].as_str().unwrap().to_owned();
            assert!(message.contains("Windows"), "{message}");
            assert!(message.contains("Linux"), "{message}");
        }
    }

    #[test]
    fn job_lifecycle_starts_snapshots_and_cancels() {
        let name = format!("job-{}", random_hex(3));
        let started = spawn_job("setup", &name, |job, cancellation| {
            job_log(&job, "working");
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if cancellation.cancelled() {
                    job_fail(&job, "Операция остановлена пользователем", true);
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            job_finish(&job, json!({ "installed": true }));
        })
        .unwrap();
        assert_eq!(started["started"], json!(true));
        // повторный запуск того же задания не стартует вторую нить
        let again = spawn_job("setup", &name, |job, _| {
            job_finish(&job, json!({ "installed": false }));
        })
        .unwrap();
        assert_eq!(again["already_running"], json!(true));
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut snapshot = job_snapshot("setup", &name);
        while !snapshot["running"].as_bool().unwrap_or(false) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            snapshot = job_snapshot("setup", &name);
        }
        assert_eq!(snapshot["running"], json!(true));
        assert!(cancel_job("setup", &name));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut final_snapshot = job_snapshot("setup", &name);
        while final_snapshot["running"].as_bool().unwrap_or(false) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            final_snapshot = job_snapshot("setup", &name);
        }
        assert_eq!(final_snapshot["running"], json!(false));
        assert_eq!(final_snapshot["cancelled"], json!(true));
    }

    #[test]
    fn classify_archive_routes_packs_by_manifest_modules() {
        let manifest = serde_json::json!({
            "format_version": 2,
            "modules": [{ "type": "data", "uuid": "x" }]
        })
        .to_string();
        let bytes = archive_bytes(&[("manifest.json", &manifest), ("packs/behavior/pack.json", "{}")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
        let targets = classify_archive(&mut archive).unwrap();
        assert_eq!(targets, vec![("behavior_packs".to_owned(), String::new())]);

        let addon = archive_bytes(&[
            ("ResourcePack/manifest.json", r#"{"modules":[{"type":"resources"}]}"#),
            ("BehaviorPack/manifest.json", r#"{"modules":[{"type":"data"}]}"#),
        ]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(addon.as_slice())).unwrap();
        let targets = classify_archive(&mut archive).unwrap();
        let directories: Vec<&str> = targets.iter().map(|(directory, _)| directory.as_str()).collect();
        assert!(directories.contains(&"resource_packs"));
        assert!(directories.contains(&"behavior_packs"));

        let world = archive_bytes(&[("levelname.txt", "My World"), ("level.dat", "")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(world.as_slice())).unwrap();
        let targets = classify_archive(&mut archive).unwrap();
        assert_eq!(targets, vec![("minecraftWorlds".to_owned(), String::new())]);
    }

    #[test]
    fn import_extension_whitelist() {
        assert!(import_extension("Pack.MCPACK").is_ok());
        assert!(import_extension("world.mcworld").is_ok());
        assert!(import_extension("mod.jar").is_err());
    }

    #[test]
    fn windows_selection_roundtrip() {
        let root = std::env::temp_dir().join(format!("nivora-bedrock-meta-{}-{}", std::process::id(), random_hex(3)));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        persist_windows_selection(&root, "preview", "1.21.120").unwrap();
        assert_eq!(windows_selected(&root), ("preview".to_owned(), "1.21.120".to_owned()));
        persist_windows_selection(&root, "release", "latest").unwrap();
        assert_eq!(windows_selected(&root), ("release".to_owned(), "latest".to_owned()));
        // мусорные значения игнорируются
        persist_windows_selection(&root, "release", "bad version; rm -rf").unwrap();
        assert_eq!(windows_selected(&root), (String::new(), String::new()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn live_state_writes_both_contracts() {
        let root = std::env::temp_dir().join(format!("nivora-bedrock-live-{}-{}", std::process::id(), random_hex(3)));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_live_state(&root, true, 4242, None, 123.5, "some.log");
        for name in [".trial-launcher-live.json", ".nivora-launcher-live.json"] {
            let value = read_bounded_json(&root.join(name), 4096).expect(name);
            assert_eq!(value["running"], json!(true));
            assert_eq!(value["pid"], json!(4242));
            assert_eq!(value["log_id"], json!("some.log"));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scope_tail_shows_last_four_chars() {
        assert_eq!(scope_tail("d71d487f"), "…487f");
        assert_eq!(scope_tail("abc"), "abc");
        assert_eq!(scope_tail("LocalState-file"), "…file");
    }

    #[test]
    fn store_page_link_points_to_official_product() {
        assert!(STORE_PAGE_LINK_RU.contains(WIN_CHANNEL_RELEASE.product_id));
        assert!(BEDROCK_ON_LINUX_URL.contains("BedrockOnLinux"));
    }

    #[test]
    /// Живая проверка встроенной среды: настоящий pyz + python3 из репозитория.
    /// Игнорируется по умолчанию (CI не зависит от дерева репозитория):
    ///   cargo test --locked bol_probe_real_bundle -- --ignored --nocapture
    #[test]
    #[ignore = "нужен настоящий vendor/bedrock-on-linux-2.2.9.pyz из дерева репозитория"]
    fn bol_probe_real_bundle() {
        let candidates = bundled_pyz_candidates();
        let pyz = candidates
            .iter()
            .find(|path| path.is_file() && fs::metadata(path).map(|meta| meta.len() < BUNDLED_PYZ_MAX_BYTES).unwrap_or(false))
            .expect("pyz не найден: запустите из дерева репозитория");
        let python = python_for_zipapp().expect("python3 недоступен");
        let command = vec![
            python.to_string_lossy().into_owned(),
            pyz.to_string_lossy().into_owned(),
        ];
        let home = std::env::temp_dir().join(format!("nivora-bol-probe-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let probe = bol_probe(&command, &home);
        let _ = fs::remove_dir_all(&home);
        println!("probe: supported={} version={} message={}", probe.supported, probe.version, probe.message);
        assert!(probe.supported, "встроенная среда BoL не прошла проверку: {}", probe.message);
        assert_eq!(probe.version, BEDROCK_ON_LINUX_BUNDLED_VERSION);
    }

    /// Реальная проверка подмены notify-send: PATH из bol_environment указывает
    /// на заглушку внутри профиля, а не на системный notify-send.
    #[cfg(target_os = "linux")]
    #[test]
    fn bol_environment_prepends_notify_send_stub() {
        let home = std::env::temp_dir().join(format!("nivora-bol-env-{}-{}", std::process::id(), random_hex(3)));
        fs::create_dir_all(&home).unwrap();
        let environment = bol_environment(&home, None);
        let stub = environment
            .get("PATH")
            .and_then(|path| path.split(':').next().map(str::to_owned))
            .expect("PATH в окружении BoL");
        let resolved = std::env::split_paths(&environment.get("PATH").unwrap())
            .map(|dir| dir.join("notify-send"))
            .find(|path| path.is_file())
            .expect("заглушка notify-send не найдена в PATH");
        assert!(resolved.starts_with(&home), "{} вне профиля {}", resolved.display(), stub);
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v notify-send")
            .env("PATH", environment.get("PATH").unwrap())
            .output()
            .unwrap();
        assert!(output.status.success(), "sh не нашёл notify-send в PATH окружения BoL");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn launch_intercept_recognizes_real_bedrock_instance_directory() {
        // Полный путь пользователя: settings.json → game_dir → каталог
        // экземпляра → metadata → перехват. Регрессионный тест на случай
        // рассинхронизации классификации Rust/Python: запуск Bedrock,
        // провалившийся в Python-воркер, показывал заглушку вместо игры.
        let root = std::env::temp_dir().join(format!("nivora-intercept-{}", random_hex(6)));
        let config = root.join("config");
        let data = root.join("data");
        let game = root.join("game");
        let instance = game.join("instances").join("bedrock-main");
        fs::create_dir_all(config.join(LAUNCHER_DIR)).unwrap();
        fs::create_dir_all(data.join(LAUNCHER_DIR)).unwrap();
        fs::create_dir_all(&instance).unwrap();
        fs::write(
            config.join(LAUNCHER_DIR).join("settings.json"),
            format!("{{\"game_dir\": {}}}", serde_json::to_string(game.to_string_lossy().as_ref()).unwrap()),
        )
        .unwrap();
        fs::write(
            instance.join(INSTANCE_METADATA_FILE),
            "{\"kind\":\"client\",\"game_edition\":\"bedrock\",\"bedrock_profile\":\"nivora-test\"}",
        )
        .unwrap();

        let old_config = std::env::var("XDG_CONFIG_HOME").ok();
        let old_data = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_DATA_HOME", &data);
        let decision = should_intercept_launch(&json!({
            "operation": "launch_instance",
            "name": "bedrock-main",
        }));
        let display_decision = should_intercept_launch(&json!({
            "operation": "launch_instance",
            "name": " bedrock-main ",
        }));
        match old_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_data {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }

        assert!(decision, "bedrock-экземпляр обязан перехватываться Rust-движком");
        assert!(display_decision, "имя с пробелами Trim-ится как в реальном запросе");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn launch_interception_routes_only_bedrock_launches() {
        // не launch-операции никогда не перехватываются
        assert!(!should_intercept_launch(&json!({ "operation": "install", "name": "x" })));
        assert!(!should_intercept_launch(&json!({ "operation": "delete_instance", "name": "x" })));
        // launch неизвестного экземпляра уходит воркеру (перехвата нет)
        assert!(!should_intercept_launch(&json!({
            "operation": "launch",
            "instance_name": format!("no-such-bedrock-{}", random_hex(4)),
        })));
        // пустое имя — не перехватываем
        assert!(!should_intercept_launch(&json!({ "operation": "launch", "instance_name": "" })));
        // launch_intercepted требует имя
        assert!(launch_intercepted(&json!({ "operation": "launch" })).is_err());
    }

    fn request_field_helper_reads_requests() {
        let request = json!({ "name": "  bedrock-one  " });
        assert_eq!(request_field(&request, "name").unwrap(), "bedrock-one");
        assert!(request_field(&json!({}), "name").is_err());
        assert_eq!(request_field(&json!({}), "display_name").unwrap(), "");
    }

    // ── молчаливые скачивания: читатель строк и watchdog ──

    #[test]
    fn clean_process_line_strips_ansi_and_nul_noise() {
        // Прогресс-бары перерисовываются CSI-курсорами и цветами
        assert_eq!(
            clean_process_line(b"\x1b[2K\x1b[1ADownloading 42%"),
            Some("Downloading 42%".to_owned())
        );
        // NUL-добивка псевдотерминала и возвраты каретки как хвост
        assert_eq!(clean_process_line(b"\0\0ok  \0"), Some("ok".to_owned()));
        // OSC-последовательность (заголовок окна) вырезается целиком
        assert_eq!(clean_process_line(b"\x1b]0;title\x07line"), Some("line".to_owned()));
        // Только шум — не строка вовсе
        assert_eq!(clean_process_line(b"\x1b[2K\r\r"), None);
        assert_eq!(clean_process_line(b"   "), None);
    }

    #[test]
    fn line_reader_treats_carriage_return_as_terminator() {
        let payload: &[u8] = b"one\rpre-reload\rtwo\nthree\r\nfour";
        let (sender, receiver) = mpsc::channel::<Option<String>>();
        spawn_line_reader(Some(std::io::Cursor::new(payload.to_vec())), sender);
        let mut collected: Vec<String> = Vec::new();
        loop {
            match receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(Some(line)) => collected.push(line),
                Ok(None) => break,
                Err(_) => panic!("читатель не завершился"),
            }
        }
        // `\r`-перерисовки превращаются в строки, хвост без терминатора тоже доезжает
        assert_eq!(collected, vec!["one", "pre-reload", "two", "three", "four"]);
    }

    #[test]
    fn download_watchdog_ignores_silent_but_growing_downloads() {
        let home = std::env::temp_dir().join(format!("nivora-watchdog-{}", random_hex(6)));
        fs::create_dir_all(&home).unwrap();
        let package = home.join("package.part");
        fs::write(&package, vec![0u8; 1024]).unwrap();
        let mut watchdog = DownloadWatchdog::with_interval(&home, Duration::ZERO);

        // Первый обход — только снимок, вердикта ещё нет
        assert_eq!(watchdog.check(), WatchdogVerdict::Pending);

        // Дерево выросло — скачивание живо, хотя stdout молчит
        let grown = home.join("package.part");
        fs::write(&grown, vec![0u8; 4096]).unwrap();
        match watchdog.check() {
            WatchdogVerdict::Active(bytes) => assert_eq!(bytes, 4096),
            other => panic!("рост не замечен: {other:?}"),
        }

        // Ничего не менялось — это настоящее зависание
        assert_eq!(watchdog.check(), WatchdogVerdict::Silent);

        // Новый файл тоже признак работы
        fs::write(home.join("second.part"), vec![0u8; 10]).unwrap();
        assert!(matches!(watchdog.check(), WatchdogVerdict::Active(_)));

        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn run_job_process_kills_stalled_installer() {
        // Установщик, замолчавший навсегда: stall-детекция обязана остановить его
        // быстрее жёсткого потолка и вернуть внятную ошибку.
        let mut command = Command::new("sh");
        command.args(["-c", "echo start; sleep 60"]).stdout(Stdio::piped()).stderr(Stdio::piped());
        let started = Instant::now();
        let mut silent = |_line: &str| {};
        let result = run_job_process(
            &mut command,
            None,
            &mut silent,
            Duration::from_secs(60),
            Duration::from_millis(400),
        );
        let error = result.expect_err("зависший установщик должен быть остановлен");
        assert!(error.contains("перестал отвечать"), "неожиданная ошибка: {error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn run_job_process_reports_zero_exit_for_chatty_process() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo line-one; echo line-two"]).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut seen: Vec<String> = Vec::new();
        let mut logger = |line: &str| seen.push(line.to_owned());
        let code = run_job_process(
            &mut command,
            None,
            &mut logger,
            Duration::from_secs(10),
            Duration::from_secs(10),
        )
        .expect("живой процесс завершается сам");
        assert_eq!(code, 0);
        assert_eq!(seen, vec!["line-one", "line-two"]);
    }
}
