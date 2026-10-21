// системные справочники и пробы окружения: выбор Java для запуска,
// состояние песочницы bubblewrap и каталог серверных ядер.
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::state::{launcher_roots, MAX_SETTINGS_JSON_BYTES};

const SERVER_CORES: [[&str; 4]; 10] = [
    ["paper", "Paper", "PaperMC", "Быстрое стабильное ядро для Paper API."],
    ["purpur", "Purpur", "PaperMC", "Paper-совместимое ядро с расширенными настройками."],
    ["folia", "Folia", "PaperMC", "Региональный параллельный серверный runtime."],
    ["spigot", "Spigot", "Bukkit", "Классическое Bukkit-совместимое ядро."],
    ["leaf", "Leaf", "Paper", "Paper fork с дополнительными оптимизациями."],
    ["pufferfish", "Pufferfish", "Paper", "Оптимизированное Paper-совместимое ядро."],
    ["patina", "Patina", "Paper", "Производительный Paper-based дистрибутив."],
    ["velocity", "Velocity", "Proxy", "Современное прокси-ядро для сети серверов."],
    ["waterfall", "Waterfall", "Proxy", "BungeeCord-совместимое прокси-ядро."],
    ["custom", "Своё ядро", "Custom", "Загрузите любой совместимый server.jar."],
];

const SECURITY_PROFILES: [&str; 4] = ["compat", "off", "manual", "strict"];
const SECURITY_PERMISSIONS: [&str; 10] = [
    "network:game",
    "files:instance",
    "desktop:display",
    "desktop:audio",
    "files:host-read",
    "files:host-write",
    "process:spawn",
    "chat:read",
    "input:capture",
    "native:load",
];

fn version_output_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"version\s+"([^"]+)"#).expect("regex версии"))
}

/// поиск исполняемого файла в PATH (первый существующий и исполняемый).
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_file() && super::platform::is_executable(&meta) {
            return Some(candidate);
        }
    }
    None
}

fn expanduser(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(value)
}

/// запуск с таймаутом: зависший процесс убивается, а не вешает операцию.
struct RunOutcome {
    status: Option<i32>,
    stderr_tail: String,
    stdout: String,
}

fn run_with_timeout(program: &Path, args: &[&str], timeout: Duration) -> Option<RunOutcome> {
    let mut child = super::platform::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut stream) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut stream, &mut stdout);
                }
                if let Some(mut stream) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stream, &mut stderr);
                }
                return Some(RunOutcome { status: status.code(), stderr_tail: stderr, stdout });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

fn validate_java(resolved: &Path) -> Result<(), String> {
    let name = resolved.file_name().and_then(|value| value.to_str()).unwrap_or("").to_lowercase();
    let parent_name = resolved.parent().and_then(|value| value.file_name()).and_then(|value| value.to_str()).unwrap_or("").to_lowercase();
    let meta = std::fs::metadata(resolved).map_err(|_| err("Java executable не найден"))?;
    // набор имён как в validate_java_path (runtime.py): на Windows бинарники
    // носят суффикс .exe.
    let allowed_names: &[&str] = if cfg!(windows) { &["java.exe", "javaw.exe"] } else { &["java", "javaw"] };
    if !allowed_names.contains(&name.as_str()) || parent_name != "bin" || !meta.is_file() {
        return Err(err("Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin"));
    }
    if !super::platform::is_executable(&meta) {
        return Err(err("Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin"));
    }
    // владение и group/world-запись — Unix-специфика; на Windows каталоги
    // JDK закрыты ACL профиля пользователя, проверять нечего.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let root = resolved.parent().and_then(Path::parent).ok_or_else(|| err("Не удалось проверить права Java executable"))?;
        let bin_meta = std::fs::metadata(resolved.parent().unwrap()).map_err(|_| err("Не удалось проверить права Java executable"))?;
        let root_meta = std::fs::metadata(root).map_err(|_| err("Не удалось проверить права Java executable"))?;
        let euid = unsafe { libc::geteuid() };
        for meta in [&meta, &bin_meta, &root_meta] {
            let uid = meta.uid();
            if uid != euid && uid != 0 {
                return Err(err("Java executable и каталог JDK/JRE должны принадлежать текущему пользователю или root"));
            }
            if meta.mode() & 0o022 != 0 {
                return Err(err("Java executable и каталог JDK/JRE не должны быть доступны для записи группе или всем"));
            }
        }
    }
    Ok(())
}

fn err(message: &str) -> String {
    message.to_owned()
}

