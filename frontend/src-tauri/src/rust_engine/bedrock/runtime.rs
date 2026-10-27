use super::*;

pub(super) struct BolRuntime {
    pub(super) commands: Vec<Vec<String>>,
    pub(super) setup_error: String,
}

pub(super) struct ProbeStatus {
    pub(super) supported: bool,
    pub(super) version: String,
    pub(super) message: String,
}

/// имя файла встроенной среды: версия и имя держатся в синхроне
fn bundled_pyz_name() -> &'static str {
    static NAME: LazyLock<String> =
        LazyLock::new(|| format!("bedrock-on-linux-{BEDROCK_ON_LINUX_BUNDLED_VERSION}.pyz"));
    &NAME
}

pub(super) fn bundled_pyz_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("NIVORA_BEDROCK_ON_LINUX_PYZ") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            candidates.push(dir.join("vendor").join(bundled_pyz_name()));
            candidates.push(dir.join("../share/nivora-launcher/vendor").join(bundled_pyz_name()));
            candidates.push(dir.join("../Resources/vendor").join(bundled_pyz_name()));
            // при разработке исполняемый файл в target/debug, среда в resources/vendor
            candidates.push(dir.join("../../resources/vendor").join(bundled_pyz_name()));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("vendor").join(bundled_pyz_name()));
    }
    candidates.push(launcher_roots().data_home.join("vendor").join(bundled_pyz_name()));
    candidates
}

pub(super) fn python_for_zipapp() -> Option<PathBuf> {
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

pub(super) fn bol_runtime() -> BolRuntime {
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

/// минимальная версия glibc для входа в Store через Xodus
pub(super) fn glibc_version() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let ldd = find_in_path("ldd")?;
    let output = run_timed(prepare_command(super::super::platform::command(ldd).arg("--version")), Duration::from_secs(10))?;
    glibc_re()
        .captures(&output.1)
        .and_then(|captures| captures.get(1).map(|value| value.as_str().to_owned()))
}

pub(super) fn bol_probe(command: &[String], home: &Path) -> ProbeStatus {
    let unsupported = |version: String, message: &str| ProbeStatus {
        supported: false,
        version,
        message: message.to_owned(),
    };
    let mut process = super::super::platform::command(&command[0]);
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

pub(super) fn bol_select_supported(commands: &[Vec<String>], home: &Path) -> (Vec<String>, ProbeStatus) {
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

pub(super) fn bol_require_runtime(home: &Path) -> Result<(Vec<String>, ProbeStatus), String> {
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

pub(super) fn stop_registered_process(key: &str) -> bool {
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
            let _ = super::super::platform::command(taskkill)
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

pub(super) fn bol_environment(home: &Path, xdg_data_home: Option<&Path>) -> HashMap<String, String> {
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

pub(super) fn bol_run(
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
    let mut process = super::super::platform::command(&command[0]);
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
                    // отсутствие вывода не означает зависание: do_setup в CLI
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

pub(super) fn spawn_line_reader<S: Read + Send + 'static>(stream: Option<S>, sender: mpsc::Sender<Option<String>>) {
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

pub(super) fn clean_process_line(raw: &[u8]) -> Option<String> {
    let mut cleaned: Vec<u8> = Vec::with_capacity(raw.len().min(1024));
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        // удаляем CSI-последовательности терминала и OSC-заголовки
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

/// размер в человекочитаемом виде для heartbeat-строк скачивания
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

/// результат проверки роста дерева, если процесс не пишет в stdout
#[derive(Debug, PartialEq)]
pub(super) enum WatchdogVerdict {
    Pending,
    Active(u64),
    Silent,
}

pub(super) struct DownloadWatchdog {
    root: PathBuf,
    snapshot: u64,
    have_snapshot: bool,
    last_walk: Option<Instant>,
    interval: Duration,
}

impl DownloadWatchdog {
    pub(super) fn new(root: &Path) -> Self {
        Self::with_interval(root, Duration::from_secs(20))
    }

    pub(super) fn with_interval(root: &Path, interval: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            snapshot: 0,
            have_snapshot: false,
            last_walk: None,
            interval,
        }
    }

    pub(super) fn check(&mut self) -> WatchdogVerdict {
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

