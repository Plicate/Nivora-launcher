use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::config::{format_memory_kib, java_memory_value, parse_memory_kib, system_memory_kib, ConfigStore};
use super::constants::MAX_SERVER_CONFIG_JSON_BYTES;
use super::instances::{InstanceInfo, InstanceStore};
use super::net::atomic_json_write;
use super::operations::CancellationToken;
use super::runtime::{self, JavaRuntimeManager};
use super::process_guard::InstanceProcessLock;
use super::state::{read_bounded_json, safe_instance_name};

pub(crate) const SERVER_JAR: &str = "server.jar";
const SERVER_PROPERTIES_FILE: &str = "server.properties";
const MAX_SERVER_PROPERTIES_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const SERVER_CONFIG_FILE: &str = ".nivora-launcher-server.json";
const LIVE_STATE_FILE: &str = ".nivora-launcher-live.json";
const CONSOLE_FILE: &str = ".nivora-launcher-server-console.json";
const METRICS_FILE: &str = ".nivora-launcher-metrics.json";
const MAX_CONSOLE_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_METRICS_JSON_BYTES: u64 = 512 * 1024;
const MAX_LIVE_STATE_JSON_BYTES: u64 = 64 * 1024;
const MAX_LOG_LINES: usize = 500;
const MAX_LOG_LINE_CHARS: usize = 2048;
const UNKNOWN_RAM_SERVER_LIMIT_KIB: i64 = 2 * 1024 * 1024;
const SERVER_SECURITY_KEYS: [&str; 3] = ["java_path", "security_profile", "security_permissions"];
const SERVER_CORE_IDS: [&str; 10] = [
    "paper", "purpur", "folia", "spigot", "leaf", "pufferfish", "patina", "velocity", "waterfall", "custom",
];
const PROTECTED_SETTINGS_FORMAT: i64 = 1;
const MAX_PROTECTED_SETTINGS_BYTES: u64 = 64 * 1024;

static PROCESSES: LazyLock<Mutex<HashMap<String, ServerProcess>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static LAUNCH_ERRORS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_map<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Default)]
struct MetricsCache {
    previous_cpu_seconds: Option<f64>,
    previous_sample_at: f64,
    peak_ram_bytes: u64,
    peak_cpu_percent: f64,
}

struct ServerProcess {
    launch_id: String,
    root: PathBuf,
    pid: u32,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    started_at: f64,
    logs: VecDeque<(u64, String)>,
    log_sequence: u64,
    players: i64,
    max_players: i64,
    tps: f64,
    exit_code: Option<i32>,
    stop_requested: bool,
    finished: Arc<FinishFlag>,
    process_lock: Option<InstanceLock>,
    scheduled_restart_date: String,
    metrics: MetricsCache,
    traffic: Option<super::server_traffic::TrafficHandle>,
}

impl ServerProcess {
    fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }
}

struct FinishFlag {
    done: Mutex<bool>,
    signal: Condvar,
}

impl FinishFlag {
    fn new() -> Self {
        Self { done: Mutex::new(false), signal: Condvar::new() }
    }

    fn set(&self) {
        let mut done = lock_map(&self.done);
        *done = true;
        self.signal.notify_all();
    }

    fn wait_for(&self, timeout: Duration) -> bool {
        let done = lock_map(&self.done);
        let (done, _) = self
            .signal
            .wait_timeout_while(done, timeout, |flag| !*flag)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *done
    }
}

pub(crate) struct InstanceLock {
    #[allow(dead_code)]
    handle: File,
}

impl InstanceLock {
    pub(crate) fn acquire(data_home: &Path, instance_name: &str) -> Result<Option<Self>, String> {
        #[cfg(not(unix))]
        {
            let _ = (data_home, instance_name);
            Ok(None)
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            use std::os::unix::io::AsRawFd;

            let safe: String = instance_name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
                .collect();
            let safe = safe.trim_matches(|c| matches!(c, '.' | '-' | '_'));
            let safe = if safe.is_empty() { "instance" } else { safe };
            let mut hasher = Sha256::new();
            hasher.update(instance_name.as_bytes());
            let hash = format!("{:x}", hasher.finalize());
            let directory = data_home.join(".locks");
            fs::create_dir_all(&directory).map_err(|error| format!("Не удалось открыть блокировку экземпляра: {error}"))?;
            let _ = fs::set_permissions(&directory, fs::Permissions::from_mode(0o700));
            let path = directory.join(format!("{}-{}.lock", take_chars(safe, 64), &hash[..16]));
            let handle = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|error| format!("Не удалось открыть блокировку экземпляра: {error}"))?;
            let descriptor = handle.as_raw_fd();
            if unsafe { libc::flock(descriptor, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let error = std::io::Error::last_os_error();
                drop(handle);
                return match error.raw_os_error() {
                    Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN || code == libc::EACCES => {
                        Err("Экземпляр уже запущен".to_owned())
                    }
                    _ => Err(format!("Не удалось заблокировать экземпляр: {error}")),
                };
            }
            Ok(Some(Self { handle }))
        }
    }
}

#[cfg(unix)]
impl Drop for InstanceLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe { libc::flock(self.handle.as_raw_fd(), libc::LOCK_UN); }
    }
}

pub fn server_start(request: &Value) -> Result<Value, String> {
    start_server(&py_str(request.get("name"), ""))
}

pub fn server_stop(request: &Value) -> Result<Value, String> {
    stop_server(&py_str(request.get("name"), ""))
}

pub fn server_restart(request: &Value) -> Result<Value, String> {
    restart_server(&py_str(request.get("name"), ""))
}

fn validate_server_command(command: &str) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty() || command.chars().count() > 2000 {
        return Err("Команда сервера пуста или слишком длинная".to_owned());
    }
    if command.chars().any(char::is_control) {
        return Err("Команда сервера не должна содержать управляющие символы".to_owned());
    }
    Ok(command.to_owned())
}

pub fn server_command(request: &Value) -> Result<Value, String> {
    let name = py_str(request.get("name"), "");
    let value = validate_server_command(&py_str(request.get("command"), ""))?;
    let (config, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(&info.name) {
            Some(entry) => {
                // `try_wait` требует `&mut`, поэтому проверяем здесь
                if !entry.is_running() {
                    return Err("Сервер не запущен".to_owned());
                }
                let Some(stdin) = entry.stdin.as_mut() else {
                    return Err("Сервер не запущен".to_owned());
                };
                stdin
                    .write_all(format!("{value}\n").as_bytes())
                    .and_then(|_| stdin.flush())
                    .map_err(|error| error.to_string())?;
            }
            None => return Err("Сервер не запущен".to_owned()),
        }
    }
    status_server_with(&config, &instances, &name)
}

pub fn server_log(request: &Value) -> Result<Value, String> {
    let name = py_str(request.get("name"), "");
    let since = match request.get("since") {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    let (_, instances) = stores()?;
    let info = server_instance(&instances, &name)?;
    server_log_for(&instances, &info, since)
}

/// возвращает лог сервера с указанного курсора
pub(crate) fn server_log_for(_instances: &InstanceStore, info: &InstanceInfo, since: i64) -> Result<Value, String> {
    let live = {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(&info.name) {
            Some(entry) => Some((entry.log_sequence, entry.is_running(), entry.logs.iter().cloned().collect::<Vec<_>>())),
            None => None,
        }
    };
    match live {
        Some((sequence, running, entries)) => Ok(log_response(&entries, since, sequence as i64, running)),
        None => {
            // отрицательный курсор заставляет клиента перечитать лог после запуска
            let saved = saved_console(&info.path);
            let cursor = saved.get("cursor").and_then(Value::as_i64).unwrap_or(-1);
            let lines = if since == cursor { json!([]) } else { saved.get("lines").cloned().unwrap_or(json!([])) };
            Ok(json!({
                "lines": lines,
                "running": false,
                "cursor": cursor,
                "reset": since != 0 && since != cursor,
                "error": saved.get("error").cloned().unwrap_or(json!("")),
            }))
        }
    }
}

pub fn server_status(request: &Value) -> Result<Value, String> {
    let name = py_str(request.get("name"), "");
    let (config, instances) = stores()?;
    status_server_with(&config, &instances, &name)
}

pub(crate) fn stores() -> Result<(ConfigStore, InstanceStore), String> {
    let config = ConfigStore::load()?;
    let instances = InstanceStore::new(&config)?;
    Ok((config, instances))
}

pub(crate) fn server_instance(instances: &InstanceStore, name: &str) -> Result<InstanceInfo, String> {
    let info = instances.get(name)?;
    if info.kind != "server" {
        return Err(format!("Экземпляр «{name}» не является сервером"));
    }
    Ok(info)
}

fn start_server(name: &str) -> Result<Value, String> {
    let (config, instances) = stores()?;
    let instance = server_instance(&instances, name);
    let key = instance.as_ref().map(|info| info.name.clone()).unwrap_or_else(|_| name.to_owned());
    match start_server_inner(&config, &instances, name) {
        Ok(value) => {
            lock_map(&LAUNCH_ERRORS).remove(&key);
            Ok(value)
        }
        Err(error) => {
            lock_map(&LAUNCH_ERRORS).insert(key.clone(), error.clone());
            {
                let mut map = lock_map(&PROCESSES);
                if let Some(entry) = map.get_mut(&key) {
                    if !entry.is_running() {
                        push_log(entry, &format!("[launcher] Ошибка запуска: {error}"));
                    }
                }
            }
            if let Ok(info) = &instance {
                let saved = saved_console(&info.path);
                let mut lines: Vec<String> = saved
                    .get("lines")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                lines.push(format!("[launcher] Ошибка запуска: {error}"));
                let _ = save_console(&info.path, &lines, &error);
            }
            Err(error)
        }
    }
}

fn start_server_inner(config: &ConfigStore, instances: &InstanceStore, name: &str) -> Result<Value, String> {
    let info = server_instance(instances, name)?;
    let already_ours = {
        let mut map = lock_map(&PROCESSES);
        map.get_mut(&info.name).map(|entry| entry.is_running()).unwrap_or(false)
    };
    if already_ours {
        return status_server_with(config, instances, name);
    }
    if server_is_running(&info.path) {
        return Err("Сервер уже запущен другим процессом".to_owned());
    }

    let settings = server_config(config, instances, &info.name)?;
    if !setting_bool(&settings, "eula_accepted") {
        return Err("Сначала примите EULA сервера".to_owned());
    }
    let jar = info.path.join(SERVER_JAR);
    let jar_ready = fs::symlink_metadata(&jar).map(|meta| !meta.is_symlink() && meta.is_file()).unwrap_or(false);
    if !jar_ready {
        return Err("Ядро не найдено. Загрузите server.jar или выберите официальное ядро.".to_owned());
    }

    let configured_java = {
        let value = setting_str(&settings, "java_path");
        if value.trim().is_empty() {
            let global = config.str_setting("java_path");
            if global.trim().is_empty() { "java".to_owned() } else { global }
        } else {
            value
        }
    };
    let version = setting_str(&settings, "version");
    let (required, component) = server_java_requirement(&version);
    let java = if configured_java == "java" {
        // используем системную Java только подходящей версии
        let mut ready: Option<PathBuf> = None;
        if let Some(found) = which_java() {
            let candidate = runtime::validate_java_path(&found, false)?;
            let actual = runtime::java_version(&candidate);
            if required == 0 || actual.map(|major| major >= required).unwrap_or(false) {
                ready = Some(candidate);
            }
        }
        match ready {
            Some(path) => path,
            None => {
                let major = if required > 0 { json!(required) } else { Value::Null };
                let http = super::net::progress_client_from_settings(config.settings(), "")?;
                JavaRuntimeManager::new(config, Some(http))?
                    .java_for(&json!({ "javaVersion": { "majorVersion": major, "component": component } }), None)?
            }
        }
    } else {
        let candidate = runtime::validate_java_path(Path::new(&configured_java), true)?;
        if let Some(actual) = runtime::java_version(&candidate) {
            if required > 0 && actual > 0 && actual < required {
                return Err(format!(
                    "Java {actual} слишком старая для Minecraft {version}; требуется Java {required}. Выберите подходящую Java в настройках сервера."
                ));
            }
        }
        candidate
    };

    let memory = java_memory_value(&setting_str(&settings, "memory"))?;
    let environment = runtime::safe_process_environment(false);
    let core = setting_str(&settings, "core");
    let mut arguments: Vec<String> = Vec::new();
    arguments.extend([
        format!("-Xms{}", initial_server_heap(&memory)),
        format!("-Xmx{memory}"),
        "-jar".to_owned(),
        SERVER_JAR.to_owned(),
    ]);
    if core != "velocity" && core != "waterfall" {
        arguments.push("--nogui".to_owned());
    }

    let (launch_id, started_at, pid, stdout_stream, stderr_stream) = {
        let mut map = lock_map(&PROCESSES);
        if server_is_running(&info.path) {
            return Err("Сервер уже запущен другим процессом".to_owned());
        }
        let process_lock = InstanceLock::acquire(&config.paths.data_home, &info.name)?;
        apply_server_port(&info.path, setting_i64(&settings, "port", 25565), false)?;
        let program = java.clone();
        let mut command = super::platform::command(&program);
        command
            .args(&arguments)
            .current_dir(&info.path)
            .env_clear()
            .envs(&environment);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // группа процессов позволяет остановить и потомков сервера
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return Err(format!("Не удалось запустить сервер: {error}")),
        };
        let stdin = child.stdin.take();
        let stdout_stream = child.stdout.take();
        let stderr_stream = child.stderr.take();
        let launch_id = unique_id();
        let started_at = epoch_seconds();
        let pid = child.id();
        // курсор логов продолжается между запусками
        let previous_sequence = map.get(&info.name).map(|entry| entry.log_sequence).unwrap_or(0);
        map.insert(
            info.name.clone(),
            ServerProcess {
                launch_id: launch_id.clone(),
                root: info.path.clone(),
                pid,
                child: Some(child),
                stdin,
                started_at,
                logs: VecDeque::new(),
                log_sequence: previous_sequence,
                players: 0,
                max_players: setting_i64(&settings, "max_players", 20),
                tps: 0.0,
                exit_code: None,
                stop_requested: false,
                finished: Arc::new(FinishFlag::new()),
                process_lock,
                scheduled_restart_date: String::new(),
                metrics: MetricsCache::default(),
                traffic: Some(std::sync::Arc::new(std::sync::Mutex::new(
                    super::server_traffic::ServerTrafficTracker::new(pid, setting_i64(&settings, "port", 25565) as u16),
                ))),
            },
        );
        (launch_id, started_at, pid, stdout_stream, stderr_stream)
    };
    // ошибка записи статуса не должна осиротить запущенный процесс
    let _ = write_live(&info.path, true, pid as i64, started_at);

    let stdout_reader = stdout_stream.map(|stream| {
        let key = info.name.clone();
        let launch_id = launch_id.clone();
        thread::spawn(move || read_output_stream(key, launch_id, stream))
    });
    let stderr_reader = stderr_stream.map(|stream| {
        let key = info.name.clone();
        let launch_id = launch_id.clone();
        thread::spawn(move || drain_output_stream(key, launch_id, stream))
    });
    let readers: Vec<_> = [stdout_reader, stderr_reader].into_iter().flatten().collect();
    let finish_key = info.name.clone();
    thread::spawn(move || {
        after_output_readers_finish(readers, || finish_process(finish_key, launch_id));
    });
    status_server_with(config, instances, name)
}