/// обнаружение установленных Java: JAVA_HOME, PATH, /usr/lib/jvm, /opt.
fn discover_java_paths() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let trimmed = java_home.trim();
        if !trimmed.is_empty() {
            candidates.push(expanduser(trimmed).join("bin").join("java"));
        }
    }
    if let Some(system_java) = which("java") {
        candidates.push(system_java);
    }
    for root in ["/usr/lib/jvm", "/opt"] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let candidate = entry.path().join("bin").join("java");
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }
    let mut result: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for candidate in candidates {
        let Ok(resolved) = candidate.canonicalize() else {
            continue;
        };
        if !resolved.is_file() || seen.contains(&resolved) || validate_java(&resolved).is_err() {
            continue;
        }
        seen.insert(resolved.clone());
        result.push(resolved);
    }
    result
}

/// версия Java из вывода `-version`: главная цифра (для 1.8 — вторая).
fn java_version(java_path: &Path) -> Option<u32> {
    let outcome = run_with_timeout(java_path, &["-version"], Duration::from_secs(10))?;
    let output = format!("{}{}", outcome.stderr_tail, outcome.stdout);
    let captures = version_output_re().captures(&output)?;
    let raw = captures.get(1)?.as_str();
    let value = if let Some(rest) = raw.strip_prefix("1.") {
        rest.split('.').next().unwrap_or(rest)
    } else {
        raw.split('.').next().unwrap_or(raw)
    };
    value.parse().ok()
}

fn configured_java_path() -> String {
    let settings = super::state::read_bounded_json(&launcher_roots().config_home.join("settings.json"), MAX_SETTINGS_JSON_BYTES);
    settings
        .and_then(|value| value.get("java_path").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

pub fn java_validate(request: &Value) -> Result<Value, String> {
    let raw = request.get("path").and_then(Value::as_str).unwrap_or_default().trim();
    if raw.is_empty() {
        return Err(err("Укажите путь к Java executable"));
    }
    let path = expanduser(raw);
    // allow_directory: каталог JDK/JRE дополняется до bin/java — как в
    // validate_java_path (runtime.py).
    let resolved = if path.is_dir() {
        path.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" })
    } else {
        path
    };
    validate_java(&resolved)?;
    // javaw не пишет в stderr/stdout — версию спрашиваем у консольного
    // собрата из того же bin.
    let probe = if resolved.file_name().and_then(|value| value.to_str()).map(|value| value.to_lowercase() == "javaw.exe").unwrap_or(false) {
        resolved.with_file_name("java.exe")
    } else {
        resolved.clone()
    };
    let version = java_version(&probe).ok_or_else(|| err(&format!("Не удалось определить версию Java: {}", resolved.to_string_lossy())))?;
    let canonical = super::platform_support::canonical_path(&resolved)
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|_| resolved.to_string_lossy().to_string());
    Ok(json!({ "path": canonical, "version": version }))
}

pub fn java_options(_request: &Value) -> Result<Value, String> {
    let mut options: Vec<Value> = vec![json!({
        "value": "auto",
        "label": "Автоматически",
        "meta": "Mojang runtime → системная Java",
    })];
    let mut seen: HashSet<String> = ["auto".to_owned()].into();
    for path in discover_java_paths() {
        let value = path.to_string_lossy().to_string();
        if !seen.insert(value.clone()) {
            continue;
        }
        let label = match java_version(&path) {
            Some(version) => format!("Java {version}"),
            None => path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned(),
        };
        options.push(json!({ "value": value, "label": label, "meta": value }));
    }
    let configured = configured_java_path();
    if !configured.is_empty() && !seen.contains(&configured) {
        options.push(json!({ "value": configured, "label": "Настроенный путь", "meta": configured }));
    }
    Ok(json!({ "options": options }))
}

/// Отвечает состояние платформы безопасности. Проба bubblewrap запускает
/// процесс с таймаутом до 3 секунд: без кеша каждый bootstrap (включая
/// дымовые прогоны и автоповторы фронтенда) платил это заново, хотя
/// окружение внутри жизни процесса не меняется.
static SECURITY_STATUS_CACHE: OnceLock<Result<Value, String>> = OnceLock::new();

pub fn security_status(_request: &Value) -> Result<Value, String> {
    if let Some(cached) = SECURITY_STATUS_CACHE.get() {
        return cached.clone();
    }
    let computed = security_status_uncached();
    let _ = SECURITY_STATUS_CACHE.set(computed.clone());
    computed
}

fn security_status_uncached() -> Result<Value, String> {
    {
    let bubblewrap = which("bwrap");
    let mut strict_available = false;
    let mut probe_error = String::new();
    if let Some(path) = &bubblewrap {
        let true_binary = which("true").unwrap_or_else(|| PathBuf::from("/usr/bin/true"));
        let mut args: Vec<&str> = vec![
            "--die-with-parent",
            "--new-session",
            "--unshare-user",
            "--unshare-pid",
            "--ro-bind",
            "/usr",
            "/usr",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--",
        ];
        let true_path = true_binary.to_string_lossy().to_string();
        args.push(true_path.as_str());
        match run_with_timeout(path, &args, Duration::from_secs(3)) {
            Some(outcome) if outcome.status == Some(0) => strict_available = true,
            Some(outcome) => {
                probe_error = if outcome.stderr_tail.trim().is_empty() {
                    "bubblewrap probe failed".to_owned()
                } else {
                    outcome.stderr_tail.trim().to_owned()
                };
                probe_error = probe_error.chars().take(256).collect();
            }
            None => probe_error = "превышено время ожидания bubblewrap".to_owned(),
        }
    }
    Ok(json!({
        "platform": if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "darwin" } else { "linux" },
        "bubblewrap": bubblewrap.is_some(),
        "bubblewrap_path": bubblewrap.map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
        "strict_available": strict_available,
        "strict_probe_error": probe_error,
        "profiles": SECURITY_PROFILES,
        "permissions": SECURITY_PERMISSIONS,
    }))
    }
}

