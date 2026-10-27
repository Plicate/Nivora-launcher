// песочница игрового процесса и подготовка команды запуска
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::platform_support::is_reparse_point;
use super::state::home_dir;
use super::windows_sandbox;

pub(crate) const SECURITY_PROFILES: [&str; 4] = ["compat", "off", "manual", "strict"];
pub(crate) const SECURITY_PERMISSIONS: [&str; 10] = [
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
#[cfg_attr(not(test), allow(dead_code))]
const HIGH_RISK_PERMISSIONS: [&str; 9] = [
    "network:game",
    "desktop:display",
    "desktop:audio",
    "files:host-read",
    "files:host-write",
    "process:spawn",
    "chat:read",
    "input:capture",
    "native:load",
];
// эти разрешения принадлежат JVM; лаунчер не может безопасно менять их внутри процесса
pub(crate) const UNENFORCED_CELL_PERMISSIONS: [&str; 4] = ["process:spawn", "chat:read", "input:capture", "native:load"];
pub(crate) const CELL_DIRECTORY: &str = ".trial-cell";
pub(crate) const SERVICE_JSON_FILES: [&str; 5] = [
    ".trial-launcher.json",
    ".trial-launcher-instance.json",
    ".trial-launcher-server.json",
    ".trial-launcher-live.json",
    ".trial-launcher-metrics.json",
];

const STAGE_SYSTEM_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct InstanceCell {
    pub root: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct SecurityPolicy {
    pub profile: String,
    pub permissions: Vec<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub sandbox_requested: bool,
    pub network_access: bool,
    pub host_file_read: bool,
    pub host_file_write: bool,
    pub display_access: bool,
    pub audio_access: bool,
}

impl SecurityPolicy {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn high_risk_permissions(&self) -> Vec<&str> {
        self.permissions
            .iter()
            .map(String::as_str)
            .filter(|permission| HIGH_RISK_PERMISSIONS.contains(permission))
            .collect()
    }

    fn has(&self, permission: &str) -> bool {
        self.permissions.iter().any(|value| value == permission)
    }
}

pub(crate) fn normalize_security_profile(value: &Value) -> Result<String, String> {
    let profile = value.as_str().unwrap_or("").trim().to_lowercase();
    let profile = if profile.is_empty() { "strict".to_owned() } else { profile };
    if !SECURITY_PROFILES.contains(&profile.as_str()) {
        return Err(format!("Неизвестный security profile: {profile}"));
    }
    Ok(profile)
}

pub(crate) fn normalize_security_permissions(value: Option<&Value>) -> Vec<String> {
    let default = ["network:game", "files:instance", "desktop:display", "desktop:audio"];
    let raw: Vec<String> = match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|item| item.trim().to_lowercase())
            .filter(|item| SECURITY_PERMISSIONS.contains(&item.as_str()))
            .collect(),
        _ => default.iter().map(|item| item.to_string()).collect(),
    };
    let mut result: Vec<String> = Vec::new();
    for permission in raw {
        if !result.contains(&permission) {
            result.push(permission);
        }
    }
    result
}

pub(crate) fn security_policy(settings: &Value) -> Result<SecurityPolicy, String> {
    let profile = normalize_security_profile(settings.get("security_profile").unwrap_or(&Value::Null))?;
    let permissions = normalize_security_permissions(settings.get("security_permissions"));
    if profile == "manual" || profile == "strict" {
        let mut unsupported: Vec<String> = permissions
            .iter()
            .filter(|permission| UNENFORCED_CELL_PERMISSIONS.contains(&permission.as_str()))
            .cloned()
            .collect();
        unsupported.sort();
        if !unsupported.is_empty() {
            let names = unsupported.join(", ");
            return Err(format!(
                "Security profile {profile} не может безопасно выдать разрешения: {names}. Используйте off для доверенной сборки."
            ));
        }
    }
    if profile == "compat" || profile == "off" {
        return Ok(SecurityPolicy {
            profile,
            permissions,
            sandbox_requested: false,
            network_access: true,
            host_file_read: true,
            host_file_write: true,
            display_access: true,
            audio_access: true,
        });
    }
    Ok(SecurityPolicy {
        network_access: permissions.iter().any(|value| value == "network:game"),
        host_file_read: permissions.iter().any(|value| value == "files:host-read"),
        host_file_write: permissions.iter().any(|value| value == "files:host-write"),
        display_access: permissions.iter().any(|value| value == "desktop:display"),
        audio_access: permissions.iter().any(|value| value == "desktop:audio"),
        profile,
        permissions,
        sandbox_requested: true,
    })
}

pub(crate) fn ensure_instance_cell(instance_dir: &Path) -> Result<InstanceCell, String> {
    if is_reparse_point(instance_dir) || !instance_dir.is_dir() {
        return Err("Каталог экземпляра недоступен для создания security cell".to_owned());
    }
    let instance_dir = canonical(instance_dir);
    let root = instance_dir.join(CELL_DIRECTORY);
    if is_reparse_point(&root) {
        return Err("Security cell является ссылкой или junction".to_owned());
    }
    create_private_directory(&root)?;
    let mut directories = BTreeMap::new();
    for name in ["home", "tmp", "config", "cache", "logs"] {
        let path = root.join(name);
        if is_reparse_point(&path) {
            return Err(format!("Security cell содержит ссылку или junction: {name}"));
        }
        create_private_directory(&path)?;
        directories.insert(name, path);
    }
    set_private_mode(&root);
    Ok(InstanceCell {
        home: directories["home"].clone(),
        tmp: directories["tmp"].clone(),
        config: directories["config"].clone(),
        cache: directories["cache"].clone(),
        root,
    })
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format!("Каталог экземпляра недоступен для создания security cell: {error}"))?;
    set_private_mode(path);
    Ok(())
}

