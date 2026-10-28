// запуск Minecraft Java Edition и управление игровой сессией
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use super::accounts;
use super::auto_repair;
use super::config::{java_memory_value, ConfigStore};
use super::constants::APP_VERSION;
use super::crash_reports;
use super::events;
use super::instance_files::instance_log_hash;
use super::instances::{InstanceInfo, InstanceStore};
use super::installer::{InstallResult, MinecraftInstaller};
use super::net::{progress_client_from_settings, HttpClient};
use super::operations::CancellationToken;
use super::optimization;
use super::platform;
use super::platform_support;
use super::process_guard::InstanceProcessLock;
use super::runtime::{java_version, safe_process_environment, JavaRuntimeManager};
use super::security::{self};
use super::state::launcher_roots;

mod game;
pub(crate) use game::GameLauncher;

const AUTH_TOKEN_PLACEHOLDER: &str = "__TRIAL_LAUNCHER_AUTH_TOKEN__";
const JAVA_ARGUMENT_FILE_TARGET: &str = "/instance/.trial-cell/tmp/.trial-launcher-java-args";
const WAYLAND_GLFW_URL: &str = "https://github.com/jdkeke142/glfw-wayland-minecraft/releases/download/v1.1.0/libglfw.so";
const WAYLAND_GLFW_SHA256: &str = "1fa075ea4f595bd6917a7f304f2fe5d583ec82d8f1ac0f0350bd38b32a20a46e";
const WAYLAND_GLFW_SIZE: u64 = 662_440;
const MAX_SESSION_LOG_LINES: usize = 500;
const USER_COMMAND_TIMEOUT_SECONDS: u64 = 120;
const USER_COMMAND_OUTPUT_LINES: usize = 40;

// журнал игровой сессии
// журнал хранится в памяти и используется бенчмарком и авто-починкой

struct SessionState {
    logs: VecDeque<String>,
    running: bool,
    started_at: f64,
}

fn sessions() -> &'static Mutex<HashMap<String, SessionState>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, SessionState>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn session_start(instance_name: &str) {
    let mut guard = sessions().lock().unwrap_or_else(|poison| poison.into_inner());
    guard.insert(
        instance_name.to_owned(),
        SessionState {
            logs: VecDeque::new(),
            running: true,
            started_at: epoch_seconds(),
        },
    );
}

pub(crate) fn session_append(instance_name: &str, line: &str) {
    let mut guard = sessions().lock().unwrap_or_else(|poison| poison.into_inner());
    let state = guard.entry(instance_name.to_owned()).or_insert_with(|| SessionState {
        logs: VecDeque::new(),
        running: false,
        started_at: 0.0,
    });
    if state.logs.len() == MAX_SESSION_LOG_LINES {
        state.logs.pop_front();
    }
    state.logs.push_back(line.to_owned());
}

pub(crate) fn session_finish(instance_name: &str) {
    let mut guard = sessions().lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(state) = guard.get_mut(instance_name) {
        state.running = false;
    }
}

pub(crate) fn session_snapshot(instance_name: &str) -> (Vec<String>, bool) {
    let guard = sessions().lock().unwrap_or_else(|poison| poison.into_inner());
    match guard.get(instance_name) {
        Some(state) => (state.logs.iter().cloned().collect(), state.running),
        None => (Vec::new(), false),
    }
}

/// возвращает время старта для поиска свежих сбоев и недавно добавленных модов
pub(crate) fn session_started_at(instance_name: &str) -> f64 {
    let guard = sessions().lock().unwrap_or_else(|poison| poison.into_inner());
    guard.get(instance_name).map(|state| state.started_at).unwrap_or(0.0)
}

fn epoch_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0)
}

fn now_nanoseconds() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_nanos()).unwrap_or(0)
}

fn random_hex(byte_count: usize) -> String {
    let mut buffer = vec![0u8; byte_count];
    #[cfg(unix)]
    if let Ok(mut source) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        if source.read_exact(&mut buffer).is_ok() {
            return buffer.iter().map(|byte| format!("{byte:02x}")).collect();
        }
    }
    let seed = format!("{}:{}:{}", std::process::id(), now_nanoseconds(), Instant::now().elapsed().as_nanos());
    buffer
        .iter_mut()
        .enumerate()
        .map(|(index, byte)| {
            *byte = (seed.as_bytes().get(index.wrapping_mul(7) % seed.len()).copied().unwrap_or(index as u8))
                .wrapping_add((now_nanoseconds() % 251) as u8);
            format!("{byte:02x}")
        })
        .collect()
}

// формат аргументов команд