fn stop_server(name: &str) -> Result<Value, String> {
    let (config, instances) = stores()?;
    stop_server_with(&config, &instances, name)
}

fn stop_server_with(config: &ConfigStore, instances: &InstanceStore, name: &str) -> Result<Value, String> {
    let info = server_instance(instances, name)?;
    let key = info.name.clone();
    let launch = {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(&key) {
            Some(entry) => {
                if entry.is_running() {
                    Some(entry.launch_id.clone())
                } else {
                    None
                }
            }
            None => None,
        }
    };
    let Some(launch_id) = launch else {
        return status_server_with(config, instances, name);
    };

    let graceful = {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(&key) {
            Some(entry) if entry.launch_id == launch_id => {
                entry.stop_requested = true;
                match entry.stdin.as_mut() {
                    Some(stdin) => stdin.write_all(b"stop\n").and_then(|_| stdin.flush()).is_ok(),
                    None => false,
                }
            }
            _ => false,
        }
    };
    if graceful && wait_exit(&key, &launch_id, 8.0) {
        if !wait_finished(&key, 10.0) {
            return Err("Сервер завершился, но очистка запуска ещё не закончена; повторите действие".to_owned());
        }
        return status_server_with(config, instances, name);
    }

    terminate_tree(&key, &launch_id, false);
    if !wait_exit(&key, &launch_id, 5.0) {
        terminate_tree(&key, &launch_id, true);
        if !wait_exit(&key, &launch_id, 5.0) {
            return Err("Сервер не завершился после принудительной остановки".to_owned());
        }
    }
    if !wait_finished(&key, 10.0) {
        return Err("Очистка предыдущего запуска ещё не завершена".to_owned());
    }
    status_server_with(config, instances, name)
}

fn restart_server(name: &str) -> Result<Value, String> {
    let (config, instances) = stores()?;
    let info = server_instance(&instances, name)?;
    let key = info.name.clone();
    stop_server_with(&config, &instances, name)?;
    let previous = {
        let map = lock_map(&PROCESSES);
        map.get(&key).map(|entry| entry.finished.clone())
    };
    if let Some(flag) = previous {
        if !flag.wait_for(Duration::from_secs(10)) {
            return Err("Предыдущий запуск ещё завершает очистку".to_owned());
        }
    }
    start_server(name)
}