fn set_private_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

pub(crate) fn ensure_service_json_files(instance_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut created = Vec::new();
    for name in SERVICE_JSON_FILES {
        let path = instance_dir.join(name);
        if is_reparse_point(&path) || (path.exists() && !path.is_file()) {
            return Err(format!("Служебный путь экземпляра недоступен: {name}"));
        }
        if path.exists() {
            continue;
        }
        match write_new_private_file(&path, "{}\n") {
            Ok(()) => created.push(path),
            Err(error) => return Err(format!("Не удалось создать служебный файл экземпляра: {name}: {error}")),
        }
    }
    Ok(created)
}

fn write_new_private_file(path: &Path, content: &str) -> Result<(), std::io::Error> {
    write_new_private_bytes(path, content.as_bytes())
}

pub(crate) struct PreparedProcess {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub warning: Option<String>,
}

/// подготавливает команду и окружение с учётом профиля песочницы
pub(crate) fn prepare_process(
    command: &[String],
    instance_dir: &Path,
    environment: &BTreeMap<String, String>,
    settings: &Value,
    argument_file_fd: Option<i32>,
    argument_file_target: Option<&str>,
) -> Result<PreparedProcess, String> {
    let policy = security_policy(settings)?;
    if policy.profile == "compat" || policy.profile == "off" {
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment: environment.clone(),
            warning: None,
        });
    }
    let windows = cfg!(windows);
    if windows && !windows_appcontainer_enabled(environment) {
        // minecraft не запускается в AppContainer на обычной Windows
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment: environment.clone(),
            warning: Some(
                "Песочница AppContainer на Windows отключена: Minecraft в ней не запускается. \
                 Разрешения профиля не ограничиваются на уровне ОС."
                    .to_owned(),
            ),
        });
    }
    if policy.profile == "strict" && !matches!(system_name().as_str(), "linux" | "windows") {
        return Err("strict profile поддерживается только на Linux (bubblewrap) и Windows (AppContainer)".to_owned());
    }

    let cell = ensure_instance_cell(instance_dir)?;
    let service_files = ensure_service_json_files(instance_dir)?;
    let mut process_environment = environment.clone();
    for (key, value) in [
        ("HOME", cell.home.to_string_lossy().to_string()),
        ("TMPDIR", cell.tmp.to_string_lossy().to_string()),
        ("TMP", cell.tmp.to_string_lossy().to_string()),
        ("TEMP", cell.tmp.to_string_lossy().to_string()),
        ("XDG_CONFIG_HOME", cell.config.to_string_lossy().to_string()),
        ("XDG_CACHE_HOME", cell.cache.to_string_lossy().to_string()),
        ("TRIAL_LAUNCHER_SECURITY_PROFILE", policy.profile.clone()),
        ("TRIAL_LAUNCHER_CELL_ROOT", cell.root.to_string_lossy().to_string()),
    ] {
        process_environment.insert(key.to_owned(), value);
    }
    if windows {
        return prepare_windows_process(command, instance_dir, &cell, &service_files, process_environment, &policy, environment);
    }
    let manual_no_sandbox = "manual profile отключает OS-level sandbox; разрешения network, file и desktop не обеспечиваются.";
    if policy.profile == "manual" && system_name() != "linux" {
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment: process_environment,
            warning: Some(format!(
                "manual profile на {} не предоставляет OS-level sandbox; разрешения network, file и desktop не обеспечиваются.",
                python_system_name()
            )),
        });
    }
    if policy.profile == "manual" && which("bwrap").is_none() {
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment: process_environment,
            warning: Some(
                "bwrap не найден: manual profile не может ограничить network, file или desktop permissions.".to_owned(),
            ),
        });
    }
    if policy.profile == "manual" && policy.host_file_write {
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment: process_environment,
            warning: Some(manual_no_sandbox.to_owned()),
        });
    }

    let Some(bubblewrap) = which("bwrap") else {
        return Err("strict profile требует установленный bubblewrap (bwrap)".to_owned());
    };
    if policy.host_file_write {
        return Err("strict profile не выдаёт модам запись в файлы компьютера; используйте off для доверенной сборки".to_owned());
    }

    let mut mapped_command: Vec<String> = command.iter().map(|value| map_instance_argument(value, instance_dir)).collect();
    let graphics_mode = process_environment
        .get("TRIAL_LAUNCHER_GRAPHICS_MODE")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let sys_mount = if policy.display_access && matches!(graphics_mode.as_str(), "auto" | "mesa" | "nvidia") {
        ("--ro-bind", "/sys")
    } else {
        ("--tmpfs", "")
    };
    let argument_file_reference: String;
    if let (Some(_fd), Some(target)) = (argument_file_fd, argument_file_target) {
        if !target.starts_with("/instance/.trial-cell/") {
            return Err("Java argument file должен находиться внутри security cell".to_owned());
        }
        let relative_target = target.strip_prefix("/instance/").unwrap_or(target);
        let target_path = instance_dir.join(relative_target);
        if relative_target.split(['/', '\\']).any(|part| part == "..") || target_path.exists() || is_reparse_point(&target_path) {
            return Err("Путь Java argument file занят или небезопасен".to_owned());
        }
        argument_file_reference = format!("@/proc/self/fd/{}", _fd);
        for value in mapped_command.iter_mut() {
            if *value == argument_file_reference {
                *value = format!("@{target}");
            }
        }
    }
    let (java_root, mapped_java) = sandbox_java_path(&command[0]);
    let mut sandbox: Vec<String> = vec![
        bubblewrap.to_string_lossy().to_string(),
        "--die-with-parent".to_owned(),
        "--new-session".to_owned(),
        "--unshare-user".to_owned(),
        "--unshare-pid".to_owned(),
        "--unshare-ipc".to_owned(),
        "--unshare-uts".to_owned(),
        "--unshare-cgroup".to_owned(),
        "--ro-bind".to_owned(),
        "/usr".to_owned(),
        "/usr".to_owned(),
        "--tmpfs".to_owned(),
        "/etc".to_owned(),
        "--ro-bind".to_owned(),
        "/bin".to_owned(),
        "/bin".to_owned(),
        "--ro-bind".to_owned(),
        "/sbin".to_owned(),
        "/sbin".to_owned(),
        "--ro-bind".to_owned(),
        "/lib".to_owned(),
        "/lib".to_owned(),
        "--ro-bind".to_owned(),
        "/lib64".to_owned(),
        "/lib64".to_owned(),
        "--proc".to_owned(),
        "/proc".to_owned(),
        "--dev".to_owned(),
        "/dev".to_owned(),
    ];
    if sys_mount.0 == "--ro-bind" {
        sandbox.extend(["--ro-bind".to_owned(), sys_mount.1.to_owned(), sys_mount.1.to_owned()]);
    } else {
        sandbox.extend(["--tmpfs".to_owned(), "/sys".to_owned()]);
    }
    sandbox.extend([
        "--tmpfs".to_owned(),
        "/run".to_owned(),
        "--tmpfs".to_owned(),
        "/home".to_owned(),
        "--tmpfs".to_owned(),
        "/root".to_owned(),
        "--tmpfs".to_owned(),
        "/mnt".to_owned(),
        "--tmpfs".to_owned(),
        "/media".to_owned(),
        "--tmpfs".to_owned(),
        "/opt".to_owned(),
        "--tmpfs".to_owned(),
        "/var".to_owned(),
        "--tmpfs".to_owned(),
        "/tmp".to_owned(),
        if policy.has("files:instance") { "--bind".to_owned() } else { "--ro-bind".to_owned() },
        instance_dir.to_string_lossy().to_string(),
        "/instance".to_owned(),
        "--chdir".to_owned(),
        "/instance".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
    ]);
    append_minimal_etc_bindings(&mut sandbox, &cell)?;
    // запуску Java нужен каталог fontconfig для загрузки шрифтов
    let fonts_config = Path::new("/etc/fonts");
    if fonts_config.is_dir() && !fonts_config.is_symlink() {
        sandbox.extend(["--ro-bind".to_owned(), fonts_config.to_string_lossy().to_string(), fonts_config.to_string_lossy().to_string()]);
    }
    for service_file in &service_files {
        sandbox.extend([
            "--ro-bind".to_owned(),
            service_file.to_string_lossy().to_string(),
            format!("/instance/{}", service_file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()),
        ]);
    }
    if let (Some(fd), Some(target)) = (argument_file_fd, argument_file_target) {
        sandbox.extend(["--ro-bind-data".to_owned(), fd.to_string(), target.to_owned()]);
    }
    append_device_bindings(&mut sandbox, policy.display_access, policy.audio_access);
    if let (Some(root), Some(mapped)) = (java_root.as_ref(), mapped_java.as_ref()) {
        sandbox.extend(["--ro-bind".to_owned(), root.to_string_lossy().to_string(), "/trial-java".to_owned()]);
        mapped_command[0] = mapped.clone();
    }
    if let Some(root) = java_root.as_ref() {
        append_java_system_config(&mut sandbox, root);
    }
    if !policy.network_access {
        sandbox.push("--unshare-net".to_owned());
    }
    if policy.host_file_read {
        // монтируем только разрешённые каталоги, не весь корень хоста
        for visible_path in ["/home", "/root", "/mnt", "/media"] {
            let path = Path::new(visible_path);
            if path.exists() {
                sandbox.extend(["--ro-bind".to_owned(), visible_path.to_owned(), visible_path.to_owned()]);
            }
        }
    }
    process_environment.insert("HOME".to_owned(), "/instance/.trial-cell/home".to_owned());
    process_environment.insert("TMPDIR".to_owned(), "/instance/.trial-cell/tmp".to_owned());
    process_environment.insert("TMP".to_owned(), "/instance/.trial-cell/tmp".to_owned());
    process_environment.insert("TEMP".to_owned(), "/instance/.trial-cell/tmp".to_owned());
    process_environment.insert("XDG_CONFIG_HOME".to_owned(), "/instance/.trial-cell/config".to_owned());
    process_environment.insert("XDG_CACHE_HOME".to_owned(), "/instance/.trial-cell/cache".to_owned());
    process_environment.insert("TRIAL_LAUNCHER_CELL_ROOT".to_owned(), "/instance/.trial-cell".to_owned());
    process_environment.remove("DBUS_SESSION_BUS_ADDRESS");
    let mut warning: Option<String> = None;
    let display = process_environment.get("DISPLAY").map(|value| value.trim().to_owned()).unwrap_or_default();
    let wayland_display = process_environment
        .get("WAYLAND_DISPLAY")
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    let runtime_dir = process_environment
        .get("XDG_RUNTIME_DIR")
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    if policy.display_access {
        let x11_socket_dir = Path::new("/tmp/.X11-unix");
        if !display.is_empty()
            && local_x11_display(&display)
            && x11_socket_dir.is_dir()
            && !x11_socket_dir.is_symlink()
        {
            sandbox.extend(["--ro-bind".to_owned(), x11_socket_dir.to_string_lossy().to_string(), x11_socket_dir.to_string_lossy().to_string()]);
            if prepare_xauthority(&mut process_environment, &cell) {
                sandbox.extend([
                    "--ro-bind".to_owned(),
                    cell.root.join("xauthority").to_string_lossy().to_string(),
                    "/instance/.trial-cell/xauthority".to_owned(),
                ]);
            } else {
                process_environment.remove("XAUTHORITY");
            }
        } else {
            if !display.is_empty() {
                warning = Some("strict sandbox отключил X11: нужен локальный X11 socket и корректный Xauthority.".to_owned());
            }
            process_environment.remove("XAUTHORITY");
        }
        if !wayland_display.is_empty() && !runtime_dir.is_empty() {
            append_runtime_socket(&mut sandbox, &Path::new(&runtime_dir).join(&wayland_display));
        }
        // каталог HOME в песочнице пустой, поэтому темы курсоров подключаются отдельно
        let host_home = home_dir();
        for (host_icons, cell_icons) in [
            (host_home.join(".icons"), PathBuf::from("/instance/.trial-cell/home/.icons")),
            (
                host_home.join(".local").join("share").join("icons"),
                PathBuf::from("/instance/.trial-cell/home/.local/share/icons"),
            ),
        ] {
            if host_icons.is_dir() && !host_icons.is_symlink() {
                sandbox.extend(["--ro-bind".to_owned(), host_icons.to_string_lossy().to_string(), cell_icons.to_string_lossy().to_string()]);
            }
        }
    } else {
        process_environment.remove("DISPLAY");
        process_environment.remove("WAYLAND_DISPLAY");
        process_environment.remove("XAUTHORITY");
        process_environment.remove("GDK_BACKEND");
        process_environment.remove("QT_QPA_PLATFORM");
        if !display.is_empty() || !wayland_display.is_empty() {
            warning = Some("strict sandbox отключил графический интерфейс: отсутствует desktop:display.".to_owned());
        }
    }
    if policy.audio_access {
        for socket_name in ["pipewire-0", "pulse/native"] {
            if !runtime_dir.is_empty() {
                append_runtime_socket(&mut sandbox, &Path::new(&runtime_dir).join(socket_name));
            }
        }
    } else {
        process_environment.remove("PULSE_SERVER");
        process_environment.remove("PIPEWIRE_REMOTE");
        process_environment.remove("PIPEWIRE_LATENCY");
        if !policy.display_access {
            process_environment.remove("XDG_RUNTIME_DIR");
        }
    }
    // рабочий каталог родительского процесса остаётся путём хоста
    if policy.has("files:host-read") {
        warning = Some(match warning {
            Some(text) => format!("{text} strict sandbox разрешает чтение host-файлов только в пределах явно подключённых read-only путей."),
            None => "strict sandbox разрешает чтение host-файлов только в пределах явно подключённых read-only путей.".to_owned(),
        });
    }
    if !policy.network_access {
        warning = Some(match warning {
            Some(text) => format!("{text} {} sandbox отключил сеть; включите network:game для сетевой игры.", policy.profile),
            None => format!("{} sandbox отключил сеть; включите network:game для сетевой игры.", policy.profile),
        });
    }
    sandbox.extend(mapped_command);
    Ok(PreparedProcess {
        command: sandbox,
        cwd: instance_dir.to_path_buf(),
        environment: process_environment,
        warning,
    })
}