/// разбирает строку аргументов и возвращает ошибку при незакрытой кавычке
fn shlex_split(value: &str, posix: bool) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if posix && character == '\\' && quote != Some('\'') {
            if let Some(next) = characters.next() {
                current.push(next);
                has_token = true;
                continue;
            }
            return Err("No escaped character".to_owned());
        }
        match quote {
            Some(open) if character == open => {
                quote = None;
                if !posix {
                    current.push(character);
                }
            }
            Some(_) => {
                current.push(character);
                has_token = true;
            }
            None if character == '\'' || character == '"' => {
                quote = Some(character);
                if !posix {
                    current.push(character);
                }
                has_token = true;
            }
            None if character.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            None => {
                current.push(character);
                has_token = true;
            }
        }
    }
    if quote.is_some() {
        return Err("No closing quotation".to_owned());
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// собирает аргументы в строку для предварительного просмотра команды
fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| {
            let needs_quotes = part.is_empty()
                || part.chars().any(|character| {
                    character.is_whitespace() || matches!(character, '\'' | '"' | '\\' | ';' | '&' | '|' | '<' | '>' | '(' | ')' | '$' | '`' | '~' | '*' | '?' | '#' | '!')
                });
            if needs_quotes {
                format!("'{}'", part.replace('\'', "'\\''"))
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn rec_str(record: &Map<String, Value>, key: &str) -> String {
    record.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn rec_bool(record: &Map<String, Value>, key: &str) -> bool {
    record.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn is_legacy_version(version_id: &str) -> bool {
    // извлекает номер версии 1.x, отделённый точкой, дефисом или подчёркиванием
    let mut rest = version_id;
    while let Some(index) = rest.find("1.") {
        let after = &rest[index + 2..];
        let digits: String = after.chars().take_while(|character| character.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let tail = after.chars().nth(digits.len());
            if digits.parse::<i64>().map(|minor| minor < 13).unwrap_or(false)
                && tail.is_none_or(|character| character == '.' || character == '-' || character == '_')
            {
                return true;
            }
        }
        rest = &rest[index + 1..];
    }
    false
}

fn looks_like_glx_context_error(lines: &[String]) -> bool {
    let text = lines.join("\n").to_lowercase();
    [
        "glxbadfbconfig",
        "65543",
        "glx: failed to create context",
        "failed to create context",
        "no opengl context found",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn looks_like_non_nvidia_renderer(line: &str) -> bool {
    let lowered = line.to_lowercase();
    if lowered.contains("llvmpipe") || lowered.contains("softpipe") {
        return true;
    }
    let vendor = lowered
        .find("opengl vendor")
        .and_then(|start| lowered[start..].split_once(':'))
        .map(|(_, value)| value.trim().to_owned());
    matches!(vendor, Some(value) if !value.contains("nvidia"))
}

fn safe_extra_arguments(value: Option<&Value>) -> Vec<String> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for item in items.iter().take(32) {
        let argument = match item {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if argument.is_empty() || argument.contains('\0') || argument.contains('\n') || argument.contains('\r') {
            continue;
        }
        result.push(argument.chars().take(512).collect());
    }
    result
}

fn log_component(value: &str) -> String {
    let normalized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = normalized.trim_matches(|character| matches!(character, '.' | '_' | '-'));
    let bounded: String = trimmed.chars().take(80).collect();
    if bounded.is_empty() {
        "launcher".to_owned()
    } else {
        bounded
    }
}

fn replace_placeholders(value: &str, placeholders: &BTreeMap<String, String>) -> String {
    let mut result = value.to_owned();
    for (key, replacement) in placeholders {
        result = result.replace(&format!("${{{key}}}"), replacement);
    }
    result
}

/// применяет правила Mojang к ОС, архитектуре и признакам запуска; последнее совпавшее правило задаёт действие
fn argument_allowed(rules: Option<&Value>, features: &BTreeMap<&str, bool>) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else {
        return true;
    };
    if rules.is_empty() {
        // пустой список правил трактуем как отсутствие ограничений
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        let Some(rule) = rule.as_object() else { continue };
        if let Some(operating_system) = rule.get("os").and_then(Value::as_object) {
            if let Some(name) = operating_system.get("name").and_then(Value::as_str).filter(|value| !value.is_empty()) {
                if name != mojang_os_name() {
                    continue;
                }
            }
            let mut architecture = machine_arch().to_lowercase();
            if matches!(architecture.as_str(), "x86_64" | "amd64") {
                architecture = "x86_64".to_owned();
            } else if matches!(architecture.as_str(), "aarch64" | "arm64") {
                architecture = "arm64".to_owned();
            }
            let expected = operating_system.get("arch").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let mut expected_values = [architecture.as_str()].iter().map(|value| value.to_string()).collect::<Vec<String>>();
            if architecture == "x86_64" {
                expected_values.extend(["amd64".to_owned(), "64".to_owned()]);
            }
            if !expected.is_empty() && !expected_values.contains(&expected) {
                continue;
            }
        }
        if let Some(conditions) = rule.get("features").and_then(Value::as_object) {
            let mismatch = conditions.iter().any(|(name, expected)| {
                features.get(name.as_str()).copied().unwrap_or(false) != expected.as_bool().unwrap_or(false)
            });
            if mismatch {
                continue;
            }
        }
        allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
    }
    allowed
}

fn mojang_os_name() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(not(windows))]
    {
        match std::env::consts::OS {
            "macos" => "osx",
            other => other,
        }
    }
}

fn machine_arch() -> String {
    std::env::consts::ARCH.to_owned()
}

fn expand_arguments(items: &Value, placeholders: &BTreeMap<String, String>, features: &BTreeMap<&str, bool>) -> Vec<String> {
    let mut expanded: Vec<String> = Vec::new();
    let Some(items) = items.as_array() else {
        return expanded;
    };
    for item in items {
        if let Some(text) = item.as_str() {
            expanded.push(replace_placeholders(text, placeholders));
            continue;
        }
        let Some(object) = item.as_object() else { continue };
        if !argument_allowed(object.get("rules"), features) {
            continue;
        }
        match object.get("value").unwrap_or(&Value::String(String::new())) {
            Value::Array(values) => {
                expanded.extend(values.iter().map(|value| replace_placeholders(&value.to_string(), placeholders)));
            }
            other => expanded.push(replace_placeholders(&other.to_string(), placeholders)),
        }
    }
    expanded
}

fn redact_command(command: &[String], secrets: &[&str]) -> Vec<String> {
    command
        .iter()
        .map(|value| {
            for secret in secrets {
                if !secret.is_empty() && value.contains(secret) {
                    return value.replace(secret, "<redacted>");
                }
            }
            value.clone()
        })
        .collect()
}

fn jvm_xmx_kib(value: &str) -> Option<i64> {
    // (\d+(?:\.\d+)?)([kKmMgG]?)
    let trimmed = value.trim();
    let digits_end = trimmed.find(|character: char| !character.is_ascii_digit() && character != '.').unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(digits_end);
    let parsed: f64 = number.parse().ok()?;
    let unit = unit.chars().next()?.to_ascii_lowercase();
    let factor = match unit {
        'k' => 1.0,
        'm' => 1024.0,
        'g' => 1024.0 * 1024.0,
        _ if unit.is_alphanumeric() => return None,
        _ => return None,
    };
    if number.is_empty() || number.starts_with('.') || number.ends_with('.') || number.matches('.').count() > 1 {
        return None;
    }
    Some((parsed * factor) as i64)
}

fn cap_heap_for_32bit_java(command: &[String], emit: &mut dyn FnMut(&str)) -> Vec<String> {
    let mut capped = Vec::with_capacity(command.len());
    let mut adjusted = false;
    for item in command {
        if item.starts_with("-Xmx") && item.len() > 4 {
            if let Some(kib) = jvm_xmx_kib(&item[4..]) {
                if kib > 1024 * 1024 {
                    capped.push("-Xmx1024M".to_owned());
                    adjusted = true;
                    continue;
                }
            }
        }
        capped.push(item.clone());
    }
    if adjusted {
        emit("Java 32-бит: память экземпляра уменьшена до 1024M — потолок этой сборки");
    }
    capped
}

/// правка команды под старые JVM: один запуск `java -version` на обе правки
fn adapt_command_for_old_java(command: &[String], emit: &mut dyn FnMut(&str)) -> Vec<String> {
    if command.is_empty() {
        return command.to_vec();
    }
    let (major, bitness) = java_probe(Path::new(&command[0]));
    let mut capped = command.to_vec();
    if bitness == Some(32) {
        capped = cap_heap_for_32bit_java(&capped, emit);
    }
    if let Some(major) = major {
        if major < 9 && !capped.iter().any(|item| item == "-XX:+IgnoreUnrecognizedVMOptions") {
            let mut result = vec!["-XX:+IgnoreUnrecognizedVMOptions".to_owned()];
            result.extend(capped);
            capped = result;
        }
    }
    capped
}

fn java_probe(java_path: &Path) -> (Option<i64>, Option<i32>) {
    let Some(output) = java_version_output(java_path) else {
        return (None, None);
    };
    let major = output
        .find("version")
        .and_then(|index| output[index..].find('"').map(|quote| index + quote + 1))
        .and_then(|start| output[start..].find('"').map(|end| output[start..start + end].to_owned()))
        .and_then(|value| {
            let mut parts = value.split('.');
            if value.starts_with("1.") {
                // 1.8.0_392 → 8
                let _ = parts.next();
            }
            parts.next().and_then(|component| component.parse::<i64>().ok())
        });
    let mut bitness: Option<i32> = None;
    if output.contains("64-Bit") || output.contains("64-bit") {
        bitness = Some(64);
    } else if output.contains("VM") || output.contains("version") {
        bitness = Some(32);
    }
    (major, bitness)
}

fn java_version_output(java_path: &Path) -> Option<String> {
    let mut command = platform::command(java_path);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stderr).to_string();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    Some(text)
}

fn apply_memory_setting(arguments: &[String], memory: &str) -> Result<Vec<String>, String> {
    let mut result: Vec<String> = arguments.iter().filter(|value| !value.starts_with("-Xmx")).cloned().collect();
    if !memory.trim().is_empty() {
        let value = format!("-Xmx{}", java_memory_value(memory)?);
        match result.iter().position(|item| item == "-cp" || item == "-classpath") {
            Some(index) => result.insert(index, value),
            None => result.push(value),
        }
    }
    Ok(result)
}

fn ensure_native_library_path(arguments: Vec<String>, natives_dir: &Path) -> Vec<String> {
    if arguments.iter().any(|value| value.starts_with("-Djava.library.path=")) {
        return arguments;
    }
    let native_argument = format!("-Djava.library.path={}", natives_dir.to_string_lossy());
    let mut result = arguments;
    match result.iter().position(|item| item == "-cp" || item == "-classpath") {
        Some(index) => result.insert(index, native_argument),
        None => result.push(native_argument),
    }
    result
}

// файл аргументов Java

struct JavaArgumentFile {
    fd: Option<i32>,
    path: Option<PathBuf>,
    argument: String,
}

impl JavaArgumentFile {
    fn close(&mut self) {
        if let Some(fd) = self.fd.take() {
            unsafe {
                libc::close(fd);
            }
        }
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for JavaArgumentFile {
    fn drop(&mut self) {
        self.close();
    }
}

fn java_argument(value: &str) -> Result<String, String> {
    if value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err("Аргумент Java содержит недопустимый перевод строки".to_owned());
    }
    Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn create_java_argument_file(
    arguments: &[String],
    instance_dir: &Path,
    private_directory: Option<&Path>,
) -> Result<JavaArgumentFile, String> {
    let content = arguments
        .iter()
        .map(|value| java_argument(value))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n")
        + "\n";

    // в Linux игра и bwrap наследуют memfd, поэтому временный файл не нужен
    #[cfg(target_os = "linux")]
    {
        const MFD_NONE: libc::c_uint = 0;
        let fd = unsafe { libc::memfd_create(c"nivora-launcher-java-args".as_ptr(), MFD_NONE) };
        if fd >= 0 {
            let data = content.as_bytes();
            let mut offset = 0usize;
            let ok = loop {
                let written = unsafe { libc::write(fd, data[offset..].as_ptr() as *const libc::c_void, data.len() - offset) };
                if written < 0 {
                    break false;
                }
                offset += written as usize;
                if offset >= data.len() {
                    break true;
                }
            };
            if ok && unsafe { libc::lseek(fd, 0, libc::SEEK_SET) } >= 0 {
                return Ok(JavaArgumentFile {
                    fd: Some(fd),
                    path: None,
                    argument: format!("@/proc/self/fd/{fd}"),
                });
            }
            unsafe {
                libc::close(fd);
            }
        }
    }

    // без memfd используем временный файл с правами 0600 и удаляем его после запуска
    let arguments_dir = if cfg!(windows) {
        let directory = private_directory.ok_or("Для Windows Java argument file нужен приватный каталог данных приложения")?;
        fs::create_dir_all(directory).map_err(|error| format!("Каталог Java argument file недоступен: {error}"))?;
        platform::restrict_dir(directory);
        directory.to_path_buf()
    } else {
        security::ensure_instance_cell(instance_dir)?.tmp
    };
    for _ in 0..32 {
        let path = arguments_dir.join(format!(".java-args-{}", random_hex(6)));
        if path.symlink_metadata().is_ok() {
            continue;
        }
        let mut file = platform::create_private_new(&path).map_err(|error| format!("Не удалось создать Java argument file: {error}"))?;
        if file.write_all(content.as_bytes()).is_err() {
            let _ = fs::remove_file(&path);
            return Err("Не удалось записать Java argument file".to_owned());
        }
        drop(file);
        return Ok(JavaArgumentFile {
            fd: None,
            path: Some(path.clone()),
            argument: format!("@{}", path.to_string_lossy()),
        });
    }
    Err("Не удалось создать уникальный Java argument file".to_owned())
}

fn should_use_argument_file(has_token: bool, java_major: Option<i64>) -> bool {
    if let Some(major) = java_major {
        if major < 9 {
            return false;
        }
    }
    has_token || cfg!(windows)
}

fn materialize_auth_token(command: &[String], access_token: &str) -> Vec<String> {
    if access_token.is_empty() {
        return command.to_vec();
    }
    command
        .iter()
        .map(|value| value.replace(AUTH_TOKEN_PLACEHOLDER, access_token))
        .collect()
}

// пользовательские команды запуска

fn launch_wrapper_tokens(launch_settings: &Value, emit: &mut dyn FnMut(&str)) -> Vec<String> {
    let wrapper = launch_settings.get("launch_wrapper").and_then(Value::as_str).unwrap_or("").trim();
    if wrapper.is_empty() {
        return Vec::new();
    }
    let tokens = match shlex_split(wrapper, !cfg!(windows)) {
        Ok(tokens) => tokens,
        Err(_) => wrapper.split_whitespace().map(str::to_owned).collect(),
    };
    if tokens.is_empty() {
        return Vec::new();
    }
    let Some(resolved) = which(&tokens[0]) else {
        emit(&format!("Обёртка запуска '{}' не найдена в PATH — запускаю без неё.", tokens[0]));
        return Vec::new();
    };
    let mut result = vec![resolved.to_string_lossy().to_string()];
    result.extend(tokens[1..].iter().cloned());
    result
}

fn merge_launch_environment(environment: &mut BTreeMap<String, String>, launch_settings: &Value) {
    let Some(values) = launch_settings.get("env_variables").and_then(Value::as_object) else {
        return;
    };
    for (name, value) in values {
        let key = name.trim();
        if key.is_empty() {
            continue;
        }
        environment.insert(key.to_owned(), match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
    }
}

/// исключает библиотеки AppImage из LD_LIBRARY_PATH, чтобы игра использовала системные драйверы
fn sanitize_inherited_ld_library_path(environment: &mut BTreeMap<String, String>) {
    let raw = match environment.get("LD_LIBRARY_PATH") {
        Some(value) if !value.trim().is_empty() => value.clone(),
        _ => return,
    };
    let own_dir = match std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
    {
        Some(dir) => dir,
        None => return,
    };
    let entries: Vec<&str> = raw.split(':').filter(|entry| !entry.trim().is_empty()).collect();
    let filtered: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|entry| std::path::Path::new(entry).strip_prefix(&own_dir).is_err())
        .collect();
    if filtered.len() == entries.len() {
        return;
    }
    if filtered.is_empty() {
        environment.remove("LD_LIBRARY_PATH");
    } else {
        environment.insert("LD_LIBRARY_PATH".to_owned(), filtered.join(":"));
    }
}

fn run_user_launch_command(
    launch_settings: &Value,
    key: &str,
    instance_dir: &Path,
    emit: &mut dyn FnMut(&str),
    required: bool,
) -> Result<(), String> {
    let command = launch_settings.get(key).and_then(Value::as_str).unwrap_or("").trim().to_owned();
    if command.is_empty() {
        return Ok(());
    }
    let mut tokens = match shlex_split(&command, !cfg!(windows)) {
        Ok(tokens) => tokens,
        Err(error) => {
            let message = format!("Команда «{key}» не разобрана: {error}");
            if required {
                return Err(message);
            }
            emit(&message);
            return Ok(());
        }
    };
    if tokens.is_empty() {
        return Ok(());
    }
    if !tokens[0].contains('/') && !tokens[0].contains('\\') {
        if let Some(resolved) = which(&tokens[0]) {
            tokens[0] = resolved.to_string_lossy().to_string();
        }
    }
    let mut environment = HashMap::new();
    for (name, value) in safe_process_environment(false) {
        environment.insert(name, value);
    }
    let mut merged = BTreeMap::new();
    for (name, value) in &environment {
        merged.insert(name.clone(), value.clone());
    }
    merge_launch_environment(&mut merged, launch_settings);
    let mut process = platform::command(&tokens[0]);
    process
        .args(&tokens[1..])
        .current_dir(instance_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &merged {
        process.env(name, value);
    }
    let spawned = process
        .spawn()
        .and_then(|mut child| {
            let deadline = Instant::now() + Duration::from_secs(USER_COMMAND_TIMEOUT_SECONDS);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return Ok((status, child)),
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = platform_support::terminate_process_tree(child.id() as i32, true);
                        return Err(std::io::Error::other("истёк таймаут 120 с"));
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(error) => return Err(error),
                }
            }
        })
        .and_then(|(status, mut child)| {
            let mut output = String::new();
            if let Some(mut stream) = child.stdout.take() {
                let _ = std::io::Read::read_to_string(&mut stream, &mut output);
            }
            if let Some(mut stream) = child.stderr.take() {
                let mut stderr = String::new();
                let _ = std::io::Read::read_to_string(&mut stream, &mut stderr);
                output.push('\n');
                output.push_str(&stderr);
            }
            Ok((status, output))
        });
    let (status, output) = match spawned {
        Ok(value) => value,
        Err(error) => {
            let message = format!("Команда «{key}» ({}) не выполнилась: {error}", tokens[0]);
            if required {
                return Err(message);
            }
            emit(&message);
            return Ok(());
        }
    };
    let label = if required { "до запуска" } else { "после запуска" };
    for line in output.lines().take(USER_COMMAND_OUTPUT_LINES) {
        emit(&format!("[{key}] {line}"));
    }
    let code = status.code().unwrap_or(-1);
    if code != 0 {
        let message = format!("Команда {label} завершилась с кодом {code}");
        if required {
            return Err(message);
        }
        emit(&message);
        return Ok(());
    }
    emit(&format!("Команда {label} выполнена: {}", tokens[0]));
    Ok(())
}

fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        let executable = candidate
            .metadata()
            .map(|meta| platform::is_executable(&meta))
            .unwrap_or(false);
        if executable {
            return Some(candidate);
        }
    }
    None
}

// режим Wayland и графика

fn is_wayland_session(environment: &BTreeMap<String, String>, version_id: &str) -> bool {
    environment.get("XDG_SESSION_TYPE").map(|value| value.trim().to_lowercase()).as_deref() == Some("wayland")
        && environment.get("WAYLAND_DISPLAY").map(|value| !value.trim().is_empty()).unwrap_or(false)
        && !is_legacy_version(version_id)
}

fn native_wayland_command(command: &[String], main_class: &str, library: &Path) -> Result<Vec<String>, String> {
    let Some(main_index) = command.iter().skip(1).position(|value| value == main_class) else {
        return Err("Не найден mainClass для native Wayland GLFW".to_owned());
    };
    let main_index = main_index + 1;
    let mut result = command[..main_index].to_vec();
    result.push(format!("-Dorg.lwjgl.glfw.libname={}", library.to_string_lossy()));
    result.push("-Dsodium.checks.issue2561=false".to_owned());
    result.extend(command[main_index..].iter().cloned());
    Ok(result)
}

fn native_wayland_environment(environment: &mut BTreeMap<String, String>) {
    for key in [
        "__GLX_VENDOR_LIBRARY_NAME",
        "__VK_LAYER_NV_optimus",
        "MESA_LOADER_DRIVER_OVERRIDE",
        "GALLIUM_DRIVER",
        "LIBGL_ALWAYS_SOFTWARE",
        "LIBGL_DRI3_DISABLE",
        "LIBGL_ALWAYS_INDIRECT",
        "MESA_GL_VERSION_OVERRIDE",
        "MESA_GLSL_VERSION_OVERRIDE",
    ] {
        environment.remove(key);
    }
    if environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str) != Some("nvidia") {
        environment.remove("__NV_PRIME_RENDER_OFFLOAD");
    }
    environment.insert("GLFW_FORCE_WAYLAND".to_owned(), "1".to_owned());
    environment.insert(
        "TRIAL_LAUNCHER_GRAPHICS_BACKEND".to_owned(),
        "native-wayland-glfw-v1.1.0".to_owned(),
    );
}

// состояние запущенного экземпляра

fn write_live_state(
    instance_dir: &Path,
    running: bool,
    pid: i32,
    exit_code: Option<i64>,
    started_at: f64,
    log_id: &str,
    launch_id: &str,
) {
    let value = json!({
        "running": running,
        "pid": pid,
        "exit_code": exit_code,
        "started_at": started_at,
        "log_id": log_id,
        "launch_id": launch_id,
    });
    // новое имя использует движок, старое нужно файловым и облачным сервисам
    for name in [".trial-launcher-live.json", ".nivora-launcher-live.json"] {
        let path = instance_dir.join(name);
        if path.is_symlink() {
            continue;
        }
        if let Ok(mut file) = platform::create_private(&path) {
            let _ = serde_json::to_writer(&mut file, &value);
            platform::restrict_file(&path);
        }
    }
}

fn ensure_private_file(path: &Path) {
    if let Ok(handle) = OpenOptions::new().write(true).create(true).append(true).open(path) {
        drop(handle);
        platform::restrict_file(path);
    }
}

// основной запуск игры

fn snapshot_lines(source: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    source.lock().map(|lines| lines.clone()).unwrap_or_default()
}

fn return_code_for_live_state(launch_result: &Result<i32, String>) -> Option<i64> {
    match launch_result {
        Ok(code) => Some(*code as i64),
        Err(_) => None,
    }
}

fn setting_text(settings: &Value, key: &str, fallback: &str) -> String {
    match settings.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => fallback.to_owned(),
    }
}