fn wait_exit(key: &str, launch_id: &str, seconds: f64) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    loop {
        let exited = {
            let mut map = lock_map(&PROCESSES);
            match map.get_mut(key) {
                None => true,
                Some(entry) => entry.launch_id != launch_id || !entry.is_running(),
            }
        };
        if exited {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_finished(key: &str, seconds: f64) -> bool {
    let finished = {
        let map = lock_map(&PROCESSES);
        map.get(key).map(|entry| entry.finished.clone())
    };
    match finished {
        Some(flag) => flag.wait_for(Duration::from_secs_f64(seconds.max(0.0))),
        None => true,
    }
}

fn terminate_tree(key: &str, launch_id: &str, force: bool) {
    #[cfg(unix)]
    {
        let pid = {
            let map = lock_map(&PROCESSES);
            map.get(key)
                .filter(|entry| entry.launch_id == launch_id)
                .map(|entry| entry.pid as i32)
        };
        if let Some(pid) = pid {
            let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
            let group_result = unsafe { libc::kill(-pid, signal) };
            if group_result != 0 {
                unsafe { libc::kill(pid, signal) };
            }
        }
    }
    #[cfg(not(unix))]
    {
        if force {
            let mut map = lock_map(&PROCESSES);
            if let Some(entry) = map.get_mut(key) {
                if entry.launch_id == launch_id {
                    if let Some(child) = entry.child.as_mut() {
                        let _ = child.kill();
                    }
                }
            }
        }
    }
}

fn read_output_stream(key: String, launch_id: String, stream: ChildStdout) {
    for line in BufReader::new(stream).lines() {
        match line {
            Ok(text) => append_log(&key, &launch_id, &text),
            Err(_) => break,
        }
    }
}

fn drain_output_stream(key: String, launch_id: String, stream: ChildStderr) {
    for line in BufReader::new(stream).lines() {
        match line {
            Ok(text) => append_log(&key, &launch_id, &text),
            Err(_) => break,
        }
    }
}

fn after_output_readers_finish(
    readers: Vec<thread::JoinHandle<()>>,
    after: impl FnOnce(),
) {
    for reader in readers {
        let _ = reader.join();
    }
    after();
}

fn append_log(key: &str, launch_id: &str, raw: &str) {
    let mut map = lock_map(&PROCESSES);
    if let Some(entry) = map.get_mut(key) {
        if entry.launch_id == launch_id {
            push_log(entry, raw);
        }
    }
}

fn push_log(entry: &mut ServerProcess, raw: &str) {
    let line = tail_chars(raw.trim_end_matches(['\r', '\n']), MAX_LOG_LINE_CHARS);
    entry.log_sequence += 1;
    entry.logs.push_back((entry.log_sequence, line.clone()));
    while entry.logs.len() > MAX_LOG_LINES {
        entry.logs.pop_front();
    }
    parse_server_line(entry, &line);
}

fn parse_server_line(entry: &mut ServerProcess, line: &str) {
    static PLAYERS: OnceLock<Regex> = OnceLock::new();
    static TPS: OnceLock<Regex> = OnceLock::new();
    let players = PLAYERS.get_or_init(|| {
        Regex::new(r"There are (\d+) of a max of (\d+) players online").unwrap()
    });
    if let Some(captures) = players.captures(line) {
        if let Some(value) = captures.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
            entry.players = value;
        }
        if let Some(value) = captures.get(2).and_then(|m| m.as_str().parse::<i64>().ok()) {
            entry.max_players = value;
        }
    }
    let tps = TPS.get_or_init(|| Regex::new(r"(?:TPS|tps)[:= ]+([0-9]+(?:\.[0-9]+)?)").unwrap());
    if let Some(captures) = tps.captures(line) {
        if let Some(value) = captures.get(1).and_then(|m| m.as_str().parse::<f64>().ok()) {
            entry.tps = value;
        }
    }
}

fn finish_process(key: String, launch_id: String) {
    loop {
        {
            let mut map = lock_map(&PROCESSES);
            match map.get_mut(&key) {
                None => break,
                Some(entry) => {
                    if entry.launch_id != launch_id {
                        break;
                    }
                    if let Some(child) = entry.child.as_mut() {
                        if let Ok(Some(status)) = child.try_wait() {
                            entry.exit_code = status.code();
                            break;
                        }
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    let mut should_restart = false;
    let needs_check = {
        let map = lock_map(&PROCESSES);
        map.get(&key)
            .map(|entry| entry.launch_id == launch_id && !entry.stop_requested)
            .unwrap_or(false)
    };
    if needs_check {
        // читаем настройки вне блокировки процессов: это файловый ввод-вывод
        if let Ok((config, instances)) = stores() {
            if let Ok(settings) = server_config(&config, &instances, &key) {
                should_restart = setting_bool(&settings, "auto_restart");
            }
        }
    }

    let mut is_current = false;
    let mut console_lines: Vec<String> = Vec::new();
    let mut root = PathBuf::new();
    {
        let mut map = lock_map(&PROCESSES);
        if let Some(entry) = map.get_mut(&key) {
            if entry.launch_id == launch_id {
                is_current = true;
                root = entry.root.clone();
                let exit_code = entry.exit_code;
                let mut cache = entry.metrics.clone();
                let snapshot = compute_metrics(
                    &mut cache,
                    entry.pid,
                    entry.started_at,
                    false,
                    entry.players,
                    entry.max_players,
                    entry.tps,
                    process_tree_sample(entry.pid as i32),
                    directory_size(&entry.root),
                    epoch_seconds(),
                );
                entry.metrics = cache;
                let mut snapshot = snapshot;
                snapshot["ended_at"] = json!(epoch_seconds());
                snapshot["exit_code"] = json!(exit_code);
                merge_traffic(&mut snapshot, entry.traffic.as_ref(), false);
                let _ = write_last_snapshot(&entry.root, &snapshot);
                console_lines = entry.logs.iter().map(|(_, line)| line.clone()).collect();
            }
        }
        // ограничиваем число сохранённых завершённых запусков
        let mut stopped: Vec<(String, f64)> = map
            .iter()
            .filter(|(_, entry)| entry.exit_code.is_some() && entry.launch_id != launch_id)
            .map(|(name, entry)| (name.clone(), entry.started_at))
            .collect();
        stopped.sort_by(|first, second| first.1.partial_cmp(&second.1).unwrap_or(std::cmp::Ordering::Equal));
        let remove_count = stopped.len().saturating_sub(31);
        for (name, _) in &stopped[..remove_count] {
            map.remove(name);
        }
    }

    if is_current {
        let _ = write_live(&root, false, 0, 0.0);
        let _ = save_console(&root, &console_lines, "");
    }

    let finished = {
        let map = lock_map(&PROCESSES);
        map.get(&key)
            .filter(|entry| entry.launch_id == launch_id)
            .map(|entry| entry.finished.clone())
    };
    {
        let mut map = lock_map(&PROCESSES);
        if let Some(entry) = map.get_mut(&key) {
            if entry.launch_id == launch_id {
                entry.process_lock = None;
            }
        }
    }
    if let Some(flag) = finished {
        flag.set();
    }

    if should_restart {
        thread::sleep(Duration::from_secs(1));
        let proceed = {
            let map = lock_map(&PROCESSES);
            map.get(&key)
                .map(|entry| entry.launch_id == launch_id && !entry.stop_requested)
                .unwrap_or(false)
        };
        if proceed {
            let _ = start_server(&key);
        }
    }
}

fn merge_traffic(target: &mut Value, traffic: Option<&super::server_traffic::TrafficHandle>, running: bool) {
    let Some(handle) = traffic else { return };
    let Ok(mut monitor) = handle.lock() else { return };
    let sample = monitor.sample(running);
    if let (Some(fields), Some(object)) = (sample.as_object(), target.as_object_mut()) {
        for (key, value) in fields {
            object.insert(key.clone(), value.clone());
        }
    }
}

struct EntrySnapshot {
    launch_id: String,
    running: bool,
    pid: u32,
    started_at: f64,
    players: i64,
    max_players: i64,
    tps: f64,
    exit_code: Option<i32>,
    metrics: MetricsCache,
    traffic: Option<super::server_traffic::TrafficHandle>,
}

pub(crate) fn status_server_with(config: &ConfigStore, instances: &InstanceStore, name: &str) -> Result<Value, String> {
    let info = server_instance(instances, name)?;
    let settings = server_config(config, instances, &info.name)?;
    let now = epoch_seconds();
    let previous = last_metrics(&info.path);

    let snapshot = {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(&info.name) {
            Some(entry) => Some(EntrySnapshot {
                launch_id: entry.launch_id.clone(),
                running: entry.is_running(),
                pid: entry.pid,
                started_at: entry.started_at,
                players: entry.players,
                max_players: entry.max_players,
                tps: entry.tps,
                exit_code: entry.exit_code,
                metrics: entry.metrics.clone(),
                traffic: entry.traffic.clone(),
            }),
            None => None,
        }
    };
    let entry_exists = snapshot.is_some();
    let entry_exit = snapshot.as_ref().and_then(|item| item.exit_code);

    let mut running = false;
    let mut pid = 0i64;
    let mut started_at = 0.0;
    let mut launch_id = String::new();
    let metrics = match snapshot {
        Some(item) if item.running => {
            running = true;
            pid = item.pid as i64;
            started_at = item.started_at;
            launch_id = item.launch_id.clone();
            // дорогие замеры выполняются вне блокировки, чтобы не задерживать логи
            let storage = directory_size(&info.path);
            let sample = process_tree_sample(item.pid as i32);
            let mut cache = item.metrics;
            let value = compute_metrics(
                &mut cache,
                item.pid,
                item.started_at,
                true,
                item.players,
                item.max_players,
                item.tps,
                sample,
                storage,
                now,
            );
            {
                let mut map = lock_map(&PROCESSES);
                if let Some(entry) = map.get_mut(&info.name) {
                    if entry.launch_id == item.launch_id {
                        entry.metrics = cache;
                    }
                }
            }
            let mut value = value;
            merge_traffic(&mut value, item.traffic.as_ref(), true);
            value
        }
        _ => {
            let mut metrics =
                stopped_metrics(&info.path, setting_i64(&settings, "max_players", 20), &previous, now);
            if let Some(item) = snapshot.as_ref() {
                merge_traffic(&mut metrics, item.traffic.as_ref(), false);
            }
            metrics
        }
    };

    let saved_error = {
        let errors = lock_map(&LAUNCH_ERRORS);
        match errors.get(&info.name) {
            Some(text) if !text.is_empty() => text.clone(),
            _ => saved_console(&info.path)
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        }
    };
    let exit_code = if entry_exists {
        entry_exit
    } else {
        previous.get("exit_code").and_then(Value::as_i64).map(|code| code as i32)
    };
    let players = metrics.get("players").cloned().unwrap_or(json!(0));
    let max_players = metrics
        .get("max_players")
        .cloned()
        .unwrap_or_else(|| json!(setting_i64(&settings, "max_players", 20)));
    let tps = metrics.get("tps").cloned().unwrap_or(json!(0.0));
    let uptime_seconds = metrics.get("uptime_seconds").cloned().unwrap_or(json!(0));
    let result = json!({
        "running": running,
        "pid": pid,
        "started_at": started_at,
        "start_error": if running { String::new() } else { saved_error },
        "exit_code": exit_code,
        "players": players,
        "max_players": max_players,
        "tps": tps,
        "uptime_seconds": uptime_seconds,
        "metrics": metrics,
        "last_metrics": previous.clone(),
    });
    if running {
        maybe_scheduled_restart(&info.name, &launch_id, settings.get("restart_schedule").cloned().unwrap_or(json!({})));
    }
    Ok(result)
}

fn stopped_metrics(root: &Path, max_players: i64, previous: &Value, now: f64) -> Value {
    json!({
        "timestamp": now,
        "pid": 0,
        "running": false,
        "uptime_seconds": 0,
        "ram_bytes": 0,
        "cpu_percent": 0.0,
        "storage_bytes": directory_size(root),
        "players": 0,
        "max_players": max_players,
        "tps": 0.0,
        "last": previous,
    })
}

fn compute_metrics(
    cache: &mut MetricsCache,
    pid: u32,
    started_at: f64,
    running: bool,
    players: i64,
    max_players: i64,
    tps: f64,
    sample: Option<(f64, u64)>,
    storage_bytes: u64,
    now: f64,
) -> Value {
    let mut cpu_percent = 0.0;
    let mut ram_bytes = 0u64;
    let mut alive = false;
    if let Some((cpu_seconds, ram)) = sample {
        alive = true;
        ram_bytes = ram;
        if let Some(previous) = cache.previous_cpu_seconds {
            let elapsed = (now - cache.previous_sample_at).max(0.05);
            cpu_percent = ((cpu_seconds - previous) / elapsed * 100.0).clamp(0.0, 999.0);
        }
        cache.previous_cpu_seconds = Some(cpu_seconds);
        cache.previous_sample_at = now;
    }
    cache.peak_ram_bytes = cache.peak_ram_bytes.max(ram_bytes);
    cache.peak_cpu_percent = cache.peak_cpu_percent.max(cpu_percent);
    let uptime_seconds = if started_at > 0.0 { (now - started_at).max(0.0) as i64 } else { 0 };
    json!({
        "timestamp": now,
        "pid": if alive { pid } else { 0 },
        "running": running && alive,
        "uptime_seconds": uptime_seconds,
        "peak_ram_bytes": cache.peak_ram_bytes,
        "peak_cpu_percent": (cache.peak_cpu_percent * 100.0).round() / 100.0,
        "ram_bytes": ram_bytes,
        "cpu_percent": (cpu_percent * 100.0).round() / 100.0,
        "storage_bytes": storage_bytes,
        "players": players.max(0),
        "max_players": max_players.max(0),
        "tps": (tps.max(0.0) * 100.0).round() / 100.0,
    })
}

fn maybe_scheduled_restart(key: &str, launch_id: &str, schedule: Value) {
    if !schedule.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }
    {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(key) {
            Some(entry) if entry.launch_id == launch_id && entry.scheduled_restart_date.is_empty() => {}
            _ => return,
        }
    }
    let Some((hour, minute)) = parse_schedule_time(schedule.get("time").and_then(Value::as_str).unwrap_or("04:00")) else {
        return;
    };
    let Some(now) = local_time() else { return };
    if (now.hour, now.minute) < (hour, minute) {
        return;
    }
    let today = format!("{:04}-{:02}-{:02}", now.year, now.month, now.day);
    if schedule.get("last_run").and_then(Value::as_str).unwrap_or("") == today {
        return;
    }
    let Some(scheduled_epoch) = today_epoch_at(hour, minute) else { return };
    let started_at = {
        let map = lock_map(&PROCESSES);
        map.get(key)
            .filter(|entry| entry.launch_id == launch_id)
            .map(|entry| entry.started_at)
            .unwrap_or(0.0)
    };
    // запуск после времени расписания не перезапускаем повторно
    if started_at <= 0.0 || started_at >= scheduled_epoch {
        return;
    }
    {
        let mut map = lock_map(&PROCESSES);
        if let Some(entry) = map.get_mut(key) {
            if entry.launch_id == launch_id {
                entry.scheduled_restart_date = today.clone();
            }
        }
    }
    let mut updated = schedule.clone();
    if let Some(fields) = updated.as_object_mut() {
        fields.insert("last_run".into(), json!(today));
    }
    persist_restart_schedule(key, &updated);
    let warn_minutes = schedule.get("warn_minutes").and_then(Value::as_i64).unwrap_or(1);
    let owned_key = key.to_owned();
    let owned_launch = launch_id.to_owned();
    thread::spawn(move || scheduled_restart_worker(owned_key, owned_launch, warn_minutes));
}

fn scheduled_restart_worker(key: String, launch_id: String, warn_minutes: i64) {
    // используем ASCII: кодировка консоли Windows не гарантирована
    let _ = server_command(&json!({
        "name": key,
        "command": format!("say Scheduled restart in {} min", warn_minutes.max(1)),
    }));
    let token = CancellationToken::new(&format!("server-restart-{launch_id}"));
    let _ = token.wait((warn_minutes.clamp(1, 30) * 60) as f64);
    token.cleanup();
    {
        let mut map = lock_map(&PROCESSES);
        match map.get_mut(&key) {
            Some(entry) => {
                if entry.launch_id != launch_id || !entry.is_running() {
                    return;
                }
            }
            None => return,
        }
    }
    let _ = restart_server(&key);
}

fn persist_restart_schedule(name: &str, schedule: &Value) {
    let Ok((config, instances)) = stores() else { return };
    let Ok(info) = server_instance(&instances, name) else { return };
    let Ok(mut values) = server_config(&config, &instances, name) else { return };
    if let Some(fields) = values.as_object_mut() {
        fields.insert("restart_schedule".into(), schedule.clone());
    }
    let _ = write_server_config(&info.path.join(SERVER_CONFIG_FILE), &values);
}

struct LocalTime {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
}

#[cfg(unix)]
fn local_time() -> Option<LocalTime> {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        Some(LocalTime {
            year: tm.tm_year as i64 + 1900,
            month: tm.tm_mon as i64 + 1,
            day: tm.tm_mday as i64,
            hour: tm.tm_hour as i64,
            minute: tm.tm_min as i64,
        })
    }
}

#[cfg(not(unix))]
fn local_time() -> Option<LocalTime> {
    None
}

#[cfg(unix)]
fn today_epoch_at(hour: i64, minute: i64) -> Option<f64> {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        tm.tm_hour = hour as libc::c_int;
        tm.tm_min = minute as libc::c_int;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        let stamp = libc::mktime(&mut tm);
        if stamp == -1 {
            None
        } else {
            Some(stamp as f64)
        }
    }
}

#[cfg(not(unix))]
fn today_epoch_at(_hour: i64, _minute: i64) -> Option<f64> {
    None
}

fn parse_schedule_time(value: &str) -> Option<(i64, i64)> {
    let (hours, minutes) = value.split_once(':')?;
    if hours.is_empty() || hours.len() > 2 || !hours.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if minutes.len() != 2 || !minutes.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((hours.parse().ok()?, minutes.parse().ok()?))
}

pub(crate) fn server_config(config: &ConfigStore, instances: &InstanceStore, name: &str) -> Result<Value, String> {
    let info = server_instance(instances, name)?;
    let path = info.path.join(SERVER_CONFIG_FILE);
    if is_symlink(&path) {
        return Err("Настройки сервера являются символьной ссылкой".to_owned());
    }
    let mut values = Map::new();
    values.insert("core".into(), json!("custom"));
    values.insert("version".into(), json!(info.minecraft_version));
    values.insert("build".into(), json!(""));
    values.insert("port".into(), json!(25565));
    values.insert("max_players".into(), json!(20));
    values.insert("motd".into(), json!(if info.display_name.is_empty() { info.name.clone() } else { info.display_name.clone() }));
    values.insert("memory".into(), json!("4GiB"));
    values.insert("java_path".into(), json!(""));
    values.insert("security_profile".into(), json!("strict"));
    values.insert("security_permissions".into(), json!(["network:game", "files:instance"]));
    values.insert("eula_accepted".into(), json!(false));
    values.insert("auto_restart".into(), json!(false));
    values.insert("restart_schedule".into(), json!({ "enabled": false, "time": "04:00", "warn_minutes": 1, "last_run": "" }));
    values.insert("snapshot_retention_hours".into(), json!(5));
    values.insert("snapshot_max_count".into(), json!(1));

    // защищённые настройки хранятся вне каталога экземпляра
    for (key, value) in load_protected_settings(config, &info.name)?.as_object().cloned().unwrap_or_default() {
        if let Some(slot) = values.get_mut(&key) {
            *slot = value;
        }
    }

    let existing = fs::symlink_metadata(&path).ok().filter(|meta| !meta.is_symlink());
    let mut rewrite = existing.is_none();
    if let Some(meta) = &existing {
        if meta.len() > MAX_SERVER_CONFIG_JSON_BYTES {
            return Err("Настройки сервера слишком большие".to_owned());
        }
        let text = fs::read_to_string(&path).map_err(|error| format!("Не удалось прочитать настройки сервера: {error}"))?;
        let raw: Value = serde_json::from_str(&text).map_err(|error| format!("Не удалось прочитать настройки сервера: {error}"))?;
        if let Some(fields) = raw.as_object() {
            let keys: Vec<String> = values.keys().cloned().collect();
            for key in keys {
                if SERVER_SECURITY_KEYS.contains(&key.as_str()) {
                    continue;
                }
                if let Some(stored) = fields.get(&key) {
                    values.insert(key, stored.clone());
                }
            }
            // переносим секреты из каталога экземпляра в защищённое хранилище
            rewrite = rewrite || SERVER_SECURITY_KEYS.iter().any(|key| fields.contains_key(*key));
        }
    }

    let original_memory = py_str(values.get("memory"), "4GiB").trim().to_owned();
    let normalized = normalize_server_config(&Value::Object(values), true)?;
    if py_str(normalized.get("memory"), "").trim() != original_memory {
        rewrite = true;
    }
    save_protected_settings(config, &info.name, &normalized)?;
    if rewrite {
        write_server_config(&path, &normalized)?;
    }
    Ok(normalized)
}

fn protected_settings_path(config: &ConfigStore, instance_name: &str) -> Result<PathBuf, String> {
    let path = config
        .paths
        .instance_security_dir
        .join(format!("{}.json", safe_instance_name(instance_name)));
    if is_symlink(&path) {
        return Err("Защищённые настройки экземпляра являются символьной ссылкой".to_owned());
    }
    Ok(path)
}

fn load_protected_settings(config: &ConfigStore, instance_name: &str) -> Result<Value, String> {
    let defaults = json!({
        "java_path": "",
        "security_profile": "strict",
        "security_permissions": ["network:game", "files:instance"],
    });
    let path = protected_settings_path(config, instance_name)?;
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(_) => {
            save_protected_settings(config, instance_name, &defaults)?;
            return Ok(defaults);
        }
    };
    if !meta.is_file() || meta.len() > MAX_PROTECTED_SETTINGS_BYTES {
        return Err("Защищённые настройки экземпляра повреждены".to_owned());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("Не удалось прочитать защищённые настройки экземпляра: {error}"))?;
    let raw: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Не удалось прочитать защищённые настройки экземпляра: {error}"))?;
    let Some(fields) = raw.as_object() else {
        return Err("Защищённые настройки экземпляра имеют неподдерживаемый формат".to_owned());
    };
    if fields.get("format").and_then(Value::as_i64) != Some(PROTECTED_SETTINGS_FORMAT) {
        return Err("Защищённые настройки экземпляра имеют неподдерживаемый формат".to_owned());
    }
    let mut values = defaults.as_object().cloned().unwrap_or_default();
    let keys: Vec<String> = values.keys().cloned().collect();
    for key in keys {
        if let Some(stored) = fields.get(&key) {
            values.insert(key, stored.clone());
        }
    }
    Ok(Value::Object(values))
}

pub(crate) fn save_protected_settings(config: &ConfigStore, instance_name: &str, settings: &Value) -> Result<(), String> {
    let path = protected_settings_path(config, instance_name)?;
    let mut payload = settings.as_object().cloned().unwrap_or_default();
    let keys: Vec<String> = payload.keys().cloned().collect();
    for key in keys {
        if !SERVER_SECURITY_KEYS.contains(&key.as_str()) {
            payload.remove(&key);
        }
    }
    payload.insert("format".into(), json!(PROTECTED_SETTINGS_FORMAT));
    atomic_json_write(&path, &Value::Object(payload), true)
}

pub(crate) fn write_server_config(path: &Path, values: &Value) -> Result<(), String> {
    let filtered: Map<String, Value> = values
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| !SERVER_SECURITY_KEYS.contains(&key.as_str()))
        .collect();
    atomic_json_write(path, &Value::Object(filtered), true)
}

pub(crate) fn server_memory_limit_kib(total_kib: Option<i64>) -> i64 {
    let total = total_kib.unwrap_or_else(system_memory_kib);
    if total <= 0 {
        return UNKNOWN_RAM_SERVER_LIMIT_KIB;
    }
    (total * 3 / 4).max(1)
}

pub(crate) fn normalize_server_config(value: &Value, clamp_memory: bool) -> Result<Value, String> {
    normalize_server_config_with_limit(value, clamp_memory, server_memory_limit_kib(None))
}

pub(crate) fn normalize_server_config_with_limit(value: &Value, clamp_memory: bool, memory_limit_kib: i64) -> Result<Value, String> {
    let mut core = py_str(value.get("core"), "custom").trim().to_lowercase();
    if core == "leaves" {
        core = "leaf".to_owned();
    }
    if !SERVER_CORE_IDS.contains(&core.as_str()) {
        core = "custom".to_owned();
    }
    let port = match py_int_field(value, "port", 25565) {
        Some(number) => number.clamp(1, 65535),
        None => return Err("Порт и количество игроков должны быть числами".to_owned()),
    };
    let max_players = match py_int_field(value, "max_players", 20) {
        Some(number) => number.clamp(1, 100000),
        None => return Err("Порт и количество игроков должны быть числами".to_owned()),
    };
    let memory_text = {
        let raw = py_str(value.get("memory"), "4GiB").trim().to_owned();
        if raw.is_empty() { "4GiB".to_owned() } else { raw }
    };
    let memory_kib = parse_memory_kib(&memory_text)?;
    // значение 4 GiB по умолчанию ужимается до лимита памяти
    let default_memory = memory_kib == 4 * 1024 * 1024;
    let memory = if memory_kib > memory_limit_kib {
        if !clamp_memory && !default_memory {
            return Err(format!(
                "Память JVM сервера не может превышать {} (75% физической памяти; остальное оставлено системе)",
                format_memory_kib(memory_limit_kib)
            ));
        }
        format_memory_kib(memory_limit_kib)
    } else {
        format_memory_kib(memory_kib)
    };
    java_memory_value(&memory)?;
    let retention = match py_int_field(value, "snapshot_retention_hours", 5) {
        Some(number) => number.clamp(1, 720),
        None => return Err("Параметры хранения снапшотов должны быть числами".to_owned()),
    };
    let max_count = match py_int_field(value, "snapshot_max_count", 1) {
        Some(number) => number.clamp(1, 50),
        None => return Err("Параметры хранения снапшотов должны быть числами".to_owned()),
    };
    let security_profile = py_str(value.get("security_profile"), "strict").trim().to_lowercase();
    if !matches!(security_profile.as_str(), "manual" | "strict" | "off") {
        return Err("Серверный security profile должен быть manual, strict или off".to_owned());
    }
    let mut permissions: Vec<String> = value
        .get("security_permissions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| py_str(Some(item), ""))
                .filter(|item| item == "network:game" || item == "files:instance")
                .collect()
        })
        .unwrap_or_default();
    if !permissions.iter().any(|item| item == "network:game") {
        permissions.push("network:game".to_owned());
    }
    if !permissions.iter().any(|item| item == "files:instance") {
        permissions.push("files:instance".to_owned());
    }
    Ok(json!({
        "core": core,
        "version": take_chars(py_str(value.get("version"), "").trim(), 32),
        "build": take_chars(py_str(value.get("build"), "").trim(), 32),
        "port": port,
        "max_players": max_players,
        "motd": take_chars(py_str(value.get("motd"), "").trim(), 256),
        "memory": memory,
        "java_path": take_chars(py_str(value.get("java_path"), "").trim(), 512),
        "security_profile": security_profile,
        "security_permissions": permissions,
        "eula_accepted": value.get("eula_accepted").and_then(Value::as_bool).unwrap_or(false),
        "auto_restart": value.get("auto_restart").and_then(Value::as_bool).unwrap_or(false),
        "restart_schedule": normalize_restart_schedule(value.get("restart_schedule")),
        "snapshot_retention_hours": retention,
        "snapshot_max_count": max_count,
    }))
}