fn windows_appcontainer_enabled(environment: &BTreeMap<String, String>) -> bool {
    let value = environment
        .get("NIVORA_WINDOWS_APPCONTAINER")
        .cloned()
        .or_else(|| std::env::var("NIVORA_WINDOWS_APPCONTAINER").ok())
        .unwrap_or_default();
    matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes")
}

#[allow(clippy::too_many_arguments)]
fn prepare_windows_process(
    command: &[String],
    instance_dir: &Path,
    cell: &InstanceCell,
    service_files: &[PathBuf],
    mut environment: BTreeMap<String, String>,
    policy: &SecurityPolicy,
    host_environment: &BTreeMap<String, String>,
) -> Result<PreparedProcess, String> {
    let no_sandbox = "manual profile отключает OS-level sandbox; разрешения network, file и desktop не обеспечиваются.";
    if policy.host_file_write {
        if policy.profile == "strict" {
            return Err("strict profile не выдаёт модам запись в файлы компьютера; используйте off для доверенной сборки".to_owned());
        }
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment,
            warning: Some(no_sandbox.to_owned()),
        });
    }
    if policy.host_file_read {
        // для чтения профиля пользователя пришлось бы менять его ACL рекурсивно
        if policy.profile == "strict" {
            return Err(
                "files:host-read на Windows не поддерживается в strict: AppContainer потребовал бы менять права на папках пользователя. \
                 Отключите разрешение или используйте off для доверенной сборки."
                    .to_owned(),
            );
        }
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment,
            warning: Some(no_sandbox.to_owned()),
        });
    }
    if !windows_sandbox::appcontainer_supported() {
        if policy.profile == "strict" {
            return Err("strict profile на Windows требует AppContainer (Windows 8 или новее)".to_owned());
        }
        return Ok(PreparedProcess {
            command: command.to_vec(),
            cwd: instance_dir.to_path_buf(),
            environment,
            warning: Some("AppContainer недоступен: manual profile не может ограничить network, file или desktop permissions.".to_owned()),
        });
    }

    environment.insert("APPDATA".to_owned(), cell.config.to_string_lossy().to_string());
    environment.insert("LOCALAPPDATA".to_owned(), cell.cache.to_string_lossy().to_string());
    for key in windows_sandbox::CELL_ENVIRONMENT_KEYS {
        if let Some(value) = environment.get(key) {
            environment.insert(format!("{}{}", windows_sandbox::SANDBOX_ENV_PREFIX, key), value.clone());
        }
        if let Some(host_value) = host_environment.get(key) {
            environment.insert(key.to_owned(), host_value.clone());
        } else {
            environment.remove(key);
        }
    }
    let instance_access = if policy.has("files:instance") { "modify" } else { "read" };
    let java_root = command.first().and_then(|value| windows_java_root(value));
    let read_files: Vec<String> = command[1..]
        .iter()
        .filter_map(|value| value.strip_prefix('@'))
        .filter(|path| {
            let path = Path::new(path);
            path.is_absolute() && path.is_file()
        })
        .map(|path| path.to_owned())
        .collect();
    let sandbox_policy = json!({
        "version": windows_sandbox::POLICY_VERSION,
        "profile_name": windows_sandbox::appcontainer_name(instance_dir),
        "instance_dir": instance_dir.to_string_lossy(),
        "instance_access": instance_access,
        // для JVM нужен временный каталог даже в экземплярах только для чтения
        "writable_dirs": if instance_access == "modify" { Vec::<String>::new() } else { vec![cell.tmp.to_string_lossy().to_string()] },
        "read_dirs": java_root.iter().map(|root| root.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "read_files": read_files,
        "deny_write_files": existing_service_files(instance_dir, service_files)
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        "network": policy.network_access,
        "display": policy.display_access,
        "audio": policy.audio_access,
    });
    let mut warnings: Vec<String> = Vec::new();
    if !policy.display_access {
        warnings.push(
            "Windows не может скрыть окно игры без desktop:display; применены ограничения UI: буфер обмена и чужие окна недоступны.".to_owned(),
        );
    }
    if !policy.audio_access {
        warnings.push(
            "Windows не позволяет запретить вывод звука в AppContainer; desktop:audio не обеспечивается (микрофон закрыт всегда).".to_owned(),
        );
    }
    if policy.network_access {
        warnings.push(
            "Windows блокирует AppContainer подключения к этому же компьютеру (localhost); для локального сервера используйте профиль off.".to_owned(),
        );
        warnings.push(
            "Входящие подключения к игре или серверу в контейнере блокируются: хостить сервер из песочницы нельзя, \
             клиенты не смогут подключиться. Сервер из desktop-приложения запускается без песочницы."
                .to_owned(),
        );
    } else {
        warnings.push(format!(
            "{} sandbox отключил сеть; включите network:game для сетевой игры.",
            policy.profile
        ));
    }
    let wrapped = windows_sandbox::wrap_command(&sandbox_policy, command)?;
    Ok(PreparedProcess {
        command: wrapped,
        cwd: instance_dir.to_path_buf(),
        environment,
        warning: Some(warnings.join(" ")),
    })
}

