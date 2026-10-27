// выбор Java, проверка песочницы и каталог серверных ядер
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

/// частота физического дисплея, не анимации браузера; неизвестную частоту не заменяем на 60 Гц
pub(crate) fn display_refresh_hz() -> Option<i64> {
    #[cfg(target_os = "windows")]
    {
        return windows_display_refresh_hz();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_display_refresh_hz();
    }
    #[cfg(target_os = "linux")]
    {
        return linux_display_refresh_hz();
    }
    #[allow(unreachable_code)]
    None
}

fn valid_refresh_hz(value: f64) -> Option<i64> {
    if !value.is_finite() || !(24.0..=480.0).contains(&value) {
        return None;
    }
    Some(value.round() as i64)
}

fn parse_refresh_token(token: &str) -> Option<i64> {
    let number = token.trim().trim_end_matches(|ch: char| !ch.is_ascii_digit() && ch != '.');
    if number.is_empty() {
        return None;
    }
    valid_refresh_hz(number.parse().ok()?)
}

/// возвращает максимальную частоту активных дисплеев
fn highest_refresh_hz(rates: impl IntoIterator<Item = Option<i64>>) -> Option<i64> {
    rates
        .into_iter()
        .flatten()
        .filter(|rate| (24..=480).contains(rate))
        .max()
}

fn parse_wlr_randr_current(text: &str) -> Option<i64> {
    highest_refresh_hz(text.lines().filter_map(|line| {
        if !line.to_ascii_lowercase().contains("current") {
            return None;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        tokens
            .iter()
            .position(|token| token.eq_ignore_ascii_case("hz"))
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| parse_refresh_token(tokens[index]))
            .map(Some)
    }))
}

fn parse_kscreen_current(text: &str) -> Option<i64> {
    highest_refresh_hz(text.lines().filter_map(|line| {
        if !line.contains('*') {
            return None;
        }
        let mode = line.split_whitespace().find(|token| token.contains('@'))?;
        parse_refresh_token(mode.split('@').nth(1)?).map(Some)
    }))
}

fn parse_xrandr_current(text: &str) -> Option<i64> {
    let mut rates = Vec::new();
    let mut in_connected_output = false;
    for line in text.lines() {
        if line.contains(" connected ") {
            in_connected_output = true;
            continue;
        }
        if line.contains(" disconnected ") {
            in_connected_output = false;
            continue;
        }
        if !in_connected_output {
            continue;
        }
        if let Some(rate) = line
            .split_whitespace()
            .find(|token| token.contains('*'))
            .and_then(|token| parse_refresh_token(token))
        {
            rates.push(Some(rate));
        }
    }
    highest_refresh_hz(rates)
}

#[cfg(target_os = "linux")]
fn linux_display_refresh_hz() -> Option<i64> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(output) = super::platform::command("wlr-randr").output() {
            if output.status.success() {
                if let Some(rate) = parse_wlr_randr_current(&String::from_utf8_lossy(&output.stdout)) {
                    return Some(rate);
                }
            }
        }
        if let Ok(output) = super::platform::command("kscreen-doctor").arg("-o").output() {
            if output.status.success() {
                return parse_kscreen_current(&String::from_utf8_lossy(&output.stdout));
            }
        }
        return None;
    }

    if std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let output = super::platform::command("xrandr").arg("--current").output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_xrandr_current(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "windows")]
fn windows_display_refresh_hz() -> Option<i64> {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW,
        DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DM_DISPLAYFREQUENCY, ENUM_CURRENT_SETTINGS,
    };

    let mut rates = Vec::new();
    for index in 0..64u32 {
        let mut display: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        display.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut display, 0) } == 0 {
            break;
        }
        if display.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
            continue;
        }
        let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
        mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if unsafe {
            EnumDisplaySettingsW(display.DeviceName.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode)
        } != 0
            && mode.dmFields & DM_DISPLAYFREQUENCY != 0
        {
            rates.push(mode.dmDisplayFrequency as f64);
        }
    }

    let detected = highest_refresh_hz(rates.into_iter().map(valid_refresh_hz));
    if detected.is_some() {
        return detected;
    }

    // если драйвер не указал имя устройства, проверяем основной монитор
    let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
    mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    let found = unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode) };
    if found == 0 || mode.dmFields & DM_DISPLAYFREQUENCY == 0 {
        return None;
    }
    valid_refresh_hz(mode.dmDisplayFrequency as f64)
}