fn normalize_restart_schedule(value: Option<&Value>) -> Value {
    let raw = value.filter(|value| value.is_object());
    let field = |key: &str, fallback: &str| {
        raw.and_then(|raw| raw.get(key))
            .map(|inner| py_str(Some(inner), fallback))
            .unwrap_or_else(|| fallback.to_owned())
    };
    let mut time_value = field("time", "04:00").trim().to_owned();
    static TIME: OnceLock<Regex> = OnceLock::new();
    let time_regex = TIME.get_or_init(|| Regex::new(r"^([01]?\d|2[0-3]):[0-5]\d\z").unwrap());
    if !time_regex.is_match(&time_value) {
        time_value = "04:00".to_owned();
    }
    let warn_minutes = raw
        .and_then(|raw| raw.get("warn_minutes"))
        .and_then(|inner| match inner {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.trim().parse::<i64>().ok(),
            Value::Bool(flag) => Some(if *flag { 1 } else { 0 }),
            _ => None,
        })
        .unwrap_or(1)
        .clamp(0, 30);
    let last_run = take_chars(&field("last_run", ""), 10);
    static DATE: OnceLock<Regex> = OnceLock::new();
    let date_regex = DATE.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}\z").unwrap());
    let last_run = if date_regex.is_match(&last_run) { last_run } else { String::new() };
    json!({
        "enabled": raw.and_then(|raw| raw.get("enabled")).and_then(Value::as_bool).unwrap_or(false),
        "time": time_value,
        "warn_minutes": warn_minutes,
        "last_run": last_run,
    })
}

pub(crate) fn server_java_requirement(version: &str) -> (i64, &'static str) {
    let value = version.trim();
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 2 && parts.len() != 3 {
        return (0, "");
    }
    if parts[0] != "1" {
        return (0, "");
    }
    let Ok(minor) = parts[1].parse::<i64>() else {
        return (0, "");
    };
    let patch = if parts.len() == 3 {
        match parts[2].parse::<i64>() {
            Ok(value) => value,
            Err(_) => return (0, ""),
        }
    } else {
        0
    };
    if minor > 20 || (minor == 20 && patch >= 5) {
        return (21, "java-runtime-delta");
    }
    if minor >= 18 {
        return (17, "java-runtime-gamma");
    }
    if minor == 17 {
        return (16, "java-runtime-alpha");
    }
    (8, "jre-legacy")
}

pub(crate) fn initial_server_heap(memory: &str) -> String {
    let bytes = memory.as_bytes();
    if bytes.len() < 2 {
        return memory.to_owned();
    }
    let (digits, unit) = bytes.split_at(bytes.len() - 1);
    let factor: u128 = match unit[0] {
        b'k' | b'K' => 1024,
        b'm' | b'M' => 1024 * 1024,
        b'g' | b'G' => 1024 * 1024 * 1024,
        _ => return memory.to_owned(),
    };
    let Ok(amount) = std::str::from_utf8(digits).unwrap_or("").parse::<u128>() else {
        return memory.to_owned();
    };
    if amount.saturating_mul(factor) >= 512u128 * 1024 * 1024 {
        "512M".to_owned()
    } else {
        memory.to_owned()
    }
}

fn which_java() -> Option<PathBuf> {
    let directories = std::env::var("PATH").ok()?;
    for directory in directories.split(':') {
        if directory.is_empty() {
            continue;
        }
        let candidate = Path::new(directory).join("java");
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn apply_server_port(root: &Path, port: i64, public_bind: bool) -> Result<(), String> {
    if !(1..=65535).contains(&port) {
        return Err("Порт сервера должен быть от 1 до 65535".to_owned());
    }
    let path = root.join(SERVER_PROPERTIES_FILE);
    let content: Vec<u8> = match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_symlink() && meta.is_file() => {
            if meta.len() > MAX_SERVER_PROPERTIES_BYTES {
                return Err("Файл server.properties недоступен или слишком большой".to_owned());
            }
            let bytes = fs::read(&path).map_err(|error| format!("Не удалось применить порт сервера: {error}"))?;
            if bytes.len() as u64 > MAX_SERVER_PROPERTIES_BYTES {
                return Err("Файл server.properties слишком большой".to_owned());
            }
            bytes
        }
        Ok(_) => return Err("Файл server.properties недоступен или слишком большой".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("Не удалось применить порт сервера: {error}")),
    };
    let text = String::from_utf8_lossy(&content).to_string();
    let updated = server_properties_with_port(&text, port, public_bind);
    if updated == text {
        return Ok(());
    }
    let temporary = root.join(format!(".{SERVER_PROPERTIES_FILE}.{}.part", unique_id()));
    if let Err(error) = write_private_file(&temporary, updated.as_bytes()) {
        return Err(format!("Не удалось применить порт сервера: {error}"));
    }
    match fs::rename(&temporary, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(format!("Не удалось применить порт сервера: {error}"))
        }
    }
}

fn write_private_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        file.write_all(content)
    }
    #[cfg(not(unix))]
    {
        let mut file = File::create(path)?;
        file.write_all(content)
    }
}

pub(crate) fn server_properties_with_port(content: &str, port: i64, public_bind: bool) -> String {
    static PORT_LINE: OnceLock<Regex> = OnceLock::new();
    let port_line = PORT_LINE.get_or_init(|| Regex::new(r"^([ \t]*server-port[ \t]*(?:=|:|[ \t]+)[ \t]*).*$").unwrap());
    let newline = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut updated: Vec<String> = Vec::new();
    let mut found = false;
    for line in split_keepends(content) {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        match port_line.captures(body) {
            Some(captures) => {
                updated.push(format!("{}{port}{ending}", &captures[1]));
                found = true;
            }
            None => updated.push(line.to_owned()),
        }
    }
    if !found {
        if let Some(last) = updated.last_mut() {
            if !last.ends_with('\n') && !last.ends_with('\r') {
                last.push_str(newline);
            }
        }
        updated.push(format!("server-port={port}{newline}"));
    }
    let mut result = updated.concat();
    if public_bind {
        // для публичного подключения очищаем только `server-ip`
        static IP_LINE: OnceLock<Regex> = OnceLock::new();
        let ip_line = IP_LINE.get_or_init(|| Regex::new(r"(?m)^[ \t]*server-ip[ \t]*(?:=|:|[ \t]+)[^\r\n]*").unwrap());
        let matched = ip_line.find_iter(&result).count();
        result = ip_line.replace_all(&result, "server-ip=").to_string();
        if matched == 0 {
            if !result.is_empty() && !result.ends_with('\n') && !result.ends_with('\r') {
                result.push_str(newline);
            }
            result.push_str("server-ip=");
            result.push_str(newline);
        }
    }
    result
}

fn split_keepends(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            parts.push(&text[start..=index]);
            start = index + 1;
        }
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