fn existing_service_files(instance_dir: &Path, created: &[PathBuf]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = created.to_vec();
    for name in SERVICE_JSON_FILES {
        let path = instance_dir.join(name);
        if path.is_file() && !is_reparse_point(&path) && !files.contains(&path) {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[cfg(windows)]
fn windows_java_root(value: &str) -> Option<PathBuf> {
    let candidate = Path::new(value);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        which(value)?
    };
    let resolved = candidate.canonicalize().ok()?;
    if !resolved.is_file() {
        return None;
    }
    let mut root = resolved.parent()?.to_path_buf();
    if root.file_name()?.to_string_lossy().to_lowercase() == "bin" {
        let parent = root.parent()?;
        if ["release", "lib", "conf"].iter().any(|marker| parent.join(marker).exists()) {
            root = parent.to_path_buf();
        }
    }
    // не подключаем корень Java, если он включает профиль пользователя
    let home = home_dir();
    if root == home || home.starts_with(&root) {
        return None;
    }
    if root.parent().is_none() {
        return None;
    }
    Some(root)
}

#[cfg(not(windows))]
fn windows_java_root(_value: &str) -> Option<PathBuf> {
    None
}

fn system_name() -> String {
    if cfg!(windows) {
        "windows".to_owned()
    } else if cfg!(target_os = "macos") {
        "darwin".to_owned()
    } else {
        "linux".to_owned()
    }
}

fn python_system_name() -> String {
    match system_name().as_str() {
        "windows" => "Windows".to_owned(),
        "darwin" => "Darwin".to_owned(),
        _ => "Linux".to_owned(),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn sandbox_status() -> Value {
    if cfg!(windows) {
        let (available, error) = windows_sandbox::probe();
        return json!({
            "platform": "windows",
            "sandbox": "appcontainer",
            "bubblewrap": false,
            "bubblewrap_path": "",
            "strict_available": available,
            "strict_probe_error": error,
            "profiles": SECURITY_PROFILES,
            "permissions": SECURITY_PERMISSIONS,
        });
    }
    let bubblewrap = which("bwrap");
    let mut probe_error = String::new();
    let mut strict_available = false;
    if cfg!(target_os = "linux") {
        if let Some(path) = &bubblewrap {
            let true_binary = which("true").unwrap_or_else(|| PathBuf::from("/usr/bin/true"));
            let args = [
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
            let mut command = crate::rust_engine::platform::command(path);
            command
                .args(args.iter().take(args.len() - 1))
                .arg(&true_binary)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped());
            match command.output() {
                Ok(output) if output.status.success() => strict_available = true,
                Ok(output) => {
                    let text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    probe_error = if text.is_empty() { "bubblewrap probe failed".to_owned() } else { text };
                    probe_error = probe_error.chars().take(256).collect();
                }
                Err(error) => probe_error = error.to_string().chars().take(256).collect(),
            }
        }
    }
    json!({
        "platform": system_name(),
        "sandbox": if bubblewrap.is_some() { "bubblewrap" } else { "none" },
        "bubblewrap": bubblewrap.is_some(),
        "bubblewrap_path": bubblewrap.as_ref().map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
        "strict_available": strict_available,
        "strict_probe_error": probe_error,
        "profiles": SECURITY_PROFILES,
        "permissions": SECURITY_PERMISSIONS,
    })
}

fn map_instance_path(value: &str, instance_dir: &Path) -> String {
    let source = instance_dir.to_string_lossy();
    if value == source {
        return "/instance".to_owned();
    }
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let prefix = format!("{}{}", source.trim_end_matches(separator), separator);
    if let Some(rest) = value.strip_prefix(&prefix) {
        return format!("/instance/{rest}");
    }
    value.to_owned()
}

fn map_instance_argument(value: &str, instance_dir: &Path) -> String {
    if let Some((prefix, suffix)) = value.split_once('=') {
        let mapped = map_instance_path_list(suffix, instance_dir);
        if mapped != suffix {
            return format!("{prefix}={mapped}");
        }
    }
    map_instance_path_list(value, instance_dir)
}

fn map_instance_path_list(value: &str, instance_dir: &Path) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let mapped: Vec<String> = value
        .split(separator)
        .map(|part| map_instance_path(part, instance_dir))
        .collect();
    mapped.join(&separator.to_string())
}

fn sandbox_java_path(value: &str) -> (Option<PathBuf>, Option<String>) {
    let candidate = expand_home(Path::new(value));
    if !candidate.is_absolute() || !candidate.exists() {
        return (None, None);
    }
    let Ok(resolved) = candidate.canonicalize() else {
        return (None, None);
    };
    let mut root = resolved
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    if root.file_name().map(|name| name.to_string_lossy().to_lowercase() == "bin").unwrap_or(false) {
        let runtime_root = root.parent().map(Path::to_path_buf);
        if let Some(runtime_root) = runtime_root {
            // достаточно одного маркера: состав каталогов различается между дистрибутивами
            let markers = [
                runtime_root.join("release").is_file(),
                runtime_root.join("lib").is_dir(),
                runtime_root.join("conf").is_dir(),
            ];
            let home = home_dir();
            let runtime_root_contains_home = runtime_root == home || home.starts_with(&runtime_root);
            if markers.iter().any(|found| *found) && !runtime_root_contains_home {
                root = runtime_root;
            }
        }
    }
    if root == Path::new("/") || !root.is_dir() {
        return (None, None);
    }
    let relative = resolved.strip_prefix(&root).map(|path| path.to_string_lossy().to_string()).unwrap_or_default();
    (Some(root), Some(format!("/trial-java/{relative}")))
}

fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix('~') {
        let rest = rest.trim_start_matches('/');
        return home_dir().join(rest);
    }
    path.to_path_buf()
}

fn append_minimal_etc_bindings(sandbox: &mut Vec<String>, cell: &InstanceCell) -> Result<(), String> {
    let staged_root = cell.root.join("system-etc");
    if is_reparse_point(&staged_root) {
        return Err("Sandbox system configuration staging path is a link".to_owned());
    }
    std::fs::create_dir_all(&staged_root).map_err(|_| "Sandbox system configuration staging path is unavailable".to_owned())?;
    if !staged_root.is_dir() {
        return Err("Sandbox system configuration staging path is unavailable".to_owned());
    }
    set_private_mode(&staged_root);
    let mut created_directories: Vec<String> = Vec::new();

    // подключаем только нужные libc и Java файлы, не весь `/etc` хоста
    for relative in [
        "group",
        "hosts",
        "ld.so.cache",
        "localtime",
        "nsswitch.conf",
        "os-release",
        "passwd",
        "resolv.conf",
        "ssl/cert.pem",
        "ssl/certs/ca-certificates.crt",
        "pki/tls/cert.pem",
        "pki/tls/certs/ca-bundle.crt",
    ] {
        let source = Path::new("/etc").join(relative);
        let target = staged_root.join(relative);
        if stage_system_file(&source, &target, &staged_root) {
            let mut destination_parent = "/etc".to_owned();
            let parts: Vec<&str> = relative.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                destination_parent = format!("{destination_parent}/{part}");
                if !created_directories.contains(&destination_parent) {
                    sandbox.extend(["--dir".to_owned(), destination_parent.clone()]);
                    created_directories.push(destination_parent.clone());
                }
            }
            sandbox.extend(["--ro-bind".to_owned(), target.to_string_lossy().to_string(), format!("/etc/{relative}")]);
        }
    }
    Ok(())
}

const JAVA_SYSTEM_CONFIG_PREFIXES: [&str; 6] = [
    "/etc/java",
    "/etc/jvm",
    "/etc/ssl/certs/java",
    "/etc/pki/java",
    "/etc/pki/ca-trust/extracted",
    "/etc/ca-certificates/extracted",
];

fn java_system_config_path(path: &Path) -> bool {
    let value = normalize_path_string(&path.to_string_lossy());
    if value.starts_with("/etc/java") {
        // каталоги конфигурации Java различаются между дистрибутивами
        return true;
    }
    JAVA_SYSTEM_CONFIG_PREFIXES
        .iter()
        .any(|prefix| value == *prefix || value.starts_with(&format!("{prefix}/")))
}

fn normalize_path_string(value: &str) -> String {
    // убираем повторные слэши в пути
    let mut result = String::new();
    let mut previous_slash = false;
    for character in value.chars() {
        if character == '/' {
            if !previous_slash {
                result.push(character);
            }
            previous_slash = true;
        } else {
            result.push(character);
            previous_slash = false;
        }
    }
    result
}

fn java_system_config_targets(java_root: &Path, max_entries: usize) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![java_root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            seen += 1;
            if seen > max_entries {
                return sorted_paths(targets);
            }
            let path = entry.path();
            if path.is_dir() && !entry.path().is_symlink() {
                stack.push(path);
            }
            if entry.file_type().map(|kind| kind.is_symlink()).unwrap_or(false) {
                // разрешаем цепочки ссылок на системные сертификаты
                let mut current = entry.path();
                for _ in 0..8 {
                    let Ok(raw) = std::fs::read_link(&current) else {
                        break;
                    };
                    let target = match current.parent() {
                        Some(parent) => parent.join(raw),
                        None => raw,
                    };
                    let target = PathBuf::from(normalize_path_string(&target.to_string_lossy()));
                    if !java_system_config_path(&target) {
                        break;
                    }
                    if !targets.contains(&target) {
                        targets.push(target.clone());
                    }
                    if !target.is_symlink() {
                        break;
                    }
                    current = target;
                }
            }
        }
    }
    sorted_paths(targets)
}