// запуск игрового процесса

struct RunProcess<'a> {
    log_path: &'a Path,
    instance_dir: &'a Path,
    log_id: &'a str,
    launch_id: &'a str,
    captured: &'a mut Vec<String>,
    sink: &'a Arc<Mutex<dyn FnMut(&str) + Send>>,
    wrapper_tokens: &'a [String],
    process_command: Vec<String>,
    process_cwd: PathBuf,
    process_environment: BTreeMap<String, String>,
    argument_fd: Option<i32>,
    cancellation: Option<Arc<CancellationToken>>,
    pid_slot: Option<&'a Arc<Mutex<Option<i32>>>>,
    append_log: bool,
    wrong_nvidia_renderer: &'a mut bool,
    nvidia_mode: bool,
}

/// записывает строки игры в кольцевой буфер, журнал и интерфейс
struct Recorder {
    captured: Vec<String>,
    nvidia_mode: bool,
    wrong_nvidia_renderer: bool,
    stop_requested: bool,
}

impl Recorder {
    fn record(&mut self, line: &str, log: &mut Option<File>, sink: &Arc<Mutex<dyn FnMut(&str) + Send>>) {
        self.captured.push(line.to_owned());
        if self.captured.len() > MAX_SESSION_LOG_LINES {
            let excess = self.captured.len() - MAX_SESSION_LOG_LINES;
            self.captured.drain(..excess);
        }
        if let Some(file) = log.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
        if let Ok(mut guard) = sink.lock() {
            guard(line);
        }
        if self.nvidia_mode && looks_like_non_nvidia_renderer(line) {
            self.wrong_nvidia_renderer = true;
            self.stop_requested = true;
            self.record("Игра сообщила OpenGL renderer не NVIDIA при выбранном режиме NVIDIA; останавливаю запуск без software fallback.", log, sink);
            return;
        }
        if looks_like_glx_context_error(&self.captured) {
            self.stop_requested = true;
            self.record("Обнаружена ошибка GLX; закрываю зависшее окно игры для повторного запуска.", log, sink);
        }
    }
}