pub fn server_cores(_request: &Value) -> Result<Value, String> {
    let mut cores: Vec<Value> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for [id, name, family, description] in SERVER_CORES {
        if !seen.insert(id) {
            continue;
        }
        cores.push(json!({ "id": id, "name": name, "family": family, "description": description }));
    }
    Ok(json!({ "cores": cores }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-system-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn server_cores_catalog_is_unique_and_complete() {
        let response = server_cores(&json!({})).expect("server_cores");
        let cores = response["cores"].as_array().expect("список ядер");
        assert_eq!(cores.len(), 10);
        let mut ids: Vec<&str> = cores.iter().map(|core| core["id"].as_str().unwrap()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 10);
        assert_eq!(cores[0]["id"], json!("paper"));
    }

    #[test]
    #[cfg(unix)] // сценарий с #!/bin/sh и битами +x
    fn java_validation_requires_bin_layout_and_owner() {
        let root = temp_root("java");
        let jdk = root.join("jdk-17");
        fs::create_dir_all(jdk.join("bin")).expect("bin");
        // права каталогов фиксируем явно: umask окружения тестов может
        // давать group-writable 0o775, который валидатор обязан отвергать.
        fs::set_permissions(&jdk, fs::Permissions::from_mode(0o755)).expect("jdk 755");
        fs::set_permissions(jdk.join("bin"), fs::Permissions::from_mode(0o755)).expect("bin 755");
        let java = jdk.join("bin").join("java");
        fs::write(&java, b"#!/bin/sh\necho 'openjdk version \"17.0.2\" 2022-01-18' >&2\n").expect("java");
        fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).expect("+x");
        assert!(validate_java(&java).is_ok(), "своя JDK с владельцем-пользователем валидна");

        // java вне каталога bin отвергается: родитель называется lib.
        let misplaced = root.join("lib").join("java");
        fs::create_dir_all(root.join("lib")).expect("lib");
        fs::write(&misplaced, b"#!/bin/sh\n").expect("java");
        fs::set_permissions(&misplaced, fs::Permissions::from_mode(0o755)).expect("+x");
        assert!(validate_java(&misplaced).is_err(), "java вне bin отвергается");

        let world_writable = jdk.join("bin").join("javaw");
        fs::write(&world_writable, b"#!/bin/sh\n").expect("javaw");
        fs::set_permissions(&world_writable, fs::Permissions::from_mode(0o777)).expect("777");
        assert!(validate_java(&world_writable).is_err(), "запись для всех отвергается");
    }

    #[test]
    #[cfg(unix)] // сценарий с #!/bin/sh и битами +x
    fn java_version_parses_modern_and_legacy_outputs() {
        let root = temp_root("version");
        fs::create_dir_all(root.join("bin")).expect("bin");
        let java = root.join("bin").join("java");
        fs::write(&java, "#!/bin/sh\necho 'java version \"1.8.0_392\"' >&2\n").expect("java");
        fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).expect("+x");
        assert_eq!(java_version(&java), Some(8));

        fs::write(&java, "#!/bin/sh\necho 'openjdk version \"21.0.3\" 2024-04-16' >&2\n").expect("java");
        assert_eq!(java_version(&java), Some(21));

        fs::write(&java, "#!/bin/sh\nexit 0\n").expect("java без вывода");
        assert_eq!(java_version(&java), None);
    }

    #[test]
    fn security_status_reports_shape_even_without_bubblewrap() {
        let response = security_status(&json!({})).expect("security_status");
        for key in ["platform", "bubblewrap", "bubblewrap_path", "strict_available", "strict_probe_error", "profiles", "permissions"] {
            assert!(response.get(key).is_some(), "нет ключа {key}");
        }
        assert_eq!(response["profiles"].as_array().map(Vec::len), Some(4));
        assert_eq!(response["permissions"].as_array().map(Vec::len), Some(10));
    }
}