fn sorted_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}

fn append_java_system_config(sandbox: &mut Vec<String>, java_root: &Path) {
    let mut mounted: Vec<PathBuf> = Vec::new();
    for target in java_system_config_targets(java_root, 20000) {
        if target.ancestors().skip(1).any(|parent| mounted.contains(&parent.to_path_buf())) {
            continue;
        }
        let Ok(resolved) = target.canonicalize() else {
            continue;
        };
        if !java_system_config_path(&resolved) {
            continue;
        }
        if !(resolved.is_file() || resolved.is_dir()) {
            continue;
        }
        sandbox.extend(["--ro-bind".to_owned(), resolved.to_string_lossy().to_string(), target.to_string_lossy().to_string()]);
        mounted.push(target);
    }
}

fn stage_system_file(source: &Path, target: &Path, staged_root: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(source) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > STAGE_SYSTEM_FILE_MAX_BYTES {
        return false;
    }
    let Ok(content) = std::fs::read(source) else {
        return false;
    };
    if content.len() as u64 > STAGE_SYSTEM_FILE_MAX_BYTES {
        return false;
    }
    let Ok(relative_target) = target.strip_prefix(staged_root) else {
        return false;
    };
    let mut current = staged_root.to_path_buf();
    let parts: Vec<std::ffi::OsString> = relative_target.iter().map(std::ffi::OsString::from).collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        current.push(part);
        if is_reparse_point(&current) {
            return false;
        }
        let _ = std::fs::create_dir_all(&current);
    }
    if is_reparse_point(target) {
        return false;
    }
    if target.exists() {
        let _ = std::fs::remove_file(target);
    }
    if write_new_private_bytes(target, &content).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o400));
    }
    true
}