fn saved_console(root: &Path) -> Value {
    let path = root.join(CONSOLE_FILE);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return json!({});
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_CONSOLE_JSON_BYTES {
        return json!({});
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return json!({});
    };
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return json!({});
    };
    let Some(fields) = value.as_object_mut() else {
        return json!({});
    };
    if !fields.get("lines").map(|lines| lines.is_array()).unwrap_or(false) {
        return json!({});
    }
    fields.insert("cursor".into(), json!(-console_mtime_micros(&meta).max(1)));
    value
}

#[cfg(unix)]
fn console_mtime_micros(meta: &fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.mtime() * 1_000_000 + (meta.mtime_nsec() / 1000) as i64
}

#[cfg(not(unix))]
fn console_mtime_micros(_meta: &fs::Metadata) -> i64 {
    0
}

fn save_console(root: &Path, lines: &[String], error: &str) -> Result<(), String> {
    let selected = select_console_lines(lines);
    atomic_json_write(&root.join(CONSOLE_FILE), &json!({ "lines": selected, "error": error }), true)
}

fn select_console_lines(lines: &[String]) -> Vec<String> {
    // ограничиваем сохранённую консоль 800 строками и 256 КиБ
    let mut selected: Vec<String> = Vec::new();
    let mut remaining: i64 = 256 * 1024;
    let skip = lines.len().saturating_sub(800);
    for line in lines.iter().skip(skip).rev() {
        let text = tail_chars(line, 2048);
        let size = text.len() as i64 + 16;
        if size > remaining {
            break;
        }
        remaining -= size;
        selected.push(text);
    }
    selected.reverse();
    selected
}

pub(crate) fn server_is_running(root: &Path) -> bool {
    let Some(state) = read_bounded_json(&root.join(LIVE_STATE_FILE), MAX_LIVE_STATE_JSON_BYTES) else {
        return false;
    };
    if !state.is_object() || state.get("running").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let Some(pid) = state.get("pid").and_then(Value::as_i64).filter(|pid| *pid > 0) else {
        return false;
    };
    if !process_alive(pid) {
        return false;
    }
    #[cfg(unix)]
    {
        let proc_dir = PathBuf::from(format!("/proc/{pid}"));
        if proc_dir.is_dir() {
            match fs::read(proc_dir.join("cmdline")) {
                Ok(raw) => {
                    let text = String::from_utf8_lossy(&raw).replace('\u{0}', " ");
                    if !text.contains("server.jar") {
                        return false;
                    }
                }
                Err(_) => {}
            }
        }
    }
    true
}

fn process_alive(pid: i64) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

fn write_live(root: &Path, running: bool, pid: i64, started_at: f64) -> Result<(), String> {
    atomic_json_write(
        &root.join(LIVE_STATE_FILE),
        &json!({ "running": running, "pid": pid, "started_at": started_at }),
        true,
    )
}

fn last_metrics(root: &Path) -> Value {
    read_bounded_json(&root.join(METRICS_FILE), MAX_METRICS_JSON_BYTES)
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({}))
}

fn write_last_snapshot(root: &Path, snapshot: &Value) -> Result<(), String> {
    if is_symlink(root) || !root.is_dir() {
        return Err("Каталог метрик экземпляра недоступен".to_owned());
    }
    atomic_json_write(&root.join(METRICS_FILE), &json!({ "last": snapshot }), true)
}

#[cfg(unix)]
fn process_tree_sample(pid: i32) -> Option<(f64, u64)> {
    use std::collections::HashSet;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    if ticks <= 0.0 {
        return None;
    }
    let mut pending = vec![pid];
    let mut seen: HashSet<i32> = HashSet::new();
    let mut root_found = false;
    let mut cpu_seconds_total = 0.0;
    let mut ram_bytes_total = 0u64;
    while let Some(current) = pending.pop() {
        if seen.len() >= 1024 {
            break;
        }
        if current <= 0 || seen.contains(&current) {
            continue;
        }
        seen.insert(current);
        let proc = Path::new("/proc").join(current.to_string());
        let sample = single_process_sample(&proc, ticks);
        if current == pid {
            if sample.is_none() {
                return None;
            }
            root_found = true;
        }
        if let Some((cpu_seconds, ram_bytes)) = sample {
            cpu_seconds_total += cpu_seconds;
            ram_bytes_total += ram_bytes;
        }
        if let Ok(children) = fs::read_to_string(proc.join("task").join(current.to_string()).join("children")) {
            for value in children.split_whitespace() {
                if let Ok(child) = value.parse::<i32>() {
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
    let stat = fs::read_to_string(proc.join("stat")).ok()?;
    let after_parenthesis = stat.rsplit(')').next()?;
    let fields: Vec<&str> = after_parenthesis.split_whitespace().collect();
    // после имени процесса utime и stime находятся в полях 14 и 15
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    let statm = fs::read_to_string(proc.join("statm")).ok()?;
    let rss_pages: f64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f64;
    if ticks <= 0.0 || page <= 0.0 {
        return None;
    }
    Some(((utime + stime) / ticks, (rss_pages * page) as u64))
}

#[cfg(not(unix))]
fn process_tree_sample(_pid: i32) -> Option<(f64, u64)> {
    None
}

fn directory_size(root: &Path) -> u64 {
    const MAX_MEASURED_FILES: usize = 20_000;
    const EXCLUDED_PARTS: [&str; 3] = [".nivora-launcher-snapshots", "logs", "cache"];
    let mut total = 0u64;
    let mut measured = 0usize;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            if measured >= MAX_MEASURED_FILES {
                return total;
            }
            let path = entry.path();
            if is_symlink(&path) {
                continue;
            }
            if path
                .components()
                .any(|part| EXCLUDED_PARTS.contains(&part.as_os_str().to_string_lossy().as_ref()))
            {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                    measured += 1;
                }
            }
        }
    }
    total
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

fn unique_id() -> String {
    static COUNTER: Mutex<u64> = Mutex::new(0);
    let mut counter = lock_map(&COUNTER);
    *counter += 1;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() ^ (u64::from(value.subsec_nanos()) << 12))
        .unwrap_or(0);
    format!("{:x}-{}-{}", nanos, std::process::id(), *counter)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false)
}

fn tail_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_owned();
    }
    value.chars().skip(count - limit).collect()
}

fn take_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn py_str(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => fallback.to_owned(),
    }
}

fn py_int_field(value: &Value, key: &str, fallback: i64) -> Option<i64> {
    match value.get(key) {
        None => Some(fallback),
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        Some(Value::Bool(flag)) => Some(if *flag { 1 } else { 0 }),
        _ => None,
    }
}

fn setting_str(settings: &Value, key: &str) -> String {
    py_str(settings.get(key), "")
}

fn setting_i64(settings: &Value, key: &str, fallback: i64) -> i64 {
    settings.get(key).and_then(Value::as_i64).unwrap_or(fallback)
}

fn setting_bool(settings: &Value, key: &str) -> bool {
    settings.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn log_response(entries: &[(u64, String)], since: i64, cursor: i64, running: bool) -> Value {
    if since != 0 && !entries.is_empty() && entries[0].0 as i64 - 1 <= since && since <= cursor {
        let lines: Vec<&String> = entries
            .iter()
            .filter(|(sequence, _)| (*sequence as i64) > since)
            .map(|(_, line)| line)
            .collect();
        return json!({ "lines": lines, "running": running, "cursor": cursor, "reset": false });
    }
    let lines: Vec<&String> = entries.iter().map(|(_, line)| line).collect();
    json!({ "lines": lines, "running": running, "cursor": cursor, "reset": since != 0 })
}

const MAX_CORE_VERSIONS: usize = 200;

fn minecraft_version_key(version: &str) -> Option<Vec<u32>> {
    let mut parts = Vec::new();
    for part in version.split('.') {
        let value: u32 = part.parse().ok()?;
        parts.push(value);
    }
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    Some(parts)
}

fn unique_core_versions(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for value in values {
        let version = value.trim();
        if version.is_empty() || !seen.insert(version.to_owned()) {
            continue;
        }
        if minecraft_version_key(version).is_some() {
            result.push(version.to_owned());
        }
    }
    result.sort_by(|left, right| {
        let left_key = minecraft_version_key(left).unwrap_or_default();
        let right_key = minecraft_version_key(right).unwrap_or_default();
        right_key.cmp(&left_key)
    });
    result.truncate(MAX_CORE_VERSIONS);
    result
}

fn versions_from_payload(value: &Value) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if let Some(versions) = value.get("versions") {
        match versions {
            Value::Array(items) => {
                for item in items {
                    if let Some(text) = item.as_str() {
                        raw.push(text.to_owned());
                    }
                }
            }
            Value::Object(groups) => {
                for group in groups.values() {
                    if let Some(items) = group.as_array() {
                        for item in items {
                            if let Some(text) = item.as_str() {
                                raw.push(text.to_owned());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    raw
}

fn github_branch_versions(value: &Value, prefix: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let Some(branches) = value.as_array() else {
        return raw;
    };
    let pattern = format!(r"^{}(\d+(?:\.\d+){{1,2}})$", regex::escape(prefix));
    let Ok(re) = Regex::new(&pattern) else {
        return raw;
    };
    for branch in branches {
        let Some(name) = branch.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(captures) = re.captures(name) {
            raw.push(captures.get(1).map(|value| value.as_str().to_owned()).unwrap_or_default());
        }
    }
    raw
}

fn patina_release_versions(value: &Value) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let Ok(re) = Regex::new(r"\b\d+\.\d+(?:\.\d+)?\b") else {
        return raw;
    };
    let Some(releases) = value.as_array() else {
        return raw;
    };
    for release in releases {
        let tag = release.get("tag_name").and_then(Value::as_str).unwrap_or("");
        let name = release.get("name").and_then(Value::as_str).unwrap_or("");
        let text = format!("{tag} {name}");
        for capture in re.captures_iter(&text) {
            raw.push(capture.get(0).map(|value| value.as_str().to_owned()).unwrap_or_default());
        }
    }
    raw
}

fn mojang_release_versions(http: &std::sync::Arc<super::net::HttpClient>) -> Result<Vec<String>, String> {
    let value = http.get_json(super::constants::VERSION_MANIFEST_URL, None)?;
    let mut raw: Vec<String> = Vec::new();
    if let Some(versions) = value.get("versions").and_then(Value::as_array) {
        for item in versions {
            if item.get("type").and_then(Value::as_str) == Some("release") {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    raw.push(id.to_owned());
                }
            }
        }
    }
    Ok(unique_core_versions(&raw))
}

fn core_versions(http: &std::sync::Arc<super::net::HttpClient>, core: &str) -> Result<Vec<String>, String> {
    match core {
        "paper" | "folia" | "velocity" | "waterfall" => {
            let value = http.get_json(&format!("https://fill.papermc.io/v3/projects/{core}"), None)?;
            Ok(unique_core_versions(&versions_from_payload(&value)))
        }
        "purpur" => {
            let value = http.get_json("https://api.purpurmc.org/v2/purpur", None)?;
            Ok(unique_core_versions(&versions_from_payload(&value)))
        }
        "leaf" => {
            let value = http.get_json("https://api.leafmc.one/v2/projects/leaf", None)?;
            Ok(unique_core_versions(&versions_from_payload(&value)))
        }
        "pufferfish" => {
            let branches = http
                .get_json("https://api.github.com/repos/pufferfish-gg/Pufferfish/branches?per_page=100", None)
                .ok();
            if let Some(branches) = branches {
                let majors = github_branch_versions(&branches, "ver/");
                if !majors.is_empty() {
                    let releases = mojang_release_versions(http)?;
                    return Ok(releases
                        .into_iter()
                        .filter(|version| {
                            majors.iter().any(|major| version == major || version.starts_with(&format!("{major}.")))
                        })
                        .collect());
                }
            }
            mojang_release_versions(http)
        }
        "patina" => {
            if let Ok(branches) = http.get_json("https://api.github.com/repos/PatinaMC/Patina/branches?per_page=100", None) {
                let versions = github_branch_versions(&branches, "ver/");
                if !versions.is_empty() {
                    return Ok(unique_core_versions(&versions));
                }
            }
            if let Ok(releases) = http.get_json("https://api.github.com/repos/PatinaMC/Patina/releases?per_page=30", None) {
                let versions = patina_release_versions(&releases);
                if !versions.is_empty() {
                    return Ok(unique_core_versions(&versions));
                }
            }
            mojang_release_versions(http)
        }
        "spigot" => mojang_release_versions(http),
        _ => Ok(Vec::new()),
    }
}

/// возвращает доступные версии выбранного ядра
pub fn server_core_versions(request: &Value) -> Result<Value, String> {
    let core = request
        .get("core")
        .and_then(Value::as_str)
        .unwrap_or("custom")
        .trim()
        .to_lowercase();
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let config = ConfigStore::load()?;
    let http = super::net::client_from_settings(config.settings(), operation_id)?;
    Ok(json!({ "core": core, "versions": core_versions(&http, &core)? }))
}

fn safe_server_name(value: &str) -> Result<String, String> {
    let mut cleaned = String::with_capacity(value.len());
    let mut previous_invalid = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            cleaned.push(character);
            previous_invalid = false;
        } else if !previous_invalid {
            cleaned.push('-');
            previous_invalid = true;
        }
    }
    let cleaned = cleaned.trim_matches(|c| c == '.' || c == '-');
    if cleaned.is_empty() {
        return Err("Имя сервера не может быть пустым".to_owned());
    }
    Ok(cleaned.chars().take(64).collect())
}

fn create_server_instance(
    config: &ConfigStore,
    instances: &InstanceStore,
    name: &str,
    display_name: &str,
    core: &str,
    version: &str,
    port: i64,
    memory: &str,
    eula_accepted: bool,
    custom_jar_name: &str,
    custom_jar_data: Option<&[u8]>,
    replace_existing: bool,
) -> Result<InstanceInfo, String> {
    let safe_name = safe_server_name(name)?;
    let target = instances.root().join(&safe_name);
    if std::fs::symlink_metadata(&target).is_ok() {
        if target.symlink_metadata().map(|meta| meta.file_type().is_symlink()).unwrap_or(false) {
            return Err("Каталог сервера является символьной ссылкой".to_owned());
        }
        if replace_existing {
            return Err(
                "Нельзя безопасно заменить сервер во время создания: загрузка ядра выполняется отдельно. Выберите другое имя или удалите сервер отдельно после резервного копирования его мира."
                    .to_owned(),
            );
        }
        return Err(format!("Экземпляр уже существует: {safe_name}"));
    }
    std::fs::create_dir_all(&target).map_err(|error| format!("Не удалось создать каталог сервера: {error}"))?;
    for directory in ["plugins", "mods", "config", "world", "logs"] {
        std::fs::create_dir_all(target.join(directory))
            .map_err(|error| format!("Не удалось создать каталог сервера: {error}"))?;
    }
    let trimmed_display = display_name.trim().chars().take(80).collect::<String>();
    let mut profile = Map::new();
    profile.insert("display_name".to_owned(), json!(if trimmed_display.is_empty() { safe_name.clone() } else { trimmed_display }));
    profile.insert("description".to_owned(), json!("Серверный экземпляр"));
    profile.insert("group".to_owned(), json!(""));
    profile.insert("kind".to_owned(), json!("server"));
    profile.insert("played_seconds".to_owned(), json!(0));
    profile.insert("server_core".to_owned(), json!(core));
    profile.insert("server_core_version".to_owned(), json!(version));
    profile.insert("server_port".to_owned(), json!(port));
    profile.insert("server_eula".to_owned(), json!(eula_accepted));
    atomic_json_write(&target.join(super::instances::INSTANCE_METADATA_FILE), &Value::Object(profile), true)?;
    let memory_limit = server_memory_limit_kib(None);
    let server_config = normalize_server_config_with_limit(
        &json!({
            "core": core,
            "version": version,
            "port": port,
            "memory": memory,
            "motd": if display_name.trim().is_empty() { safe_name.clone() } else { display_name.trim().to_owned() },
            "eula_accepted": eula_accepted,
            "security_profile": "strict",
            "security_permissions": ["network:game", "files:instance"],
        }),
        true,
        memory_limit,
    )?;
    super::server_files::write_eula_file(&target, eula_accepted)?;
    write_server_config(&target.join(SERVER_CONFIG_FILE), &server_config)?;
    let mut protected = Map::new();
    for key in SERVER_SECURITY_KEYS {
        if let Some(value) = server_config.get(key) {
            protected.insert((*key).to_owned(), value.clone());
        }
    }
    save_protected_settings(config, &safe_name, &Value::Object(protected))?;
    if let Some(content) = custom_jar_data {
        write_server_core(config, &target, if custom_jar_name.trim().is_empty() { SERVER_JAR } else { custom_jar_name }, content)?;
    }
    instances.get(&safe_name)
}

fn write_server_core(config: &ConfigStore, root: &Path, file_name: &str, content: &[u8]) -> Result<PathBuf, String> {
    let instance_name = root.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut lock = InstanceProcessLock::new(&config.paths.data_home, &instance_name);
    lock.acquire()?;
    let result = (|| -> Result<PathBuf, String> {
        if server_is_running(root) {
            return Err("Остановите сервер перед заменой ядра".to_owned());
        }
        if content.len() > 512 * 1024 * 1024 {
            return Err("Файл ядра слишком большой (максимум 512 MiB)".to_owned());
        }
        let normalized = file_name.replace('\\', "/");
        let safe = normalized.rsplit('/').next().unwrap_or_default();
        if safe != normalized || !safe.to_lowercase().ends_with(".jar") {
            return Err("Ядро должно быть JAR-файлом без пути".to_owned());
        }
        if root.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Каталог ядра находится вне экземпляров".to_owned());
        }
        let destination = root.join(SERVER_JAR);
        if destination.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Файл ядра является символьной ссылкой".to_owned());
        }
        let temporary = root.join(format!(".{SERVER_JAR}.{}.part", super::mods::random_hex(16)));
        std::fs::write(&temporary, content).map_err(|error| format!("Не удалось записать ядро сервера: {error}"))?;
        std::fs::rename(&temporary, &destination).map_err(|error| format!("Не удалось записать ядро сервера: {error}"))?;
        Ok(destination)
    })();
    lock.release();
    result
}

const SERVER_SOURCE_HOSTS: &[&str] = &[
    "api.github.com",
    "api.leafmc.one",
    "api.papermc.io",
    "api.purpurmc.org",
    "ci.pufferfish.host",
    "fill-data.papermc.io",
    "fill.papermc.io",
    "github.com",
    "hub.spigotmc.org",
    "objects.githubusercontent.com",
    "piston-meta.mojang.com",
    "raw.githubusercontent.com",
];

fn validate_server_source_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| "Некорректный адрес источника ядра".to_owned())?;
    let host = parsed.host_str().unwrap_or_default().to_lowercase().trim_end_matches('.').to_owned();
    if !SERVER_SOURCE_HOSTS.contains(&host.as_str()) {
        return Err(format!("Источник ядра не входит в официальный allowlist: {}", if host.is_empty() { "неизвестный хост".to_owned() } else { host.clone() }));
    }
    Ok(())
}