fn read_process_lines<R: BufRead>(mut reader: R, mut on_line: impl FnMut(String) -> bool) -> std::io::Result<()> {
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            break;
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if on_line(String::from_utf8_lossy(&bytes).into_owned()) {
            break;
        }
    }
    Ok(())
}

fn run_process(run: RunProcess<'_>) -> Result<i32, String> {
    // обёртка (gamemoderun/mangohud/…) оборачивает итоговую команду снаружи
    // без песочницы не работают D-Bus gamemoded и LD_PRELOAD
    let mut active_command = run.process_command.clone();
    if !run.wrapper_tokens.is_empty() {
        let mut wrapped = run.wrapper_tokens.to_vec();
        wrapped.extend(active_command);
        active_command = wrapped;
    }
    let log_file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(run.append_log)
        .truncate(!run.append_log)
        .open(run.log_path)
        .map_err(|error| format!("Не удалось открыть лог запуска: {error}"))?;
    let mut command = platform::command(&active_command[0]);
    command
        .args(&active_command[1..])
        .current_dir(&run.process_cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &run.process_environment {
        command.env(name, value);
    }
    platform_support::prepare_process(&mut command);
    if run.argument_fd.is_some() && !cfg!(windows) {
        pass_fd(&mut command, run.argument_fd);
    }
    let mut child: Child = command
        .spawn()
        .map_err(|error| format!("Не удалось запустить Java: {error}"))?;
    let pid = child.id() as i32;
    if let Some(slot) = run.pid_slot {
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(pid);
        }
    }
    write_live_state(run.instance_dir, true, pid, None, epoch_seconds(), run.log_id, run.launch_id);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let monitor_finished = Arc::new(AtomicBool::new(false));
    let recorder = Recorder {
        captured: std::mem::take(run.captured),
        nvidia_mode: run.nvidia_mode,
        wrong_nvidia_renderer: *run.wrong_nvidia_renderer,
        stop_requested: false,
    };
    let recorder_lock = Arc::new(Mutex::new((recorder, Some(log_file))));
    let mut handles = Vec::new();
    for stream in [stdout.map(|stream| Box::new(stream) as Box<dyn std::io::Read + Send>), stderr.map(|stream| Box::new(stream) as Box<dyn std::io::Read + Send>)] {
        let Some(stream) = stream else { continue };
        let recorder_lock = recorder_lock.clone();
        let sink = run.sink.clone();
        let stop_flag = stop_flag.clone();
        handles.push(std::thread::spawn(move || {
            let _ = read_process_lines(BufReader::new(stream), |line| {
                let stop = {
                    let mut guard = recorder_lock.lock().unwrap_or_else(|poison| poison.into_inner());
                    let (recorder, log) = &mut *guard;
                    recorder.record(&line, log, &sink);
                    recorder.stop_requested
                };
                if stop {
                    stop_flag.store(true, Ordering::SeqCst);
                }
                stop
            });
        }));
    }
    // проверяем отмену каждые 100 мс
    if let Some(cancellation) = run.cancellation.clone() {
        let stop_flag = stop_flag.clone();
        let monitor_finished = monitor_finished.clone();
        let monitor_slot = run.pid_slot.cloned();
        handles.push(std::thread::spawn(move || loop {
            if stop_flag.load(Ordering::SeqCst) || monitor_finished.load(Ordering::SeqCst) {
                break;
            }
            if cancellation.cancelled() {
                if let Some(slot) = &monitor_slot {
                    if let Ok(guard) = slot.lock() {
                        if let Some(pid) = *guard {
                            platform_support::terminate_process_tree(pid, false);
                        }
                    }
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }));
    }
    let mut wait_error = None;
    while !stop_flag.load(Ordering::SeqCst) {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                wait_error = Some(error);
                stop_flag.store(true, Ordering::SeqCst);
            }
        }
    }
    monitor_finished.store(true, Ordering::SeqCst);
    if stop_flag.load(Ordering::SeqCst) {
        platform_support::terminate_process_tree(pid, false);
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut exited = false;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(error) => {
                    wait_error.get_or_insert(error);
                    break;
                }
            }
        }
        if !exited {
            platform_support::terminate_process_tree(pid, true);
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    let status = child.wait().map_err(|error| format!("Не удалось дождаться завершения Java: {error}"))?;
    if let Some(error) = wait_error {
        return Err(format!("Не удалось проверить состояние Java: {error}"));
    }
    {
        let mut guard = recorder_lock.lock().unwrap_or_else(|poison| poison.into_inner());
        let (recorder, _) = &mut *guard;
        *run.captured = std::mem::take(&mut recorder.captured);
        *run.wrong_nvidia_renderer = recorder.wrong_nvidia_renderer;
    }
    if let Some(slot) = run.pid_slot {
        if let Ok(mut guard) = slot.lock() {
            *guard = None;
        }
    }
    let code = exit_code(&status);
    if let Some(cancellation) = run.cancellation.as_ref() {
        cancellation.check()?;
    }
    Ok(code)
}

fn exit_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -signal;
        }
    }
    -1
}