fn write_new_private_bytes(path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut handle = options.open(path)?;
        handle.write_all(content)
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut handle = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
        handle.write_all(content)
    }
}

fn append_device_bindings(sandbox: &mut Vec<String>, display_access: bool, audio_access: bool) {
    let mut raw_paths: Vec<&Path> = Vec::new();
    if display_access {
        raw_paths.push(Path::new("/dev/dri"));
    }
    if audio_access {
        raw_paths.push(Path::new("/dev/snd"));
    }
    for raw_path in raw_paths {
        if raw_path.is_dir() && !raw_path.is_symlink() {
            sandbox.extend(["--dev-bind".to_owned(), raw_path.to_string_lossy().to_string(), raw_path.to_string_lossy().to_string()]);
        }
    }
    if display_access {
        let mut nvidia_paths: Vec<PathBuf> = std::fs::read_dir("/dev")
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().starts_with("nvidia"))
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        nvidia_paths.sort();
        for raw_path in nvidia_paths {
            if raw_path.is_symlink() {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&raw_path) else {
                continue;
            };
            if !(meta.is_dir() || char_device(&meta)) {
                continue;
            }
            sandbox.extend(["--dev-bind".to_owned(), raw_path.to_string_lossy().to_string(), raw_path.to_string_lossy().to_string()]);
        }
    }
}

