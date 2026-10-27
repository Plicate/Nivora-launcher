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

// дочерние модули обращаются к платформе и помощникам через `super::platform::*`
#[allow(unused_imports)]
pub(super) use super::platform;
mod content;
mod profiles;
mod runtime;
#[cfg(test)]
use content::{classify_archive, import_extension, scope_tail};
pub use profiles::{
    account_status, create_instance, store_login, store_login_cancel, store_login_status,
};
pub(crate) use profiles::{delete_profile, profile_path_for, snapshot_root_for};
use profiles::{
    bol_profile_signed_in, engine_ready, linux_profile_path, profile_paths, selected_version,
    store_status, BEDROCK_LOGIN_REQUIRED_MESSAGE,
};
#[cfg(test)]
use profiles::{
    keyring_file_signed_in, profile_name, profile_name_re, share_xodus_keyring, ProfilePaths,
};
use runtime::{
    bol_require_runtime, bol_run, bol_runtime, bol_select_supported, glibc_version,
    python_for_zipapp, spawn_line_reader, stop_registered_process, DownloadWatchdog,
};
#[cfg(test)]
use runtime::{
    bol_environment, bol_probe, bundled_pyz_candidates, clean_process_line, WatchdogVerdict,
};

const BEDROCK_ON_LINUX_URL: &str = "https://github.com/Wyze3306/BedrockOnLinux";
const BEDROCK_ON_LINUX_BUNDLED_VERSION: &str = "2.2.9";
const BEDROCK_ON_LINUX_BUNDLED_SHA256: &str =
    "bb894a86ed19575635d63f11cef466b70da2b99a12a3bda8468386630c40ed5c";
const BUNDLED_PYZ_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// минимальная поддержка версии среды (2.2.0)
const MIN_BOL_VERSION: (u32, u32, u32) = (2, 2, 0);
/// glibc, ниже которого xodus-вход в Microsoft Store не работает
const MIN_GLIBC_VERSION: (u32, u32, u32) = (2, 39, 0);

pub(crate) const MAX_CONTENT_UPLOAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_CONTENT_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_CONTENT_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_JOB_LOG_LINES: usize = 200;
const MAX_SESSION_LOG_LINES: usize = 500;
const MAX_LOG_ARCHIVE_BYTES: u64 = 5 * 1024 * 1024;

const CONTENT_DIRECTORIES: &[(&str, &[&str])] = &[
    ("addons", &["behavior_packs"]),
    ("resourcepacks", &["resource_packs"]),
    ("worlds", &["minecraftWorlds", "world_templates"]),
    ("skins", &["skin_packs", "custom_skins"]),
];
const IMPORT_EXTENSIONS: &[&str] = &[".mcpack", ".mcaddon", ".mcworld", ".mctemplate", ".mcskin"];

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
/// bedrockOnLinux пишет уровень в начало строки ("error …"); среда может
/// завершиться с кодом 0 даже после собственных "error"
fn bol_error_line_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:error|critical|fatal)\b").unwrap());
    &RE
}
fn glibc_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d+\.\d+)\b").unwrap());
    &RE
}

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

fn valid_game_version(version: &str) -> bool {
    game_version_re().is_match(version)
        || (bedrock_platform() == BedrockPlatform::WindowsOfficial && version == "latest")
}

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
    // фолбэк: pid + время через SHA-256 (на Windows urandom недоступен напрямую)
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