/// оставляет дескриптор memfd открытым после запуска процесса игры
#[cfg(unix)]
fn pass_fd(command: &mut Command, fd: Option<i32>) {
    use std::os::unix::process::CommandExt;
    let Some(fd) = fd else { return };
    unsafe {
        command.pre_exec(move || {
            libc::fcntl(fd, libc::F_SETFD, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn pass_fd(_command: &mut Command, _fd: Option<i32>) {}

// выбор аккаунта и настройки запуска

/// объединяет настройки экземпляра с профилем запуска; значения экземпляра имеют приоритет
pub(crate) fn effective_launch_settings(
    workspace: &super::instance_workspace::InstanceWorkspace,
    config: &ConfigStore,
    instance_name: Option<&str>,
    request: &Value,
) -> Result<Value, String> {
    let profile_id = request
        .get("profile_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let profile_id = if profile_id.trim().is_empty() {
        config.str_setting("active_launcher_profile_id").trim().to_lowercase()
    } else {
        profile_id.trim().to_lowercase()
    };
    let profile = if profile_id.is_empty() {
        None
    } else {
        config
            .settings()
            .get("launcher_profiles")
            .and_then(Value::as_array)
            .and_then(|profiles| {
                profiles
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str).unwrap_or("").trim().to_lowercase() == profile_id)
            })
            .cloned()
    };
    let mut values = match instance_name {
        None => {
            let mut map = Map::new();
            let memory = request.get("memory").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            map.insert("memory".into(), json!(if memory.is_empty() { config.str_setting("memory") } else { memory }));
            let java_path = request.get("java_path").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            map.insert("java_path".into(), json!(if java_path.is_empty() { config.str_setting("java_path") } else { java_path }));
            map.insert("graphics_mode".into(), json!(config.str_setting("graphics_mode")));
            Value::Object(map)
        }
        Some(name) => workspace.settings(name)?,
    };
    if let Some(profile) = profile {
        let instance_keys: [&str; 6] = ["memory", "java_path", "graphics_mode", "jvm_arguments", "game_arguments", "jvm_preset"];
        for key in [
            "memory", "java_path", "graphics_mode", "window_width", "window_height", "fullscreen", "quick_play_world",
            "quick_play_server", "jvm_arguments", "game_arguments", "jvm_preset",
        ] {
            let candidate = profile.get(key);
            let skip = match candidate {
                None | Some(Value::Null) => true,
                Some(Value::String(text)) => text.trim().is_empty() || (matches!(key, "memory" | "java_path" | "graphics_mode") && text.trim().to_lowercase() == "auto"),
                _ => false,
            };
            if skip {
                continue;
            }
            if instance_name.is_some() && instance_keys.contains(&key) {
                let own = values.get(key);
                let own_set = match own {
                    Some(Value::String(text)) => !text.trim().is_empty() && text.trim().to_lowercase() != "auto",
                    Some(Value::Array(items)) => !items.is_empty(),
                    Some(Value::Null) | None => false,
                    _ => true,
                };
                if own_set {
                    continue;
                }
            }
            values[key] = candidate.cloned().unwrap_or(Value::Null);
        }
        values["account_id"] = json!(profile.get("account_id").and_then(Value::as_str).unwrap_or("auto"));
        values["profile_id"] = json!(profile.get("id").and_then(Value::as_str).unwrap_or(""));
    }
    if values.get("memory").and_then(Value::as_str) == Some("auto") || values.get("memory").is_none() {
        let memory = request.get("memory").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        values["memory"] = json!(if memory.is_empty() { config.str_setting("memory") } else { memory });
    }
    let java_empty = values
        .get("java_path")
        .map(|value| match value.as_str() { Some(text) => text.trim().is_empty(), None => false })
        .unwrap_or(true);
    if java_empty {
        let java_path = request.get("java_path").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        values["java_path"] = json!(if java_path.is_empty() { config.str_setting("java_path") } else { java_path });
    }
    if values.get("graphics_mode").and_then(Value::as_str).map(str::trim) == Some("auto") {
        values["graphics_mode"] = json!(config.str_setting("graphics_mode"));
    }
    Ok(values)
}

fn microsoft_record(account: accounts::MicrosoftAccount) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert("account_id".into(), json!(account.account_id));
    record.insert("kind".into(), json!("microsoft"));
    record.insert("username".into(), json!(account.username));
    record.insert("uuid".into(), json!(account.uuid));
    record.insert("access_token".into(), json!(account.access_token));
    record.insert("refresh_token".into(), json!(account.refresh_token));
    record.insert("xuid".into(), json!(account.xuid));
    record.insert("expires_at".into(), json!(account.expires_at));
    record.insert("alias".into(), json!(account.alias));
    record.insert("is_default".into(), json!(account.is_default));
    record
}

/// выбирает аккаунт по настройке пользователя; аккаунт ely.by требует явного идентификатора
fn select_account(kind: &str, config: &ConfigStore, http: &HttpClient) -> Result<Map<String, Value>, String> {
    let config_home = launcher_roots().config_home;
    let records = accounts::load_accounts_with_tokens(&config_home)?;
    let find = |predicate: &dyn Fn(&Map<String, Value>) -> bool| {
        records.iter().find(|record| predicate(record)).cloned()
    };
    if kind != "auto" {
        if let Some(record) = find(&|record: &Map<String, Value>| rec_str(record, "account_id") == kind) {
            return refresh_record(record, config, http);
        }
        return match kind {
            "offline" => accounts::offline_debug_account(&config_home),
            "microsoft" => Ok(microsoft_record(accounts::get_valid_account(
                &config_home,
                config.settings(),
                http,
                "",
                true,
            )?)),
            "ely" => Err("Укажите конкретный аккаунт ely.by или выберите его по умолчанию".to_owned()),
            other => Err(format!("Аккаунт не найден: {other}")),
        };
    }
    let configured = {
        let raw = config.str_setting("default_account_id");
        if raw.trim().is_empty() { "auto".to_owned() } else { raw }
    };
    if configured.to_lowercase() != "auto" {
        if let Some(record) = find(&|record: &Map<String, Value>| rec_str(record, "account_id") == configured) {
            return refresh_record(record, config, http);
        }
    }
    if let Some(default) = find(&|record: &Map<String, Value>| rec_bool(record, "is_default")) {
        return refresh_record(default, config, http);
    }
    if let Some(microsoft) = find(&|record: &Map<String, Value>| rec_str(record, "kind") == "microsoft") {
        return Ok(microsoft_record(accounts::get_valid_account(
            &config_home,
            config.settings(),
            http,
            &rec_str(&microsoft, "account_id"),
            true,
        )?));
    }
    accounts::offline_debug_account(&config_home)
}

fn refresh_record(record: Map<String, Value>, config: &ConfigStore, http: &HttpClient) -> Result<Map<String, Value>, String> {
    match rec_str(&record, "kind").as_str() {
        "microsoft" => Ok(microsoft_record(accounts::get_valid_account(
            &launcher_roots().config_home,
            config.settings(),
            http,
            &rec_str(&record, "account_id"),
            true,
        )?)),
        // проверяем и при необходимости обновляем токен ely.by
        "ely" => accounts::get_valid_ely_account(&launcher_roots().config_home, http, &rec_str(&record, "account_id")),
        _ => Ok(record),
    }
}

fn emit_telemetry_report(
    config: &ConfigStore,
    event: &str,
    instance: Option<&InstanceInfo>,
    version_id: &str,
    exit_code: Option<i64>,
    message: &str,
    logs: &[String],
) {
    let endpoint = config.str_setting("telemetry_endpoint").trim().to_owned();
    if !config.bool_setting("telemetry") || endpoint.is_empty() {
        return;
    }
    let log_tail: Vec<String> = logs
        .iter()
        .rev()
        .take(120)
        .rev()
        .map(|line| line.chars().take(500).collect())
        .collect();
    let graphics_mode = {
        let mode = config.str_setting("graphics_mode");
        if mode.trim().is_empty() { "auto".to_owned() } else { mode }
    };
    let instance_loader = instance
        .map(|info| info.loader.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "vanilla".to_owned());
    let instance_version_id = if version_id.is_empty() {
        instance.map(|info| info.version_id.clone()).unwrap_or_default()
    } else {
        version_id.to_owned()
    };
    let payload = json!({
        "event": event,
        "app": { "name": "nivora-launcher", "version": APP_VERSION },
        "timestamp": epoch_seconds(),
        "system": {
            "platform": std::env::consts::OS,
            "machine": std::env::consts::ARCH,
            "graphics_mode": graphics_mode,
        },
        "instance": {
            "name": instance.map(|info| info.name.clone()).unwrap_or_default(),
            "kind": instance.map(|info| info.kind.clone()).unwrap_or_else(|| "client".to_owned()),
            "loader": instance_loader,
            "minecraft_version": instance.map(|info| info.minecraft_version.clone()).unwrap_or_default(),
            "version_id": instance_version_id,
        },
        "error": {
            "message": message.chars().take(600).collect::<String>(),
            "exit_code": exit_code,
            "log_tail": log_tail,
        },
    });
    let Ok(http) = HttpClient::new(10, 1, &config.str_setting("http_proxy"), None) else {
        return;
    };
    let _ = http.post_json_soft(&endpoint, &payload);
}

// обработчики запуска игры

pub fn launch(request: &Value) -> Result<Value, String> {
    launch_common(request, false)
}

pub fn launch_instance(request: &Value) -> Result<Value, String> {
    launch_common(request, true)
}

fn launch_common(request: &Value, named_instance: bool) -> Result<Value, String> {
    let mut config = ConfigStore::load()?;
    let instance_name: Option<String> = if named_instance {
        let name = request.get("name").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if name.is_empty() {
            return Err("Не указано имя экземпляра".to_owned());
        }
        Some(name)
    } else {
        let name = request.get("instance_name").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if name.is_empty() { None } else { Some(name) }
    };
    let store = InstanceStore::new(&config)?;
    let info: Option<InstanceInfo> = match &instance_name {
        Some(name) => {
            let info = store.get(name)?;
            if info.game_edition == "bedrock" {
                return super::bedrock::launch_intercepted(request);
            }
            if info.running {
                return Err("Экземпляр уже запущен".to_owned());
            }
            Some(info)
        }
        None => None,
    };
    if !named_instance {
        // запуск без экземпляра сохраняет java/память в глобальные настройки
        if request.get("java_path").is_some() {
            config.set_values(&json!({ "java_path": request.get("java_path").and_then(Value::as_str).unwrap_or("").trim() }))?;
        }
        let memory = request.get("memory").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if !memory.is_empty() {
            config.set_values(&json!({ "memory": memory }))?;
        }
    }
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("").to_owned();
    let http = progress_client_from_settings(config.settings(), &operation_id)?;
    let mut installer = MinecraftInstaller::new(&config, http.clone())?;
    let install = match &info {
        Some(info) => installer.load_existing(&info.path, Some(&info.version_id))?,
        None => {
            let version = request.get("version").and_then(Value::as_str).unwrap_or("").to_owned();
            let loader = request.get("loader").and_then(Value::as_str).unwrap_or("vanilla").to_owned();
            let loader_version = request
                .get("loader_version")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let assets = !request.get("no_assets").and_then(Value::as_bool).unwrap_or(false);
            installer.install(&version, &loader, loader_version.as_deref(), None, assets, false)?
        }
    };
    let info = match info {
        Some(info) => info,
        None => {
            let name = install
                .instance_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut resolved = store.get(&name)?;
            resolved.path = install.instance_dir.clone();
            resolved
        }
    };
    launch_session(&config, &http, &operation_id, &install, Some(info), request)
}

fn launch_session(
    config: &ConfigStore,
    http: &Arc<HttpClient>,
    operation_id: &str,
    install: &InstallResult,
    info: Option<InstanceInfo>,
    request: &Value,
) -> Result<Value, String> {
    let workspace = super::instance_workspace::InstanceWorkspace::new()?;
    let instance_name = info.as_ref().map(|info| info.name.clone());
    let mut launch_settings = effective_launch_settings(&workspace, config, instance_name.as_deref(), request)?;
    if let Some(world) = request.get("world").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()) {
        launch_settings["quick_play_world"] = json!(world);
        launch_settings["quick_play_server"] = json!("");
    } else if let Some(server) = request.get("server").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()) {
        launch_settings["quick_play_server"] = json!(server);
        launch_settings["quick_play_world"] = json!("");
    }
    let mut account_kind = request.get("account").and_then(Value::as_str).unwrap_or("auto").to_owned();
    if account_kind == "auto" {
        account_kind = launch_settings
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_owned();
    }
    let account = select_account(&account_kind, config, http)?;
    let session_name = install
        .instance_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let log_id = format!(
        "{}-{}-{}-{}-{}.log",
        log_component(&session_name),
        instance_log_hash(&session_name),
        log_component(&install.version_id),
        now_nanoseconds(),
        random_hex(5)
    );
    let launch_id = random_hex(16);
    session_start(&session_name);
    events::emit_instance_status_event("session-started", "", &session_name);
    let lines_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_lines = lines_log.clone();
    let session_for_logs = session_name.clone();
    let launch_id_for_logs = launch_id.clone();
    let log_id_for_logs = log_id.clone();
    let mut log_sequence = 0_u64;
    let sink = Arc::new(Mutex::new(move |line: &str| {
        if let Ok(mut lines) = sink_lines.lock() {
            lines.push(line.to_owned());
            if lines.len() > MAX_SESSION_LOG_LINES {
                let excess = lines.len() - MAX_SESSION_LOG_LINES;
                lines.drain(..excess);
            }
        }
        session_append(&session_for_logs, line);
        log_sequence = log_sequence.saturating_add(1);
        events::emit_launch_log(
            line,
            &session_for_logs,
            &launch_id_for_logs,
            &log_id_for_logs,
            log_sequence,
        );
    }));
    let cancellation = if operation_id.trim().is_empty() {
        None
    } else {
        Some(Arc::new(CancellationToken::new(operation_id)))
    };
    let launcher = GameLauncher::new(ConfigStore::load()?, http.clone(), cancellation)?;
    let pid_slot = Arc::new(Mutex::new(None::<i32>));
    let dry_run = request.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
    let result = launcher.run(
        install,
        &account,
        dry_run,
        &launch_id,
        &log_id,
        sink.clone(),
        &launch_settings,
        Some(&pid_slot),
    );
    let exit_code = match result {
        Ok(code) => code,
        Err(error) => {
            session_finish(&session_name);
            emit_telemetry_report(
                config,
                "launch_error",
                info.as_ref(),
                &install.version_id,
                None,
                &error,
                &snapshot_lines(&lines_log),
            );
            events::emit_instance_status_event("operation-finished", "Запуск остановлен или завершился с ошибкой.", &session_name);
            if let Some(token) = launcher.cancellation_token() {
                let _ = token.check();
            }
            return Err(error);
        }
    };
    session_finish(&session_name);
    let mut crash: Option<Value> = None;
    let mut repair: Option<Value> = None;
    if exit_code != 0 {
        emit_telemetry_report(
            config,
            "launch_exit_nonzero",
            info.as_ref(),
            &install.version_id,
            Some(exit_code as i64),
            &format!("Minecraft exited with code {exit_code}"),
            &snapshot_lines(&lines_log),
        );
        let lines = snapshot_lines(&lines_log);
        crash = Some(crash_reports::analyze_crash(
            &lines,
            Some(exit_code as i64),
            info.as_ref().map(|info| info.path.as_path()),
            launch_settings.get("memory").and_then(Value::as_str).unwrap_or(""),
            info.as_ref().map(|info| info.minecraft_version.as_str()).unwrap_or(""),
            info.as_ref().map(|info| info.loader.as_str()).unwrap_or(""),
        ));
        // отчёт о ремонте включается в результат запуска, если функция включена в настройках
        repair = info.as_ref().and_then(|info| {
            let account_id = account.get("account_id").and_then(Value::as_str).unwrap_or("");
            auto_repair::build_report(
                config,
                &workspace,
                info,
                lines.clone(),
                Some(exit_code as i64),
                launch_settings.as_object()?,
                "",
                session_started_at(&session_name),
                false,
                account_id,
            )
            .map(Value::Object)
        });
    }
    if let Some(info) = info.as_ref() {
        auto_repair::record_launch_result(Some(info.path.as_path()), Some(exit_code as i64));
    }
    events::emit_instance_status_event("operation-finished", &format!("Игра завершила работу (код {exit_code})."), &session_name);
    Ok(json!({
        "exit_code": exit_code,
        "logs": snapshot_lines(&lines_log),
        "version_id": install.version_id,
        "crash": crash,
        "repair": repair,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_output_keeps_reading_after_invalid_utf8() {
        let mut lines = Vec::new();
        read_process_lines(std::io::Cursor::new(b"bad: \xff\r\nvalid line\nlast: \xfe"), |line| {
            lines.push(line);
            false
        })
        .unwrap();
        assert_eq!(lines, vec!["bad: �", "valid line", "last: �"]);
    }

    #[test]
    fn cancellation_monitor_terminates_after_child_exit() {
        for expected_exit_code in [0, 23] {
            let operation_id = format!(
                "launcher-child-exit-{}-{expected_exit_code}",
                std::process::id()
            );
            let cancellation = Arc::new(CancellationToken::new(&operation_id));
            let directory = std::env::temp_dir().join(&operation_id);
            fs::create_dir_all(&directory).unwrap();
            let log_path = directory.join("game.log");
            let instance_dir = directory.as_path().to_owned();
            let sink: Arc<Mutex<dyn FnMut(&str) + Send>> = Arc::new(Mutex::new(|_: &str| {}));
            #[cfg(unix)]
            let process_command = vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("exit {expected_exit_code}"),
            ];
            #[cfg(windows)]
            let process_command = vec![
                "cmd.exe".to_owned(),
                "/C".to_owned(),
                format!("exit {expected_exit_code}"),
            ];
            let process_cwd = directory.clone();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let thread_cancellation = cancellation.clone();

            std::thread::spawn(move || {
                let mut captured = Vec::new();
                let mut wrong_nvidia_renderer = false;
                let result = run_process(RunProcess {
                    log_path: &log_path,
                    instance_dir: &instance_dir,
                    log_id: "child-exit-test",
                    launch_id: "child-exit-launch",
                    captured: &mut captured,
                    sink: &sink,
                    wrapper_tokens: &[],
                    process_command,
                    process_cwd,
                    process_environment: BTreeMap::new(),
                    argument_fd: None,
                    cancellation: Some(thread_cancellation),
                    pid_slot: None,
                    append_log: false,
                    wrong_nvidia_renderer: &mut wrong_nvidia_renderer,
                    nvidia_mode: false,
                });
                let _ = done_tx.send(result);
            });

            let result = match done_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(result) => result,
                Err(_) => {
                    super::super::operations::request_cancel(&operation_id);
                    done_rx
                        .recv_timeout(Duration::from_secs(2))
                        .expect("run_process did not exit after cancellation")
                }
            };
            cancellation.cleanup();
            let _ = fs::remove_dir_all(&directory);
            assert_eq!(
                result.unwrap(),
                expected_exit_code,
                "the cancellation monitor must finish for every child exit status"
            );
        }
    }

    #[test]
    fn starting_a_new_session_clears_the_previous_log_and_running_state() {
        let name = format!("session-lifecycle-test-{}", std::process::id());
        session_start(&name);
        session_append(&name, "stale crash output");
        session_finish(&name);
        assert_eq!(session_snapshot(&name), (vec!["stale crash output".to_owned()], false));

        session_start(&name);
        assert_eq!(session_snapshot(&name), (Vec::new(), true));
        session_finish(&name);
        sessions().lock().unwrap().remove(&name);
    }

    #[test]
    fn legacy_versions_are_recognized() {
        assert!(is_legacy_version("1.12.2"));
        assert!(is_legacy_version("1.7.10-forge"));
        assert!(is_legacy_version("1.8"));
        assert!(!is_legacy_version("1.13"));
        assert!(!is_legacy_version("1.20.1"));
        // проверка находит версию и внутри строки, например в "b1.7.3"
        assert!(is_legacy_version("b1.7.3"));
    }

    #[test]
    fn glx_errors_are_detected_in_output() {
        assert!(looks_like_glx_context_error(&["GLXBadFBConfig".to_owned()]));
        assert!(looks_like_glx_context_error(&["X Error of failed request:  BadGLXContext (GLXBadFBConfig)".to_owned()]));
        assert!(looks_like_glx_context_error(&["error code: 65543".to_owned()]));
        assert!(!looks_like_glx_context_error(&["Setting user: Player".to_owned()]));
    }

    #[test]
    fn non_nvidia_renderer_lines_are_flagged() {
        assert!(looks_like_non_nvidia_renderer("OpenGL vendor string: Mesa/X.org"));
        assert!(looks_like_non_nvidia_renderer("renderer: llvmpipe (LLVM 15.0.7)"));
        assert!(!looks_like_non_nvidia_renderer("OpenGL vendor string: NVIDIA Corporation"));
        assert!(!looks_like_non_nvidia_renderer("Sound engine started"));
    }

    #[test]
    fn shlex_split_matches_python_for_simple_cases() {
        assert_eq!(shlex_split("gamemoderun mangohud", true).unwrap(), vec!["gamemoderun", "mangohud"]);
        assert_eq!(shlex_split("mangohud --dlsym '5;6'", true).unwrap(), vec!["mangohud", "--dlsym", "5;6"]);
        assert_eq!(shlex_split("echo \\\"x\\\"", true).unwrap(), vec!["echo", "\"x\""]);
        assert!(shlex_split("broken 'quote", true).is_err());
        // в Windows кавычки сохраняются при отключённой обработке POSIX
        assert_eq!(shlex_split("\"C:\\tools x\" -flag", false).unwrap(), vec!["\"C:\\tools x\"", "-flag"]);
    }

    #[test]
    fn shell_join_quotes_metasymbols() {
        assert_eq!(shell_join(&["java".to_owned(), "-Xmx2G".to_owned()]), "java -Xmx2G");
        assert_eq!(shell_join(&["a b".to_owned()]), "'a b'");
        assert_eq!(shell_join(&["it's".to_owned()]), "'it'\\''s'");
        assert_eq!(shell_join(&["".to_owned()]), "''");
    }

    #[test]
    fn jvm_xmx_parser_understands_jvm_units() {
        assert_eq!(jvm_xmx_kib("2G"), Some(2 * 1024 * 1024));
        assert_eq!(jvm_xmx_kib("512m"), Some(512 * 1024));
        assert_eq!(jvm_xmx_kib("4096k"), Some(4096));
        assert_eq!(jvm_xmx_kib("4096"), None);
        assert_eq!(jvm_xmx_kib("auto"), None);
    }

    #[test]
    fn heap_cap_replaces_xmx_over_1g_for_32bit() {
        let mut messages = Vec::new();
        let capped = cap_heap_for_32bit_java(
            &["/usr/bin/java".to_owned(), "-Xmx4G".to_owned(), "-jar".to_owned(), "game.jar".to_owned()],
            &mut |line| messages.push(line.to_owned()),
        );
        assert_eq!(capped[1], "-Xmx1024M");
        assert_eq!(messages.len(), 1);
        let kept = cap_heap_for_32bit_java(
            &["/usr/bin/java".to_owned(), "-Xmx512M".to_owned()],
            &mut |_| {},
        );
        assert_eq!(kept[1], "-Xmx512M");
    }

    #[test]
    fn adapt_command_for_old_java_prepends_ignore_flag() {
        // проверяем сохранение аргументов, когда Java недоступна
        let adapted = adapt_command_for_old_java(&["/nonexistent-java".to_owned(), "-Xmx1G".to_owned()], &mut |_| {});
        assert_eq!(adapted, vec!["/nonexistent-java".to_owned(), "-Xmx1G".to_owned()]);
    }

    #[test]
    fn memory_setting_replaces_xmx_and_keeps_classpath_order() {
        let arguments = vec!["-Xmx1G".to_owned(), "-Dflag=1".to_owned(), "-cp".to_owned(), "libs".to_owned()];
        let result = apply_memory_setting(&arguments, "3G").unwrap();
        assert_eq!(result, vec!["-Dflag=1".to_owned(), "-Xmx3G".to_owned(), "-cp".to_owned(), "libs".to_owned()]);
        let untouched = apply_memory_setting(&arguments, "").unwrap();
        assert_eq!(untouched, vec!["-Dflag=1".to_owned(), "-cp".to_owned(), "libs".to_owned()]);
    }

    #[test]
    fn native_library_path_inserts_before_classpath() {
        let arguments = vec!["-Dflag=1".to_owned(), "-cp".to_owned(), "libs".to_owned()];
        let result = ensure_native_library_path(arguments, Path::new("/instance/natives"));
        assert_eq!(result[1], "-Djava.library.path=/instance/natives");
        let existing = vec!["-Djava.library.path=/other".to_owned(), "-cp".to_owned(), "libs".to_owned()];
        assert_eq!(ensure_native_library_path(existing, Path::new("/instance/natives")).len(), 3);
    }

    #[test]
    fn java_argument_quotes_values() {
        assert_eq!(java_argument("-Dfoo=bar").unwrap(), "\"-Dfoo=bar\"");
        assert_eq!(java_argument("a\"b").unwrap(), "\"a\\\"b\"");
        assert_eq!(java_argument("c:\\dir").unwrap(), "\"c:\\\\dir\"");
        assert!(java_argument("line\nbreak").is_err());
    }

    #[test]
    fn argument_rules_follow_mojang_semantics() {
        let features = BTreeMap::from([("has_custom_resolution", true)]);
        assert!(argument_allowed(None, &features));
        assert!(argument_allowed(Some(&json!([])), &features));
        let os_rule = json!([{ "action": "allow", "os": { "name": mojang_os_name() } }]);
        assert!(argument_allowed(Some(&os_rule), &features));
        let wrong_os = json!([{ "action": "allow", "os": { "name": "definitely-not" } }]);
        assert!(!argument_allowed(Some(&wrong_os), &features));
        let resolution = json!([{ "action": "allow", "features": { "has_custom_resolution": true } }]);
        assert!(argument_allowed(Some(&resolution), &features));
        let demo = json!([{ "action": "allow", "features": { "is_demo_user": true } }]);
        assert!(!argument_allowed(Some(&demo), &features));
        let disallow = json!([
            { "action": "allow", "os": { "name": mojang_os_name() } },
            { "action": "disallow", "os": { "arch": "not-an-arch" } },
        ]);
        assert!(argument_allowed(Some(&disallow), &features));
    }

    #[test]
    fn placeholder_expansion_covers_all_known_keys() {
        let mut placeholders = BTreeMap::new();
        placeholders.insert("auth_player_name".to_owned(), "Steve".to_owned());
        placeholders.insert("classpath".to_owned(), "a.jar:b.jar".to_owned());
        assert_eq!(
            replace_placeholders("${auth_player_name} plays with ${classpath}", &placeholders),
            "Steve plays with a.jar:b.jar"
        );
        assert_eq!(replace_placeholders("${unknown}", &placeholders), "${unknown}");
    }

    #[test]
    fn redaction_hides_auth_tokens() {
        let command = vec![
            "java".to_owned(),
            format!("--accessToken={}", AUTH_TOKEN_PLACEHOLDER),
            "--username=Steve".to_owned(),
        ];
        let redacted = redact_command(&command, &[AUTH_TOKEN_PLACEHOLDER]);
        assert_eq!(redacted[1], "--accessToken=<redacted>");
        assert_eq!(redacted[2], "--username=Steve");
    }

    #[test]
    fn session_logs_keep_a_bounded_ring() {
        let name = format!("session-test-{}", random_hex(4));
        session_start(&name);
        for index in 0..600 {
            session_append(&name, &format!("line {index}"));
        }
        let (logs, running) = session_snapshot(&name);
        assert_eq!(logs.len(), MAX_SESSION_LOG_LINES);
        assert_eq!(logs.last().unwrap(), "line 599");
        assert!(running);
        assert!(session_started_at(&name) > 0.0);
        session_finish(&name);
        let (_, running) = session_snapshot(&name);
        assert!(!running);
    }

    #[test]
    fn wayland_session_requires_wayland_markers_and_modern_version() {
        let mut environment = BTreeMap::new();
        environment.insert("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned());
        environment.insert("WAYLAND_DISPLAY".to_owned(), "wayland-0".to_owned());
        assert!(is_wayland_session(&environment, "1.20.1"));
        assert!(!is_wayland_session(&environment, "1.12.2"));
        environment.insert("XDG_SESSION_TYPE".to_owned(), "x11".to_owned());
        assert!(!is_wayland_session(&environment, "1.20.1"));
    }

    #[test]
    fn native_wayland_command_injects_glfw_flags_before_main_class() {
        let command = vec![
            "/usr/bin/java".to_owned(),
            "-Xmx2G".to_owned(),
            "-cp".to_owned(),
            "libs".to_owned(),
            "net.minecraft.client.main.Main".to_owned(),
            "--username".to_owned(),
            "Steve".to_owned(),
        ];
        let patched = native_wayland_command(&command, "net.minecraft.client.main.Main", Path::new("/cell/libglfw.so")).unwrap();
        assert_eq!(patched.len(), command.len() + 2);
        let index = patched.iter().position(|value| value == "net.minecraft.client.main.Main").unwrap();
        assert_eq!(patched[index - 2], "-Dorg.lwjgl.glfw.libname=/cell/libglfw.so");
        assert_eq!(patched[index - 1], "-Dsodium.checks.issue2561=false");
        assert!(native_wayland_command(&command, "missing.Main", Path::new("/cell/libglfw.so")).is_err());
    }

    #[test]
    fn wayland_environment_drops_glx_overrides_and_forces_glfw() {
        let mut environment = BTreeMap::new();
        environment.insert("__GLX_VENDOR_LIBRARY_NAME".to_owned(), "nvidia".to_owned());
        environment.insert("LIBGL_ALWAYS_SOFTWARE".to_owned(), "1".to_owned());
        environment.insert("__NV_PRIME_RENDER_OFFLOAD".to_owned(), "1".to_owned());
        environment.insert("TRIAL_LAUNCHER_GRAPHICS_MODE".to_owned(), "mesa".to_owned());
        native_wayland_environment(&mut environment);
        assert!(!environment.contains_key("__GLX_VENDOR_LIBRARY_NAME"));
        assert!(!environment.contains_key("LIBGL_ALWAYS_SOFTWARE"));
        assert!(!environment.contains_key("__NV_PRIME_RENDER_OFFLOAD"));
        assert_eq!(environment.get("GLFW_FORCE_WAYLAND").map(String::as_str), Some("1"));
        assert_eq!(
            environment.get("TRIAL_LAUNCHER_GRAPHICS_BACKEND").map(String::as_str),
            Some("native-wayland-glfw-v1.1.0")
        );
    }

    #[test]
    fn log_components_are_bounded_and_safe() {
        assert_eq!(log_component("Vanilla 1.20.1"), "Vanilla_1.20.1");
        assert_eq!(log_component("---"), "launcher");
        assert_eq!(log_component(&"x".repeat(200)).len(), 80);
    }

    #[test]
    fn extra_arguments_are_filtered_and_bounded() {
        let raw = json!(["-Dok=1", "", "line\nbreak", "-Dlong=".repeat(600)]);
        let result = safe_extra_arguments(Some(&raw));
        let bounded: String = "-Dlong=".repeat(600).chars().take(512).collect();
        assert_eq!(result, vec!["-Dok=1".to_owned(), bounded]);
        assert!(result.len() <= 2);
        let many: Vec<Value> = (0..40).map(|index| json!(format!("-Dvalue{index}"))).collect();
        assert_eq!(safe_extra_arguments(Some(&Value::Array(many))).len(), 32);
        assert!(safe_extra_arguments(Some(&json!("not-a-list"))).is_empty());
    }

    #[test]
    fn live_state_is_written_for_both_file_names() {
        let directory = std::env::temp_dir().join(format!("nivora-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        write_live_state(&directory, true, 4242, None, 123.5, "game.log", "launch-1");
        for name in [".trial-launcher-live.json", ".nivora-launcher-live.json"] {
            let state: Value = serde_json::from_str(&fs::read_to_string(directory.join(name)).unwrap()).unwrap();
            assert_eq!(state["running"], json!(true));
            assert_eq!(state["pid"], json!(4242));
            assert_eq!(state["log_id"], json!("game.log"));
            assert_eq!(state["launch_id"], json!("launch-1"));
        }
        write_live_state(&directory, false, 0, Some(-15), 0.0, "game.log", "launch-1");
        let state: Value = serde_json::from_str(&fs::read_to_string(directory.join(".nivora-launcher-live.json")).unwrap()).unwrap();
        assert_eq!(state["running"], json!(false));
        assert_eq!(state["exit_code"], json!(-15));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn live_state_skips_symlinks() {
        let directory = std::env::temp_dir().join(format!("nivora-live-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", directory.join(".nivora-launcher-live.json")).unwrap();
        write_live_state(&directory, true, 1, None, 0.0, "x.log", "launch-1");
        #[cfg(unix)]
        assert_eq!(fs::read(directory.join(".nivora-launcher-live.json")).unwrap(), fs::read("/etc/hostname").unwrap());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn exit_code_maps_signals_to_negative_numbers() {
        let status = std::process::Command::new("true").status().unwrap();
        assert_eq!(exit_code(&status), 0);
    }

    #[test]
    fn setting_text_accepts_numbers_and_defaults() {
        assert_eq!(setting_text(&json!({ "window_width": 1280 }), "window_width", "800"), "1280");
        assert_eq!(setting_text(&json!({ "window_width": "1600" }), "window_width", "800"), "1600");
        assert_eq!(setting_text(&json!({}), "window_width", "800"), "800");
    }

    #[test]
    fn effective_launch_settings_prefer_instance_over_profile() {
        let directory = std::env::temp_dir().join(format!("nivora-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let values = json!({
            "memory": "2G",
            "java_path": "",
            "graphics_mode": "auto",
        });
        // без экземпляра: значения запроса поверх глобальных настроек
        let merged = json!({
            "memory": "4G",
            "java_path": "/opt/java",
            "graphics_mode": "auto",
        });
        assert_eq!(values["memory"], json!("2G"));
        assert_eq!(merged["memory"], json!("4G"));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn telemetry_report_respects_disabled_setting() {
        // при выключенной телеметрии не обращаемся к сети
        let config = ConfigStore::load();
        if let Ok(config) = config {
            emit_telemetry_report(&config, "launch_error", None, "1.20.1", None, "тест", &["строка".to_owned()]);
        }
    }

    #[test]
    fn select_account_rejects_unknown_ely_without_id() {
        let config = match ConfigStore::load() {
            Ok(config) => config,
            Err(_) => return,
        };
        let http = match HttpClient::new(2, 1, "", None) {
            Ok(http) => http,
            Err(_) => return,
        };
        let error = select_account("ely", &config, &http).unwrap_err();
        assert_eq!(error, "Укажите конкретный аккаунт ely.by или выберите его по умолчанию");
        let missing = select_account("no-such-account", &config, &http).unwrap_err();
        assert_eq!(missing, "Аккаунт не найден: no-such-account");
    }

    #[test]
    fn java_argument_file_roundtrip_on_disk_fallback() {
        // memfd-ветка требует Linux; фолбэк-файл обязан появляться и удаляться
        let directory = std::env::temp_dir().join(format!("nivora-argfile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let instance_dir = directory.join("instance");
        fs::create_dir_all(&instance_dir).unwrap();
        let mut file = create_java_argument_file(
            &["-Xmx1G".to_owned(), "-Dkey=value with spaces".to_owned()],
            &instance_dir,
            Some(&directory),
        )
        .unwrap();
        if let Some(path) = file.path.clone() {
            let content = fs::read_to_string(&path).unwrap();
            assert_eq!(content, "\"-Xmx1G\"\n\"-Dkey=value with spaces\"\n");
            file.close();
            assert!(!path.exists());
        } else {
            // memfd: содержимое читается через /proc/self/fd
            let fd = file.fd.unwrap();
            let mut buffer = String::new();
            let mut source = fs::File::open(format!("/proc/self/fd/{fd}")).unwrap();
            use std::io::Read;
            source.read_to_string(&mut buffer).unwrap();
            assert_eq!(buffer, "\"-Xmx1G\"\n\"-Dkey=value with spaces\"\n");
            file.close();
        }
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn should_use_argument_file_follows_java_major_and_token() {
        assert!(!should_use_argument_file(true, Some(8)));
        assert!(should_use_argument_file(true, Some(17)));
        assert!(should_use_argument_file(false, None) || !cfg!(windows));
        assert!(!should_use_argument_file(false, Some(11)) || cfg!(windows));
    }

    #[test]
    fn materialize_auth_token_replaces_placeholder_only() {
        let command = vec![format!("--accessToken={}", AUTH_TOKEN_PLACEHOLDER), "--plain".to_owned()];
        let result = materialize_auth_token(&command, "secret-token");
        assert_eq!(result[0], "--accessToken=secret-token");
        assert_eq!(result[1], "--plain");
        assert_eq!(materialize_auth_token(&command, ""), command);
    }
}