fn optional_i64_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|item| item.as_i64().or_else(|| item.as_str().and_then(|text| text.parse().ok())))
}

fn patina_download(http: &std::sync::Arc<super::net::HttpClient>, version: &str, build: &str) -> Result<(String, String), String> {
    let repository = http.get_json("https://api.github.com/repos/PatinaMC/Patina", None)?;
    let mut branch = repository
        .get("default_branch")
        .and_then(Value::as_str)
        .unwrap_or("ver/1.21.1")
        .to_owned();
    if branch.is_empty() || branch.len() > 80 || !branch.chars().all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '/' | '-')) {
        branch = "ver/1.21.1".to_owned();
    }
    let readme_url = format!("https://raw.githubusercontent.com/PatinaMC/Patina/{branch}/README.md");
    let validator = |target: &str| validate_server_source_url(target);
    let raw_bytes = http.request(&readme_url, "GET", None, None, None, Some(&validator))?;
    let raw = String::from_utf8_lossy(&raw_bytes).to_string();
    let mut links: Vec<String> = Vec::new();
    let mut rest = raw.as_str();
    while let Some(start) = rest.find("](https://github.com/PatinaMC/Patina/raw/") {
        let tail = &rest[start + 2..];
        let Some(end) = tail.find(')') else { break };
        links.push(tail[..end].to_owned());
        rest = &tail[end..];
    }
    let major = version.split('.').take(2).collect::<Vec<_>>().join(".");
    let mut selected = links.iter().find(|link| link.contains(&format!("/{version}/"))).cloned().unwrap_or_default();
    if selected.is_empty() {
        selected = links.iter().find(|link| link.contains(&format!("/{major}/"))).cloned().unwrap_or_default();
    }
    if selected.is_empty() {
        return Err(format!("Patina не опубликовала официальный JAR для {version}"));
    }
    Ok((selected, if build.is_empty() { version.to_owned() } else { build.to_owned() }))
}

fn build_spigot_with_buildtools(
    http: &std::sync::Arc<super::net::HttpClient>,
    root: &Path,
    version: &str,
    confirm_unverified: bool,
) -> Result<Value, String> {
    if !confirm_unverified {
        return Err("[unverified-download] Spigot собирается официальным BuildTools без доступного SHA-256. Подтвердите скачивание явно.".to_owned());
    }
    let java = which_java().ok_or_else(|| "Для сборки Spigot нужен Java executable".to_owned())?;
    let buildtools_url = "https://hub.spigotmc.org/jenkins/job/BuildTools/lastSuccessfulBuild/artifact/target/BuildTools.jar";
    validate_server_source_url(buildtools_url)?;
    let directory = root.join(format!(".nivora-spigot-build-{}", super::mods::random_hex(8)));
    std::fs::create_dir_all(&directory).map_err(|error| format!("Не удалось запустить официальный Spigot BuildTools: {error}"))?;
    let result = (|| -> Result<Value, String> {
        let buildtools = directory.join("BuildTools.jar");
        let validator = |target: &str| validate_server_source_url(target);
        http.download(buildtools_url, &buildtools, None, None, None, None, None, None, Some(&validator))
            .map_err(|_| "Не удалось запустить официальный Spigot BuildTools".to_owned())?;
        let mut command = super::platform::command(&java);
        command
            .args(["-jar", "BuildTools.jar", "--rev", version])
            .current_dir(&directory)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (name, value) in super::runtime::safe_process_environment(false) {
            command.env(name, value);
        }
        let mut child = command.spawn().map_err(|_| "Не удалось запустить официальный Spigot BuildTools".to_owned())?;
        // ограничиваем сборку 30 минутами
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1800);
        let status = loop {
            match child.try_wait().map_err(|_| "Не удалось запустить официальный Spigot BuildTools".to_owned())? {
                Some(status) => break status,
                None if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    return Err("Не удалось запустить официальный Spigot BuildTools".to_owned());
                }
                None => std::thread::sleep(std::time::Duration::from_millis(500)),
            }
        };
        if !status.success() {
            return Err("BuildTools не собрал Spigot".to_owned());
        }
        let source = directory.join(format!("spigot-{version}.jar"));
        if !source.is_file() || source.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("BuildTools завершился без spigot JAR".to_owned());
        }
        let content = std::fs::read(&source).map_err(|_| "BuildTools не собрал Spigot".to_owned())?;
        let destination = root.join(SERVER_JAR);
        if destination.symlink_metadata().map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Файл server.jar является символьной ссылкой".to_owned());
        }
        let temporary = root.join(format!(".{SERVER_JAR}.{}.part", super::mods::random_hex(16)));
        std::fs::write(&temporary, &content).map_err(|_| "BuildTools не собрал Spigot".to_owned())?;
        std::fs::rename(&temporary, &destination).map_err(|_| "BuildTools не собрал Spigot".to_owned())?;
        Ok(json!({
            "core": "spigot",
            "version": version,
            "build": "buildtools",
            "path": root.join(SERVER_JAR).to_string_lossy(),
            "url": buildtools_url,
            "sha256": super::net::file_hash(&root.join(SERVER_JAR), "sha256").unwrap_or_default(),
            "verified_by": "spigot-buildtools",
            "trusted_hash": false,
        }))
    })();
    let _ = std::fs::remove_dir_all(&directory);
    result
}