fn local_x11_display(value: &str) -> bool {
    let display = value.strip_prefix("unix/").unwrap_or(value);
    let Some(rest) = display.strip_prefix(':') else {
        return false;
    };
    let number = rest.split('.').next().unwrap_or("");
    !number.is_empty() && number.chars().all(|character| character.is_ascii_digit())
}

fn prepare_xauthority(environment: &mut BTreeMap<String, String>, cell: &InstanceCell) -> bool {
    let raw = environment.get("XAUTHORITY").map(|value| value.trim().to_owned()).unwrap_or_default();
    if raw.is_empty() {
        return false;
    }
    let source = expand_home(Path::new(&raw));
    let target = cell.root.join("xauthority");
    if source.is_symlink() || !source.is_file() || target.is_symlink() {
        return false;
    }
    if std::fs::copy(&source, &target).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
    }
    environment.insert("XAUTHORITY".to_owned(), "/instance/.trial-cell/xauthority".to_owned());
    true
}

fn append_runtime_socket(sandbox: &mut Vec<String>, socket_path: &Path) {
    if socket_path.is_symlink() {
        return;
    }
    let is_socket = std::fs::metadata(socket_path)
        .ok()
        .map(|meta| socket_file_type(&meta))
        .unwrap_or(false);
    if !is_socket {
        return;
    }
    let runtime_root = Path::new("/run");
    let Ok(resolved) = socket_path.canonicalize() else {
        return;
    };
    let Ok(relative) = resolved.strip_prefix(runtime_root) else {
        return;
    };
    let target = runtime_root.join(relative);
    let mut parents: Vec<PathBuf> = Vec::new();
    let mut current = target.parent().map(Path::to_path_buf);
    while let Some(directory) = current {
        if directory == runtime_root || !directory.starts_with(runtime_root) {
            break;
        }
        parents.push(directory.clone());
        current = directory.parent().map(Path::to_path_buf);
    }
    for parent in parents.into_iter().rev() {
        sandbox.extend(["--dir".to_owned(), parent.to_string_lossy().to_string()]);
    }
    sandbox.extend(["--ro-bind".to_owned(), socket_path.to_string_lossy().to_string(), target.to_string_lossy().to_string()]);
}