#[cfg(target_os = "macos")]
fn macos_display_refresh_hz() -> Option<i64> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(
            max_displays: u32,
            active_displays: *mut u32,
            display_count: *mut u32,
        ) -> i32;
        fn CGDisplayCopyDisplayMode(display: u32) -> *const c_void;
        fn CGDisplayModeGetRefreshRate(mode: *const c_void) -> f64;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const c_void);
    }

    let mut count = 0;
    if unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) } != 0 || count == 0 {
        return None;
    }
    let mut displays = vec![0u32; count as usize];
    if unsafe { CGGetActiveDisplayList(count, displays.as_mut_ptr(), &mut count) } != 0 {
        return None;
    }
    displays.truncate(count as usize);
    let rates = displays.into_iter().filter_map(|display| {
        let mode = unsafe { CGDisplayCopyDisplayMode(display) };
        if mode.is_null() {
            return None;
        }
        let refresh = unsafe { CGDisplayModeGetRefreshRate(mode) };
        unsafe { CFRelease(mode) };
        valid_refresh_hz(refresh)
    });
    highest_refresh_hz(rates.map(Some))
}

fn version_output_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"version\s+"([^"]+)"#).expect("regex версии"))
}

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

/// запускает процесс с таймаутом и завершает его при зависании
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
    // в Windows принимаем `java.exe` и `javaw.exe`
    let allowed_names: &[&str] = if cfg!(windows) { &["java.exe", "javaw.exe"] } else { &["java", "javaw"] };
    if !allowed_names.contains(&name.as_str()) || parent_name != "bin" || !meta.is_file() {
        return Err(err("Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin"));
    }
    if !super::platform::is_executable(&meta) {
        return Err(err("Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin"));
    }
    // в Unix проверяем владельца и запрет записи для группы и остальных; в Windows это обеспечивает ACL профиля
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

/// извлекает версию Java из `-version`; для 1.8 берёт вторую цифру
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
    let resolved = if path.is_dir() {
        path.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" })
    } else {
        path
    };
    validate_java(&resolved)?;
    // `javaw` не выводит версию, поэтому проверяем `java` из того же каталога
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

/// кеширует состояние песочницы: проверка bubblewrap занимает до трёх секунд
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
    if cfg!(windows) {
        // проверка Windows запускает процесс в AppContainer
        let (available, error) = super::windows_sandbox::probe();
        return Ok(json!({
            "platform": "windows",
            "sandbox": "appcontainer",
            "bubblewrap": false,
            "bubblewrap_path": "",
            "strict_available": available,
            "strict_probe_error": error,
            "profiles": SECURITY_PROFILES,
            "permissions": SECURITY_PERMISSIONS,
        }));
    }
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
        // поле `sandbox` содержит название выбранной среды
        "sandbox": if bubblewrap.is_some() { "bubblewrap" } else { "none" },
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
    fn display_refresh_parsers_only_accept_the_active_native_mode() {
        let wlr = "DP-1 \"Display\"\n  2560x1440 px, 60.000000 Hz (current, preferred)\n  1920x1080 px, 144.000000 Hz\nDP-2 \"Fast display\"\n  2560x1440 px, 144.000000 Hz (current, preferred)\n";
        assert_eq!(parse_wlr_randr_current(wlr), Some(144));
        assert_eq!(parse_wlr_randr_current("DP-1 1920x1080 px, 60.000000 Hz"), None);

        let kscreen = "Output: 1 eDP-1 enabled connected\n  Modes: 0: 1920x1080@60.000*! 1: 1920x1080@100.000\nOutput: 2 DP-1 enabled connected\n  Modes: 0: 2560x1440@144.000*! 1: 2560x1440@60.000\n";
        assert_eq!(parse_kscreen_current(kscreen), Some(144));
        assert_eq!(parse_kscreen_current("Modes: 0: 1920x1080@60.000"), None);

        let xrandr = "HDMI-1 connected primary 1920x1080+0+0\n   1920x1080  60.00*+  59.94\nDP-1 connected 2560x1440+1920+0\n   2560x1440  144.00*+  60.00\n";
        assert_eq!(parse_xrandr_current(xrandr), Some(144));
        assert_eq!(parse_xrandr_current("HDMI-1 connected 1920x1080+0+0\n  1920x1080  60.00+"), None);
        assert_eq!(valid_refresh_hz(0.0), None);
        assert_eq!(valid_refresh_hz(1.0), None);
        assert_eq!(valid_refresh_hz(1000.0), None);
        assert_eq!(highest_refresh_hz([None, Some(1), Some(144)]), Some(144));
        assert_eq!(highest_refresh_hz([None, Some(1)]), None);
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
        // фиксируем права явно: umask тестовой среды может разрешить запись группе
        fs::set_permissions(&jdk, fs::Permissions::from_mode(0o755)).expect("jdk 755");
        fs::set_permissions(jdk.join("bin"), fs::Permissions::from_mode(0o755)).expect("bin 755");
        let java = jdk.join("bin").join("java");
        fs::write(&java, b"#!/bin/sh\necho 'openjdk version \"17.0.2\" 2022-01-18' >&2\n").expect("java");
        fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).expect("+x");
        assert!(validate_java(&java).is_ok(), "своя JDK с владельцем-пользователем валидна");

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