fn download_server_core(
    http: &std::sync::Arc<super::net::HttpClient>,
    root: &Path,
    core: &str,
    version: &str,
    build: &str,
    confirm_unverified: bool,
) -> Result<Value, String> {
    if server_is_running(root) {
        return Err("Остановите сервер перед заменой ядра".to_owned());
    }
    let core_id = core.trim().to_lowercase();
    let minecraft_version = version.trim();
    if minecraft_version.is_empty()
        || minecraft_version.len() > 32
        || !minecraft_version.chars().all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
    {
        return Err("Укажите корректную версию Minecraft для ядра".to_owned());
    }
    let mut expected_sha256: Option<String> = None;
    let mut expected_size: Option<u64> = None;
    let mut verified_by = String::new();
    let url: String;
    let resolved_build: String;
    match core_id.as_str() {
        "purpur" => {
            url = format!("https://api.purpurmc.org/v2/purpur/{minecraft_version}/latest/download");
            resolved_build = "latest".to_owned();
        }
        "paper" | "folia" | "velocity" | "waterfall" => {
            let project = core_id.as_str();
            let builds_value = http.get_json(
                &format!("https://fill.papermc.io/v3/projects/{project}/versions/{minecraft_version}/builds"),
                None,
            )?;
            let (builds, v3) = if let Some(rows) = builds_value.as_array() {
                (rows.clone(), true)
            } else if let Some(rows) = builds_value.get("builds").and_then(Value::as_array) {
                (rows.clone(), false)
            } else {
                (Vec::new(), false)
            };
            if builds.is_empty() {
                return Err(format!("PaperMC не нашёл сборки {project} для {minecraft_version}"));
            }
            let build_of = |item: &Value| -> String {
                item.get("id")
                    .or_else(|| item.get("build"))
                    .map(|value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))
                    .unwrap_or_default()
            };
            let mut selected = if build.is_empty() {
                None
            } else {
                builds.iter().find(|item| build_of(item) == build).cloned()
            };
            if selected.is_none() {
                selected = builds
                    .iter()
                    .max_by_key(|item| item.get("id").or_else(|| item.get("build")).and_then(Value::as_i64).unwrap_or(0))
                    .cloned();
            }
            let selected = selected.unwrap_or_default();
            resolved_build = build_of(&selected);
            if resolved_build.is_empty() || !resolved_build.chars().all(|char| char.is_ascii_digit()) {
                return Err("PaperMC вернул некорректный номер сборки".to_owned());
            }
            if v3 {
                let download = selected
                    .pointer("/downloads/server:default")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                url = download.get("url").and_then(Value::as_str).unwrap_or_default().to_owned();
                expected_sha256 = download
                    .pointer("/checksums/sha256")
                    .and_then(Value::as_str)
                    .map(|value| value.to_lowercase())
                    .filter(|value| !value.is_empty());
            } else {
                let file_name = format!("{project}-{minecraft_version}-{resolved_build}.jar");
                url = format!("https://api.papermc.io/v2/projects/{project}/versions/{minecraft_version}/builds/{resolved_build}/downloads/{file_name}");
                expected_sha256 = selected
                    .pointer("/downloads/application/sha256")
                    .and_then(Value::as_str)
                    .map(|value| value.to_lowercase())
                    .filter(|value| !value.is_empty());
            }
            if let Some(size) = optional_i64_field(&selected, "size").or_else(|| selected.pointer("/downloads/server:default").and_then(|value| optional_i64_field(value, "size"))) {
                if size > 0 {
                    expected_size = Some(size as u64);
                }
            }
            if !expected_sha256.is_none() {
                verified_by = "papermc-api".to_owned();
            }
            if url.is_empty() {
                return Err(format!("PaperMC не вернул ссылку на сборку {project}"));
            }
        }
        "leaf" => {
            let builds_value = http.get_json(
                &format!("https://api.leafmc.one/v2/projects/leaf/versions/{minecraft_version}/builds"),
                None,
            )?;
            let builds = builds_value
                .get("builds")
                .and_then(Value::as_array)
                .cloned()
                .or_else(|| builds_value.as_array().cloned())
                .unwrap_or_default();
            if builds.is_empty() {
                return Err(format!("Leaf не нашёл сборки для {minecraft_version}"));
            }
            let build_of = |item: &Value| -> String {
                item.get("build").map(|value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())).unwrap_or_default()
            };
            let mut selected = if build.is_empty() {
                None
            } else {
                builds.iter().find(|item| build_of(item) == build).cloned()
            };
            if selected.is_none() {
                selected = builds
                    .iter()
                    .max_by_key(|item| item.get("build").and_then(Value::as_i64).unwrap_or(0))
                    .cloned();
            }
            let selected = selected.unwrap_or_default();
            resolved_build = build_of(&selected);
            if resolved_build.is_empty() || !resolved_build.chars().all(|char| char.is_ascii_digit()) {
                return Err("Leaf вернул некорректный номер сборки".to_owned());
            }
            let file_name = format!("leaf-{minecraft_version}-{resolved_build}.jar");
            url = format!("https://api.leafmc.one/v2/projects/leaf/versions/{minecraft_version}/builds/{resolved_build}/downloads/{file_name}");
            let download = selected.pointer("/downloads/application").cloned().unwrap_or_else(|| json!({}));
            expected_sha256 = download
                .get("sha256")
                .and_then(Value::as_str)
                .map(|value| value.to_lowercase())
                .filter(|value| !value.is_empty());
            if let Some(size) = optional_i64_field(&download, "size") {
                expected_size = Some(size as u64);
            }
            if !expected_sha256.is_none() {
                verified_by = "leaf-api".to_owned();
            }
        }
        "pufferfish" => {
            let branch = minecraft_version.split('.').take(2).collect::<Vec<_>>().join(".");
            let build_info = http.get_json(
                &format!("https://ci.pufferfish.host/job/Pufferfish-{branch}/lastSuccessfulBuild/api/json"),
                None,
            )?;
            let artifacts = build_info.get("artifacts").and_then(Value::as_array).cloned().unwrap_or_default();
            let jars: Vec<&Value> = artifacts
                .iter()
                .filter(|item| {
                    item.get("fileName")
                        .and_then(Value::as_str)
                        .map(|name| name.to_lowercase().ends_with(".jar"))
                        .unwrap_or(false)
                })
                .collect();
            let artifact = jars
                .iter()
                .find(|item| {
                    let name = item.get("fileName").and_then(Value::as_str).unwrap_or_default().to_lowercase();
                    (name.contains("paperclip") || name.contains("pufferfish") || name.contains("server"))
                        && !(name.contains("api") || name.contains("sources") || name.contains("javadoc"))
                })
                .copied()
                .or_else(|| jars.first().copied());
            let build_url = build_info
                .get("url")
                .and_then(Value::as_str)
                .map(|value| value.trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("https://ci.pufferfish.host/job/Pufferfish-{branch}/lastSuccessfulBuild"));
            let relative_path = artifact
                .and_then(|item| item.get("relativePath"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_owned();
            if relative_path.is_empty() {
                return Err(format!("Pufferfish не нашёл JAR для {minecraft_version}"));
            }
            url = format!("{build_url}/artifact/{}", relative_path.trim_start_matches('/'));
            resolved_build = build_info
                .get("number")
                .map(|value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))
                .unwrap_or_else(|| "latest".to_owned());
        }
        "patina" => {
            let (selected, resolved) = patina_download(http, minecraft_version, build)?;
            url = selected;
            resolved_build = resolved;
        }
        "spigot" => {
            return build_spigot_with_buildtools(http, root, minecraft_version, confirm_unverified);
        }
        _ => return Err("Для этого ядра загрузка не автоматизирована. Загрузите совместимый server.jar вручную.".to_owned()),
    }
    validate_server_source_url(&url)?;
    if expected_sha256.is_none() && !confirm_unverified {
        return Err("[unverified-download] Серверный источник не предоставил доверенный хеш. Подтвердите скачивание явно.".to_owned());
    }
    let destination = root.join(SERVER_JAR);
    let validator = |target: &str| validate_server_source_url(target);
    http.download(
        &url,
        &destination,
        None,
        expected_sha256.as_deref(),
        None,
        expected_size,
        None,
        None,
        Some(&validator),
    )?;
    let actual_sha256 = super::net::file_hash(&destination, "sha256").unwrap_or_default();
    Ok(json!({
        "core": core_id,
        "version": minecraft_version,
        "build": resolved_build,
        "path": destination.to_string_lossy(),
        "url": url,
        "sha256": actual_sha256,
        "verified_by": verified_by,
        "trusted_hash": expected_sha256.is_some(),
    }))
}

pub fn server_create(request: &Value) -> Result<Value, String> {
    use base64::Engine;
    let (config, instances) = stores()?;
    let custom_jar_name = request.get("custom_jar_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let custom_jar_data = match request.get("custom_jar_data") {
        Some(Value::String(encoded)) if !encoded.is_empty() => {
            if encoded.len() > 4 * ((512 * 1024 * 1024 + 2) / 3) {
                return Err("Файл ядра слишком большой".to_owned());
            }
            Some(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| "Файл ядра повреждён".to_owned())?,
            )
        }
        _ => None,
    };
    let info = create_server_instance(
        &config,
        &instances,
        request.get("name").and_then(Value::as_str).unwrap_or_default(),
        request.get("display_name").and_then(Value::as_str).unwrap_or_default(),
        request.get("core").and_then(Value::as_str).unwrap_or("custom"),
        request.get("version").and_then(Value::as_str).unwrap_or_default(),
        request.get("port").and_then(Value::as_i64).unwrap_or(25565),
        request.get("memory").and_then(Value::as_str).unwrap_or("4GiB"),
        request.get("eula_accepted").and_then(Value::as_bool).unwrap_or(false),
        &custom_jar_name,
        custom_jar_data.as_deref(),
        request.get("replace_existing").and_then(Value::as_bool).unwrap_or(false),
    )?;
    let display = if info.display_name.is_empty() { info.name.clone() } else { info.display_name.clone() };
    super::events::emit_status_event("server-created", &format!("Сервер «{display}» создан."));
    let config_value = server_config(&config, &instances, &info.name)?;
    Ok(json!({
        "server": super::server_files::server_public(&info, &config_value),
        "instances": instances.list().iter().map(|item| item.public()).collect::<Vec<Value>>(),
    }))
}

pub fn server_upload_core(request: &Value) -> Result<Value, String> {
    use base64::Engine;
    let (config, instances) = stores()?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let info = server_instance(&instances, name)?;
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or_default();
    if encoded.len() > 4 * ((512 * 1024 * 1024 + 2) / 3) {
        return Err("Файл ядра слишком большой".to_owned());
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Файл ядра повреждён".to_owned())?;
    let file_name = request.get("file_name").and_then(Value::as_str).unwrap_or(SERVER_JAR).trim().to_owned();
    let path = write_server_core(&config, &info.path, &file_name, &content)?;
    super::events::emit_status_event("server-core-uploaded", "Ядро сервера загружено.");
    let config_value = server_config(&config, &instances, name)?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "server": super::server_files::server_public(&info, &config_value),
    }))
}