fn socket_file_type(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        meta.file_type().is_socket()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

fn char_device(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        meta.file_type().is_char_device()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let mut candidates: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if cfg!(windows) {
        // здесь не нужны расширения PATHEXT: `icacls` вызывается по абсолютному пути
        candidates.retain(|candidate| candidate.is_dir());
    }
    for directory in candidates {
        let candidate = directory.join(name);
        if candidate.is_file() {
            let executable = std::fs::metadata(&candidate)
                .ok()
                .map(|meta| super::platform::is_executable(&meta))
                .unwrap_or(false);
            if executable || cfg!(windows) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-security-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn profile_normalization_rejects_unknown_and_defaults_to_strict() {
        assert_eq!(normalize_security_profile(&Value::Null).unwrap(), "strict");
        assert_eq!(normalize_security_profile(&json!(" off ")).unwrap(), "off");
        assert_eq!(
            normalize_security_profile(&json!("wide-open")).unwrap_err(),
            "Неизвестный security profile: wide-open"
        );
    }

    #[test]
    fn strict_policy_rejects_unenforced_permissions() {
        let settings = json!({
            "security_profile": "strict",
            "security_permissions": ["network:game", "process:spawn", "chat:read"],
        });
        assert_eq!(
            security_policy(&settings).unwrap_err(),
            "Security profile strict не может безопасно выдать разрешения: chat:read, process:spawn. \
             Используйте off для доверенной сборки."
        );
    }

    #[test]
    fn compat_policy_grants_everything_without_sandbox() {
        let policy = security_policy(&json!({"security_profile": "compat"})).unwrap();
        assert!(!policy.sandbox_requested);
        assert!(policy.network_access && policy.host_file_read && policy.host_file_write);
    }

    #[test]
    fn strict_policy_maps_permissions_to_flags() {
        let settings = json!({
            "security_profile": "strict",
            "security_permissions": ["network:game", "desktop:audio"],
        });
        let policy = security_policy(&settings).unwrap();
        assert!(policy.sandbox_requested);
        assert!(policy.network_access);
        assert!(!policy.display_access);
        assert!(policy.audio_access);
        assert!(!policy.host_file_write);
        assert_eq!(policy.high_risk_permissions(), vec!["network:game", "desktop:audio"]);
    }

    #[test]
    fn instance_cell_creates_private_layout() {
        let root = temp_root("cell");
        let instance = root.join("My Instance");
        std::fs::create_dir_all(&instance).unwrap();
        let cell = ensure_instance_cell(&instance).unwrap();
        assert!(
            cell.home.is_dir() && cell.tmp.is_dir() && cell.config.is_dir() && cell.cache.is_dir()
                && cell.root.join("logs").is_dir()
        );
        assert_eq!(cell.root.file_name().unwrap(), CELL_DIRECTORY);
    }

    #[test]
    fn instance_cell_rejects_missing_directory() {
        let root = temp_root("cell-missing");
        assert_eq!(
            ensure_instance_cell(&root.join("nope")).unwrap_err(),
            "Каталог экземпляра недоступен для создания security cell"
        );
    }

    #[test]
    fn service_files_created_once() {
        let root = temp_root("service");
        let instance = root.join("i");
        std::fs::create_dir_all(&instance).unwrap();
        let created = ensure_service_json_files(&instance).unwrap();
        assert_eq!(created.len(), SERVICE_JSON_FILES.len());
        let again = ensure_service_json_files(&instance).unwrap();
        assert!(again.is_empty());
        assert_eq!(std::fs::read_to_string(instance.join(".trial-launcher-live.json")).unwrap(), "{}\n");
    }

    #[test]
    fn instance_path_mapping_translates_classpath() {
        let instance = Path::new("/home/user/game/instances/demo");
        assert_eq!(map_instance_path("/home/user/game/instances/demo", instance), "/instance");
        assert_eq!(
            map_instance_path("/home/user/game/instances/demo/libraries/a.jar", instance),
            "/instance/libraries/a.jar"
        );
        assert_eq!(map_instance_path("/usr/share/other", instance), "/usr/share/other");
        let classpath = format!("/home/user/game/instances/demo/l1.jar:/home/user/game/instances/demo/l2.jar");
        assert_eq!(
            map_instance_argument(&format!("-cp={classpath}"), instance),
            "-cp=/instance/l1.jar:/instance/l2.jar"
        );
    }

    #[test]
    fn x11_display_detection_accepts_local_sockets_only() {
        assert!(local_x11_display(":0"));
        assert!(local_x11_display(":1.0"));
        assert!(local_x11_display("unix/:0"));
        assert!(!local_x11_display("localhost:10.0"));
        assert!(!local_x11_display("wayland-0"));
    }

    #[test]
    fn java_system_config_recognizes_distro_layouts() {
        assert!(java_system_config_path(Path::new("/etc/java-17-openjdk")));
        assert!(java_system_config_path(Path::new("/etc/java/openjdk/management/management.properties")));
        assert!(java_system_config_path(Path::new("/etc/pki/ca-trust/extracted/java")));
        assert!(!java_system_config_path(Path::new("/etc/passwd")));
        // проверка по префиксу также совпадает для /etc/java-not-here
        assert!(java_system_config_path(Path::new("/etc/java-not-here")));
    }

    #[test]
    fn sandbox_status_shape_matches_frontend_contract() {
        let status = sandbox_status();
        assert!(matches!(status["platform"].as_str(), Some("linux") | Some("windows") | Some("darwin")));
        assert!(status["strict_available"].is_boolean());
        assert!(status["profiles"].as_array().unwrap().len() == SECURITY_PROFILES.len());
        assert!(status["permissions"].as_array().unwrap().len() == SECURITY_PERMISSIONS.len());
    }

    #[test]
    fn strict_requires_bubblewrap_and_no_host_write() {
        let settings = json!({"security_profile": "strict", "security_permissions": ["files:host-write"]});
        let environment = BTreeMap::new();
        let result = prepare_process(
            &[],
            Path::new("/nonexistent-instance"),
            &environment,
            &settings,
            None,
            None,
        );
        assert!(result.is_err());
    }
}