/// флаг CREATE_NO_WINDOW: вспомогательные утилиты Windows не должны мелькать консолью
fn prepare_command(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// запускает процесс с общим таймаутом и возвращает код с выводом. `None` означает таймаут
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

/// каталог контента официальной игры: …\Packages\<family>\LocalState\games\com.mojang
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

struct WinPackageCache {
    at: Instant,
    release: WinPackageState,
    preview: WinPackageState,
}

static WIN_PACKAGE_CACHE: LazyLock<Mutex<Option<WinPackageCache>>> =
    LazyLock::new(|| Mutex::new(None));

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
    let mut command = super::platform::command(powershell);
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

/// открыть официальную страницу продукта в Microsoft Store
fn open_store_product(edition: &str) -> bool {
    let channel = win_channel(edition);
    let uri = format!("ms-windowsstore://pdp/?ProductId={}", channel.product_id);
    let mut command = super::platform::command("cmd.exe");
    command.args(["/C", "start", "", &uri]);
    prepare_command(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// запуск официальной игры по протоколу (minecraft:// / minecraft-preview://)
fn launch_official(edition: &str) -> Result<(), String> {
    let channel = win_channel(edition);
    let mut command = super::platform::command("cmd.exe");
    command.args(["/C", "start", "", channel.protocol]);
    let started = prepare_command(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if started.map(|status| status.success()).unwrap_or(false) {
        return Ok(());
    }
    // используем AUMID через `AppsFolder` как запасной способ
    let mut fallback = super::platform::command("explorer.exe");
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

/// идентификатор PID игры по образу процесса (tasklist есть на любой Windows)
fn windows_game_pids() -> Vec<u32> {
    let mut command = super::platform::command("tasklist.exe");
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
    super::platform::command(taskkill)
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn read_instance_metadata(path: &Path) -> Value {
    // старые профили могут содержать файл .trial-launcher-instance.json
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

/// выбранный канал официальной версии хранится в метаданных экземпляра
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
        let name = entry.name().map(|v| v.replace('\\', "/")).unwrap_or_default();
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
    // индекс растёт на диске без вывода; watchdog отслеживает его загрузку
    let mut watchdog = DownloadWatchdog::new(&profile);
    let (code, lines) =
        bol_run(&argv, &profile, "versions", None, None, Duration::from_secs(300), 10_000, false, None, None, Some(&mut watchdog))
            .map_err(|error| format!("Не удалось загрузить список версий Minecraft: {error}"))?;
    if code != 0 {
        let detail = lines.last().cloned().unwrap_or_else(|| format!("код {code}"));
        return Err(format!("Не удалось загрузить список версий Minecraft: {detail}"));
    }
    let mut parsed = parse_game_versions(&lines);
    // отмечаем выбранную версию установленной после успешной настройки
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
            // microsoft Store устанавливает только последнюю версию
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
    // загрузка может идти без вывода; watchdog проверяет её по изменению файлов
    let mut watchdog = DownloadWatchdog::new(&profile);
    let (code, logs) =
        bol_run(&argv, &profile, "setup", cancellation, Some(&mut logger), Duration::from_secs(600), 500, false, None, None, Some(&mut watchdog))?;
    let (selected_edition, selected_version) = selected_version(&profile);
    // bol может вернуть 0 при ошибке; проверяем вывод и settings.json
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
        // автоматическая попытка winget срабатывает не для всех учётных
        if let Some(winget) = winget_executable() {
            job_log(job, &format!("Пробую установку через winget ({}):", channel.product_id));
            let mut command = super::platform::command(&winget);
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
                // долгая загрузка допустима; таймаут тишины обнаруживает зависание
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
        // если winget не сработал, предлагаем страницу продукта в Microsoft Store
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

pub fn content_list(request: &Value) -> Result<Value, String> {
    content::content_list(request)
}

pub fn screenshots_list(request: &Value) -> Result<Value, String> {
    content::screenshots_list(request)
}

pub fn content_import(request: &Value) -> Result<Value, String> {
    content::content_import(request)
}

pub fn content_remove(request: &Value) -> Result<Value, String> {
    content::content_remove(request)
}

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
    // лицензия xodus обязательна на каждом запуске (игра зашифрована под
    if !bol_profile_signed_in(&profile) {
        return Err(BEDROCK_LOGIN_REQUIRED_MESSAGE.to_owned());
    }
    // удерживаем блокировку до завершения игровой сессии
    let mut session_lock = InstanceProcessLock::new(&ctx.data_home, &info.name);
    session_lock.acquire()?;
    // если установке версии не хватило сети/времени на движок, "play" умирал
    // если возникает `GDK-Proton missing`, устанавливаем компонент сами
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
        // процессов (Wine + игра) от этого корня
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
    // boL умеет падать с кодом 0/1 без старта игры, когда привязка аккаунта
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
    // ждём появление процесса игры, затем сопровождаем сессию до выхода
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
        // если процесс не найден, не ждём его бесконечно
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
        // полная ключница: вход выполнен
        let profile = temp_profile("full");
        fs::write(
            profile.join("xodus-home").join(".xodus-keyring.ron"),
            "(device: \"dev_license\", user: \"user-tokens\", account: \"user-DA\")",
        )
        .unwrap();
        assert!(bol_profile_signed_in(&profile), "user-tokens + user-DA = вход выполнен");
        let _ = fs::remove_dir_all(&profile);

        // только девайс-записи: xodus провизию устройства делает сам, это НЕ вход
        let profile = temp_profile("device-only");
        fs::write(
            profile.join("xodus-home").join(".xodus-keyring.ron"),
            "(device: \"dev_license\")",
        )
        .unwrap();
        assert!(!bol_profile_signed_in(&profile), "девайс-запись без пользовательских токенов — входа нет");

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
        // локальный каталог сохранён под именем xodus-home.local
        assert!(prof_a.join("xodus-home.local").is_dir() || prof_b.join("xodus-home.local").is_dir());
        assert!(!prof_a.join("xodus-home.local").join(".xodus-keyring.ron").exists(), "ключница из A переехала, копии не осталось");

        // повторная консолидация не должна нарушать ссылки
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
        // идентификатор журнала содержит имя, хеш и суффикс Bedrock
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
            let zip = zip::ZipWriter::new(&mut buffer);
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
        // полный путь пользователя: settings.json → game_dir → каталог
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
        // операции, кроме запуска, не перехватываются
        assert!(!should_intercept_launch(&json!({ "operation": "install", "name": "x" })));
        assert!(!should_intercept_launch(&json!({ "operation": "delete_instance", "name": "x" })));
        // неизвестный экземпляр обрабатывается основным маршрутом
        assert!(!should_intercept_launch(&json!({
            "operation": "launch",
            "instance_name": format!("no-such-bedrock-{}", random_hex(4)),
        })));
        // пустое имя не перехватывается
        assert!(!should_intercept_launch(&json!({ "operation": "launch", "instance_name": "" })));
        assert!(launch_intercepted(&json!({ "operation": "launch" })).is_err());
    }

    #[test]
    fn request_field_helper_reads_requests() {
        let request = json!({ "name": "  bedrock-one  " });
        assert_eq!(request_field(&request, "name").unwrap(), "bedrock-one");
        assert!(request_field(&json!({}), "name").is_err());
        assert_eq!(request_field(&json!({}), "display_name").unwrap(), "");
    }

    #[test]
    fn clean_process_line_strips_ansi_and_nul_noise() {
        assert_eq!(
            clean_process_line(b"\x1b[2K\x1b[1ADownloading 42%"),
            Some("Downloading 42%".to_owned())
        );
        assert_eq!(clean_process_line(b"\0\0ok  \0"), Some("ok".to_owned()));
        assert_eq!(clean_process_line(b"\x1b]0;title\x07line"), Some("line".to_owned()));
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
        assert_eq!(collected, vec!["one", "pre-reload", "two", "three", "four"]);
    }

    #[test]
    fn download_watchdog_ignores_silent_but_growing_downloads() {
        let home = std::env::temp_dir().join(format!("nivora-watchdog-{}", random_hex(6)));
        fs::create_dir_all(&home).unwrap();
        let package = home.join("package.part");
        fs::write(&package, vec![0u8; 1024]).unwrap();
        let mut watchdog = DownloadWatchdog::with_interval(&home, Duration::ZERO);

        assert_eq!(watchdog.check(), WatchdogVerdict::Pending);

        let grown = home.join("package.part");
        fs::write(&grown, vec![0u8; 4096]).unwrap();
        match watchdog.check() {
            WatchdogVerdict::Active(bytes) => assert_eq!(bytes, 4096),
            other => panic!("рост не замечен: {other:?}"),
        }

        assert_eq!(watchdog.check(), WatchdogVerdict::Silent);

        fs::write(home.join("second.part"), vec![0u8; 10]).unwrap();
        assert!(matches!(watchdog.check(), WatchdogVerdict::Active(_)));

        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn run_job_process_kills_stalled_installer() {
        let mut command = std::process::Command::new("sh");
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
        let mut command = std::process::Command::new("sh");
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