pub fn server_core_download(request: &Value) -> Result<Value, String> {
    let (config, instances) = stores()?;
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let info = server_instance(&instances, name)?;
    let settings = server_config(&config, &instances, name)?;
    let core = request
        .get("core")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| settings.get("core").and_then(Value::as_str).unwrap_or_default().to_owned())
        .trim()
        .to_lowercase();
    let version = request
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| settings.get("version").and_then(Value::as_str).unwrap_or_default().to_owned())
        .trim()
        .to_owned();
    let build = request
        .get("build")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| settings.get("build").and_then(Value::as_str).unwrap_or_default().to_owned())
        .trim()
        .to_owned();
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or("");
    let http = super::net::client_from_settings(config.settings(), operation_id)?;
    let mut lock = InstanceProcessLock::new(&config.paths.data_home, &info.name);
    lock.acquire()?;
    let result = download_server_core(
        &http,
        &info.path,
        &core,
        &version,
        &build,
        request.get("confirm_unverified").and_then(Value::as_bool).unwrap_or(false),
    );
    lock.release();
    let download = result?;
    let save_request = json!({
        "name": name,
        "values": {
            "core": core,
            "version": version,
            "build": download.get("build").cloned().unwrap_or(Value::Null),
        }
    });
    let _ = super::server_files::server_save_config(&save_request)?;
    super::events::emit_status_event("server-core-downloaded", "Рекомендованное ядро загружено.");
    let config_value = server_config(&config, &instances, name)?;
    Ok(json!({
        "download": download,
        "server": super::server_files::server_public(&info, &config_value),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entry() -> ServerProcess {
        ServerProcess {
            launch_id: "test".to_owned(),
            root: PathBuf::from("/tmp"),
            pid: 0,
            child: None,
            stdin: None,
            started_at: 0.0,
            logs: VecDeque::new(),
            log_sequence: 0,
            players: 0,
            max_players: 0,
            tps: 0.0,
            exit_code: None,
            stop_requested: false,
            finished: Arc::new(FinishFlag::new()),
            process_lock: None,
            scheduled_restart_date: String::new(),
            metrics: MetricsCache::default(),
            traffic: None,
        }
    }

    #[test]
    fn server_commands_reject_multiline_input() {
        assert_eq!(validate_server_command("/say hello").unwrap(), "/say hello");
        assert!(validate_server_command("/say hello\n/stop").is_err());
        assert!(validate_server_command("/say hello\r\n/stop").is_err());
    }

    #[test]
    fn finalization_waits_for_every_output_stream_to_drain() {
        use std::sync::mpsc;

        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let stdout = thread::spawn(|| {});
        let stderr = thread::spawn(move || {
            release_rx.recv().unwrap();
        });
        let coordinator = thread::spawn(move || {
            after_output_readers_finish(vec![stdout, stderr], || finished_tx.send(()).unwrap());
        });

        assert!(finished_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        coordinator.join().unwrap();
    }

    #[test]
    fn core_versions_sort_and_filter_like_python() {
        let versions = unique_core_versions(&[
            "1.20.4".to_owned(),
            "1.21".to_owned(),
            "1.20.4-rc1".to_owned(),
            "1.20.4".to_owned(),
            "1.20".to_owned(),
            "мусор".to_owned(),
        ]);
        assert_eq!(versions, vec!["1.21", "1.20.4", "1.20"]);
        assert_eq!(minecraft_version_key("1.20.4"), Some(vec![1, 20, 4]));
        assert_eq!(minecraft_version_key("24w14a"), None);
        assert_eq!(minecraft_version_key("1.20.4.5"), None);
    }

    #[test]
    fn github_branch_versions_extract_supported_majors() {
        let branches = json!([
            { "name": "ver/1.21.4" },
            { "name": "ver/1.20" },
            { "name": "main" },
            { "name": "ver/abc" }
        ]);
        let mut versions = github_branch_versions(&branches, "ver/");
        versions.sort();
        assert_eq!(versions, vec!["1.20".to_owned(), "1.21.4".to_owned()]);
    }

    #[test]
    fn normalizes_core_ids() {
        let value = normalize_server_config(&json!({ "core": "PAPER" }), true).unwrap();
        assert_eq!(value["core"], "paper");
        let value = normalize_server_config(&json!({ "core": "leaves" }), true).unwrap();
        assert_eq!(value["core"], "leaf");
        let value = normalize_server_config(&json!({ "core": "unknown-core" }), true).unwrap();
        assert_eq!(value["core"], "custom");
        let value = normalize_server_config(&json!({}), true).unwrap();
        assert_eq!(value["core"], "custom");
        assert_eq!(value["port"], 25565);
        let expected_memory = format_memory_kib(parse_memory_kib("4GiB").unwrap().min(server_memory_limit_kib(None)));
        assert_eq!(value["memory"], expected_memory);
    }

    #[test]
    fn clamps_port_and_players() {
        let value = normalize_server_config(&json!({ "port": 0, "max_players": 200000 }), true).unwrap();
        assert_eq!(value["port"], 1);
        assert_eq!(value["max_players"], 100000);
        let value = normalize_server_config(&json!({ "port": 70000 }), true).unwrap();
        assert_eq!(value["port"], 65535);
        let error = normalize_server_config(&json!({ "port": "abc" }), true).unwrap_err();
        assert_eq!(error, "Порт и количество игроков должны быть числами");
        let error = normalize_server_config(&json!({ "max_players": null }), true).unwrap_err();
        assert_eq!(error, "Порт и количество игроков должны быть числами");
    }

    #[test]
    fn clamps_memory_to_limit() {
        let value = normalize_server_config(&json!({ "memory": "1048576GiB" }), true).unwrap();
        assert_eq!(value["memory"], format_memory_kib(server_memory_limit_kib(None)));
        let error = normalize_server_config(&json!({ "memory": "1048576GiB" }), false).unwrap_err();
        assert!(error.starts_with("Память JVM сервера не может превышать"));
        let value = normalize_server_config(&json!({ "memory": "512MiB" }), true).unwrap();
        assert_eq!(value["memory"], "512MiB");
        let error = normalize_server_config(&json!({ "memory": "abc" }), true).unwrap_err();
        assert_eq!(error, "Память JVM должна быть числом с единицей KiB, MiB или GiB");
    }

    #[test]
    fn default_memory_survives_low_ram_even_without_clamp() {
        let limit = server_memory_limit_kib(Some(4 * 1024 * 1024));
        for memory in ["", "4GiB", "4G", "4096MiB", "4.0GiB"] {
            let value = normalize_server_config_with_limit(&json!({ "memory": memory }), false, limit).unwrap();
            assert_eq!(value["memory"], "3GiB", "memory={memory:?}");
        }
        let error = normalize_server_config_with_limit(&json!({ "memory": "6GiB" }), false, limit).unwrap_err();
        assert!(error.contains("75% физической памяти"));
        let value = normalize_server_config_with_limit(&json!({ "memory": "2GiB" }), false, limit).unwrap();
        assert_eq!(value["memory"], "2GiB");
    }

    #[test]
    fn validates_security_profile() {
        let error = normalize_server_config(&json!({ "security_profile": "paranoid" }), true).unwrap_err();
        assert_eq!(error, "Серверный security profile должен быть manual, strict или off");
        let value = normalize_server_config(&json!({ "security_profile": "STRICT" }), true).unwrap();
        assert_eq!(value["security_profile"], "strict");
    }

    #[test]
    fn appends_required_permissions() {
        let value = normalize_server_config(&json!({ "security_permissions": ["bogus", "network:game"] }), true).unwrap();
        assert_eq!(value["security_permissions"], json!(["network:game", "files:instance"]));
        let value = normalize_server_config(&json!({ "security_permissions": [] }), true).unwrap();
        assert_eq!(value["security_permissions"], json!(["network:game", "files:instance"]));
    }

    #[test]
    fn truncates_long_fields() {
        let long_version = "1.21.1-with-very-long-suffix-beyond-limit";
        let value = normalize_server_config(&json!({ "version": long_version }), true).unwrap();
        assert_eq!(value["version"].as_str().unwrap().chars().count(), 32);
    }

    #[test]
    fn normalizes_restart_schedule() {
        let value = normalize_restart_schedule(Some(&json!({ "enabled": true, "time": "25:99", "warn_minutes": 100, "last_run": "2026-1-1" })));
        assert_eq!(value["time"], "04:00");
        assert_eq!(value["warn_minutes"], 30);
        assert_eq!(value["last_run"], "");
        let value = normalize_restart_schedule(Some(&json!({ "time": "23:59", "warn_minutes": -5, "last_run": "2026-10-13" })));
        assert_eq!(value["time"], "23:59");
        assert_eq!(value["warn_minutes"], 0);
        assert_eq!(value["last_run"], "2026-10-13");
        let value = normalize_restart_schedule(None);
        assert_eq!(value["time"], "04:00");
        assert_eq!(value["warn_minutes"], 1);
        assert_eq!(value["enabled"], false);
    }

    #[test]
    fn lax_schedule_time_matches_python() {
        assert_eq!(parse_schedule_time("25:99"), Some((25, 99)));
        assert_eq!(parse_schedule_time("4:00"), Some((4, 0)));
        assert_eq!(parse_schedule_time("04:00"), Some((4, 0)));
        assert_eq!(parse_schedule_time("4:0"), None);
        assert_eq!(parse_schedule_time("abc"), None);
    }

    #[test]
    fn java_requirement_matrix() {
        assert_eq!(server_java_requirement("1.21.5"), (21, "java-runtime-delta"));
        assert_eq!(server_java_requirement("1.20.5"), (21, "java-runtime-delta"));
        assert_eq!(server_java_requirement("1.21"), (21, "java-runtime-delta"));
        assert_eq!(server_java_requirement("1.20.4"), (17, "java-runtime-gamma"));
        assert_eq!(server_java_requirement("1.18.2"), (17, "java-runtime-gamma"));
        assert_eq!(server_java_requirement("1.17"), (16, "java-runtime-alpha"));
        assert_eq!(server_java_requirement("1.16.5"), (8, "jre-legacy"));
        assert_eq!(server_java_requirement("1.8"), (8, "jre-legacy"));
        assert_eq!(server_java_requirement("2.1"), (0, ""));
        assert_eq!(server_java_requirement("1."), (0, ""));
        assert_eq!(server_java_requirement("1.2.3.4"), (0, ""));
        assert_eq!(server_java_requirement(""), (0, ""));
    }

    #[test]
    fn initial_heap_for_common_values() {
        assert_eq!(initial_server_heap("4G"), "512M");
        assert_eq!(initial_server_heap("1G"), "512M");
        assert_eq!(initial_server_heap("512M"), "512M");
        assert_eq!(initial_server_heap("511M"), "511M");
        assert_eq!(initial_server_heap("256M"), "256M");
        assert_eq!(initial_server_heap("1024K"), "1024K");
        assert_eq!(initial_server_heap("4GiB"), "4GiB");
        assert_eq!(initial_server_heap("auto"), "auto");
    }

    #[test]
    fn properties_replaces_port_line() {
        assert_eq!(
            server_properties_with_port("server-port=25565\nmotd=hi\n", 25599, false),
            "server-port=25599\nmotd=hi\n"
        );
        assert_eq!(
            server_properties_with_port("# comment\nserver-port: 25565\n", 25599, false),
            "# comment\nserver-port: 25599\n"
        );
        assert_eq!(
            server_properties_with_port("server-port 25565\r\nx\r\n", 25599, false),
            "server-port 25599\r\nx\r\n"
        );
        assert_eq!(
            server_properties_with_port("query-port=25565\nserver-port=1\n", 25599, false),
            "query-port=25565\nserver-port=25599\n"
        );
    }

    #[test]
    fn properties_appends_port_when_missing() {
        assert_eq!(server_properties_with_port("motd=hi", 25565, false), "motd=hi\nserver-port=25565\n");
        assert_eq!(server_properties_with_port("", 25565, false), "server-port=25565\n");
        assert_eq!(server_properties_with_port("a\r\nb\r\n", 25565, false), "a\r\nb\r\nserver-port=25565\r\n");
    }

    #[test]
    fn properties_public_bind_clears_server_ip() {
        assert_eq!(
            server_properties_with_port("server-port=1\nserver-ip=10.0.0.1\n", 2, true),
            "server-port=2\nserver-ip=\n"
        );
        assert_eq!(server_properties_with_port("server-port=1\n", 2, true), "server-port=2\nserver-ip=\n");
    }

    #[test]
    fn parses_players_and_tps() {
        let mut entry = test_entry();
        parse_server_line(&mut entry, "There are 3 of a max of 20 players online: a, b, c");
        assert_eq!((entry.players, entry.max_players), (3, 20));
        assert_eq!(entry.tps, 0.0);
        parse_server_line(&mut entry, "Can't keep up! TPS: 19.87");
        assert_eq!(entry.tps, 19.87);
        parse_server_line(&mut entry, "Recent tps=18.0, 17.5");
        assert_eq!(entry.tps, 18.0);
    }

    #[test]
    fn ring_buffer_trims_to_limit() {
        let mut entry = test_entry();
        for index in 0..600 {
            push_log(&mut entry, &format!("line-{index}"));
        }
        assert_eq!(entry.logs.len(), MAX_LOG_LINES);
        assert_eq!(entry.logs.front().unwrap().1, "line-100");
        assert_eq!(entry.logs.back().unwrap().1, "line-599");
        assert_eq!(entry.log_sequence, 600);
    }

    #[test]
    fn long_lines_are_cut_to_tail() {
        let mut entry = test_entry();
        let long = format!("{}END", "x".repeat(3000));
        push_log(&mut entry, &long);
        let stored = entry.logs.back().unwrap().1.clone();
        assert_eq!(stored.chars().count(), MAX_LOG_LINE_CHARS);
        assert!(stored.ends_with("END"));
    }

    #[test]
    fn log_response_incremental_and_reset() {
        let entries: Vec<(u64, String)> = (1..=4).map(|index| (index, format!("l{index}"))).collect();
        let value = log_response(&entries, 3, 4, true);
        assert_eq!(value["lines"], json!(["l4"]));
        assert_eq!(value["reset"], false);
        let value = log_response(&entries, 0, 4, true);
        assert_eq!(value["lines"], json!(["l1", "l2", "l3", "l4"]));
        assert_eq!(value["reset"], false);
        let value = log_response(&entries, 99, 4, false);
        assert_eq!(value["lines"], json!(["l1", "l2", "l3", "l4"]));
        assert_eq!(value["reset"], true);
        let value = log_response(&entries, 2, 4, true);
        assert_eq!(value["lines"], json!(["l3", "l4"]));
        assert_eq!(value["reset"], false);
    }

    #[test]
    fn console_selection_respects_limits() {
        let lines: Vec<String> = (0..900).map(|index| format!("line-{index}")).collect();
        let selected = select_console_lines(&lines);
        assert_eq!(selected.len(), 800);
        assert_eq!(selected.first().unwrap(), "line-100");
        assert_eq!(selected.last().unwrap(), "line-899");
        let fat: Vec<String> = (0..200).map(|_| "a".repeat(3000)).collect();
        let selected = select_console_lines(&fat);
        assert!(selected.len() > 100 && selected.len() < 200);
        assert!(selected.iter().all(|line| line.chars().count() <= 2048));
    }

    #[test]
    fn unique_ids_differ() {
        assert_ne!(unique_id(), unique_id());
    }

    #[test]
    fn tail_chars_takes_suffix() {
        assert_eq!(tail_chars("abcdef", 3), "def");
        assert_eq!(tail_chars("abc", 5), "abc");
    }

    #[test]
    fn int_fields_follow_python_int() {
        let value = json!({ "port": " 123 ", "max_players": 12.7 });
        assert_eq!(py_int_field(&value, "port", 1), Some(123));
        assert_eq!(py_int_field(&value, "max_players", 1), Some(12));
        assert_eq!(py_int_field(&value, "missing", 7), Some(7));
        assert_eq!(py_int_field(&json!({ "port": null }), "port", 7), None);
    }

    #[test]
    fn compute_metrics_tracks_peaks() {
        let mut cache = MetricsCache::default();
        let first = compute_metrics(&mut cache, 42, 100.0, true, 2, 20, 19.5, Some((1.0, 1024)), 4096, 100.0);
        assert_eq!(first["pid"], 42);
        assert_eq!(first["running"], true);
        assert_eq!(first["ram_bytes"], 1024);
        assert_eq!(first["cpu_percent"], 0.0);
        let second = compute_metrics(&mut cache, 42, 100.0, true, 2, 20, 19.5, Some((2.0, 2048)), 8192, 102.0);
        assert_eq!(second["cpu_percent"], 50.0);
        assert_eq!(second["peak_ram_bytes"], 2048);
        let gone = compute_metrics(&mut cache, 42, 100.0, true, 2, 20, 19.5, None, 0, 104.0);
        assert_eq!(gone["pid"], 0);
        assert_eq!(gone["running"], false);
        assert_eq!(gone["peak_ram_bytes"], 2048);
    }
}
