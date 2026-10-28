// appcontainer и Job Object для Windows
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const POLICY_VERSION: i64 = 1;
pub(crate) const HELPER_FLAG: &str = "--win-sandbox";
pub(crate) const PROFILE_PREFIX: &str = "Nivora.Cell.";
pub(crate) const PROBE_PROFILE: &str = "Nivora.Probe";
pub(crate) const MAX_POLICY_PATHS: usize = 64;
pub(crate) const MAX_POLICY_BYTES: usize = 64 * 1024;
pub(crate) const SANDBOX_ENV_PREFIX: &str = "NIVORA_SANDBOX_ENV_";
/// переменные окружения, переопределяемые для игры в контейнере
pub(crate) const CELL_ENVIRONMENT_KEYS: [&str; 8] = [
    "HOME", "TMPDIR", "TMP", "TEMP", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA",
];

// сетевые SID из winnt.h
pub(crate) const CAPABILITY_INTERNET_CLIENT: &str = "S-1-15-3-1";
pub(crate) const CAPABILITY_INTERNET_CLIENT_SERVER: &str = "S-1-15-3-2";
pub(crate) const CAPABILITY_PRIVATE_NETWORK_CLIENT_SERVER: &str = "S-1-15-3-3";

// флаги ограничений интерфейса Job Object
pub(crate) const UILIMIT_HANDLES: u32 = 0x0001;
pub(crate) const UILIMIT_READCLIPBOARD: u32 = 0x0002;
pub(crate) const UILIMIT_WRITECLIPBOARD: u32 = 0x0004;
pub(crate) const UILIMIT_SYSTEMPARAMETERS: u32 = 0x0008;
pub(crate) const UILIMIT_DISPLAYSETTINGS: u32 = 0x0010;
pub(crate) const UILIMIT_GLOBALATOMS: u32 = 0x0020;
pub(crate) const UILIMIT_DESKTOP: u32 = 0x0040;
pub(crate) const UILIMIT_EXITWINDOWS: u32 = 0x0080;

pub(crate) const DENY_WRITE_RIGHTS: &str = "(WD,AD,WEA,WA,DE,WDAC,WO)";

/// формирует имя профиля не длиннее лимита Windows в 64 символа
pub(crate) fn appcontainer_name(instance_dir: &Path) -> String {
    let normalized = normalize_windows_path(instance_dir);
    let digest = Sha256::digest(normalized.as_bytes());
    format!("{PROFILE_PREFIX}{}", &format!("{:x}", digest)[..32])
}

/// нормализует путь: заменяет слэши и приводит его к нижнему регистру
fn normalize_windows_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let absolute = if text.len() >= 2 && text.as_bytes()[1] == b':' {
        text
    } else {
        // относительные пути получают общий префикс для стабильного имени
        format!(".\\{text}")
    };
    absolute.to_lowercase()
}

pub(crate) fn ui_restrictions(display_access: bool) -> u32 {
    let mut restrictions =
        UILIMIT_DESKTOP | UILIMIT_EXITWINDOWS | UILIMIT_SYSTEMPARAMETERS | UILIMIT_GLOBALATOMS;
    if !display_access {
        restrictions |= UILIMIT_HANDLES | UILIMIT_READCLIPBOARD | UILIMIT_WRITECLIPBOARD | UILIMIT_DISPLAYSETTINGS;
    }
    restrictions
}

pub(crate) fn network_capabilities(network_access: bool) -> Vec<&'static str> {
    if !network_access {
        return Vec::new();
    }
    vec![
        CAPABILITY_INTERNET_CLIENT,
        CAPABILITY_INTERNET_CLIENT_SERVER,
        CAPABILITY_PRIVATE_NETWORK_CLIENT_SERVER,
    ]
}

pub(crate) fn encode_policy(policy: &Value) -> Result<String, String> {
    let normalized = validate_policy(policy)?;
    let raw = serde_json::to_string(&normalized).map_err(|error| format!("Политика песочницы повреждена: {error}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE.encode(raw.as_bytes()))
}

pub(crate) fn decode_policy(value: &str) -> Result<Value, String> {
    if value.len() > MAX_POLICY_BYTES * 2 {
        return Err("Политика песочницы слишком большая".to_owned());
    }
    let raw = base64::engine::general_purpose::URL_SAFE
        .decode(value.as_bytes())
        .map_err(|_| "Политика песочницы повреждена".to_owned())?;
    let policy: Value = serde_json::from_slice(&raw).map_err(|_| "Политика песочницы повреждена".to_owned())?;
    validate_policy(&policy)
}

fn policy_str_list(policy: &Value, key: &str) -> Result<Vec<String>, String> {
    let Some(values) = policy.get(key).and_then(Value::as_array) else {
        return Err(format!("Некорректный список путей: {key}"));
    };
    if values.len() > MAX_POLICY_PATHS {
        return Err(format!("Некорректный список путей: {key}"));
    }
    let mut result = Vec::with_capacity(values.len());
    for item in values {
        let text = item.as_str().ok_or_else(|| format!("Путь в политике должен быть абсолютным: {key}"))?;
        if !is_absolute_windows_path(text) {
            return Err(format!("Путь в политике должен быть абсолютным: {key}"));
        }
        result.push(text.to_owned());
    }
    Ok(result)
}

fn is_absolute_windows_path(value: &str) -> bool {
    // принимает абсолютные пути Windows с диском или UNC
    let bytes = value.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return true;
    }
    value.starts_with("\\\\")
}

pub(crate) fn validate_policy(policy: &Value) -> Result<Value, String> {
    let Some(object) = policy.as_object() else {
        return Err("Неподдерживаемая политика песочницы".to_owned());
    };
    if object.get("version").and_then(Value::as_i64) != Some(POLICY_VERSION) {
        return Err("Неподдерживаемая политика песочницы".to_owned());
    }
    let name = object
        .get("profile_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Некорректное имя профиля AppContainer".to_owned())?;
    if !name.starts_with(PROFILE_PREFIX) || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        return Err("Некорректное имя профиля AppContainer".to_owned());
    }
    let instance_dir = object
        .get("instance_dir")
        .and_then(Value::as_str)
        .ok_or_else(|| "Папка экземпляра в политике должна быть абсолютным путём".to_owned())?;
    if !is_absolute_windows_path(instance_dir) {
        return Err("Папка экземпляра в политике должна быть абсолютным путём".to_owned());
    }
    let instance_access = object
        .get("instance_access")
        .and_then(Value::as_str)
        .ok_or_else(|| "Некорректный доступ к папке экземпляра".to_owned())?;
    if !matches!(instance_access, "modify" | "read") {
        return Err("Некорректный доступ к папке экземпляра".to_owned());
    }
    let writable_dirs = policy_str_list(policy, "writable_dirs")?;
    let read_dirs = policy_str_list(policy, "read_dirs")?;
    let read_files = policy_str_list(policy, "read_files")?;
    let deny_write_files = policy_str_list(policy, "deny_write_files")?;
    for key in ["network", "display", "audio"] {
        if !matches!(object.get(key), Some(Value::Bool(_))) {
            return Err(format!("Некорректный флаг политики: {key}"));
        }
    }
    Ok(json!({
        "version": POLICY_VERSION,
        "profile_name": name,
        "instance_dir": instance_dir,
        "instance_access": instance_access,
        "writable_dirs": writable_dirs,
        "read_dirs": read_dirs,
        "read_files": read_files,
        "deny_write_files": deny_write_files,
        "network": object["network"],
        "display": object["display"],
        "audio": object["audio"],
    }))
}

/// собирает команду запуска helper-режима песочницы
pub(crate) fn helper_command() -> Result<Vec<String>, String> {
    let executable = std::env::current_exe().map_err(|error| format!("Не удалось определить путь приложения: {error}"))?;
    Ok(vec![executable.to_string_lossy().to_string(), HELPER_FLAG.to_owned()])
}

pub(crate) fn wrap_command(policy: &Value, command: &[String]) -> Result<Vec<String>, String> {
    let mut wrapped = helper_command()?;
    wrapped.push(encode_policy(policy)?);
    wrapped.push("--".to_owned());
    wrapped.extend(command.iter().cloned());
    Ok(wrapped)
}

/// восстанавливает переменные окружения игры из `NIVORA_SANDBOX_ENV_*`
pub(crate) fn child_environment(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut overrides = BTreeMap::new();
    for (key, value) in environment {
        let upper = key.to_uppercase();
        if upper.starts_with("_MEI") || upper.starts_with("_PYI") {
            continue;
        }
        if let Some(target) = key.strip_prefix(SANDBOX_ENV_PREFIX) {
            if !target.is_empty() {
                overrides.insert(target.to_owned(), value.clone());
            }
            continue;
        }
        result.insert(key.clone(), value.clone());
    }
    for (target, value) in overrides {
        // windows-окружение регистронезависимо: совпадения по upper удаляются
        result.retain(|key, _| key.to_uppercase() != target.to_uppercase());
        result.insert(target, value);
    }
    result
}

/// блок окружения для CreateProcessW (UTF-16, отсортировано, двойной \0)
pub(crate) fn environment_block(environment: &BTreeMap<String, String>) -> Vec<u16> {
    let sorted: BTreeMap<&str, &String> = environment
        .iter()
        .filter(|(key, _)| !key.is_empty() && !key.contains('='))
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    let mut block = String::new();
    let mut items: Vec<(&str, &String)> = sorted.into_iter().collect();
    items.sort_by_key(|(key, _)| key.to_uppercase());
    for (key, value) in items {
        block.push_str(key);
        block.push('=');
        block.push_str(value);
        block.push('\0');
    }
    block.push('\0');
    block.encode_utf16().collect()
}

/// метка ACL хранится вне экземпляра, чтобы игра не могла её подменить
pub(crate) fn acl_marker_path(profile_name: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            home.join("AppData").join("Local")
        });
    base.join("nivora-launcher").join("sandbox-acl").join(format!("{profile_name}.json"))
}

pub(crate) fn acl_state(policy: &Value, sid: &str, stamps: BTreeMap<String, i64>) -> Value {
    json!({
        "sid": sid,
        "instance_dir": policy["instance_dir"],
        "instance_access": policy["instance_access"],
        "writable_dirs": sort_strings(policy["writable_dirs"].as_array()),
        "read_dirs": sort_strings(policy["read_dirs"].as_array()),
        // метка сбрасывается, если каталог экземпляра или Java пересозданы
        "stamps": stamps,
    })
}

fn sort_strings(values: Option<&Vec<Value>>) -> Vec<Value> {
    let mut items: Vec<String> = values
        .map(|values| values.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    items.sort();
    items.into_iter().map(Value::String).collect()
}

pub(crate) fn instance_grants(grantee: &str, access: &str) -> Vec<String> {
    if access == "read" {
        return vec![format!("{grantee}:(OI)(CI)(RX)")];
    }
    // запись разрешена внутри экземпляра, но не для его корневого каталога
    vec![
        format!("{grantee}:(OI)(CI)(IO)(M)"),
        format!("{grantee}:(RD,X,RA,REA,RC,S,WD,AD,WA,WEA)"),
    ]
}

pub(crate) fn appcontainer_supported() -> bool {
    #[cfg(windows)]
    {
        // эти функции доступны начиная с Windows 8
        true
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// присутствует ли AppContainer в этой системе и умеет ли запускать процесс
pub(crate) fn probe() -> (bool, String) {
    #[cfg(windows)]
    {
        if !appcontainer_supported() {
            return (false, "AppContainer недоступен: нужна Windows 8 или новее".to_owned());
        }
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
        let cmd = Path::new(&system_root).join("System32").join("cmd.exe");
        let command = vec![
            cmd.to_string_lossy().to_string(),
            "/d".to_owned(),
            "/c".to_owned(),
            "exit 0".to_owned(),
        ];
        match spawn_in_appcontainer(PROBE_PROFILE, &command, false, false, None) {
            Ok(code) if code == 0 => (true, String::new()),
            Ok(code) => (false, format!("пробный процесс в AppContainer завершился с кодом {code}")),
            Err(error) => (false, error.chars().take(256).collect()),
        }
    }
    #[cfg(not(windows))]
    {
        (false, "AppContainer недоступен: нужна Windows 8 или новее".to_owned())
    }
}

/// запускает команду в AppContainer внутри Job Object и ждёт код выхода
#[cfg(windows)]
pub(crate) fn spawn_in_appcontainer(
    profile_name: &str,
    command: &[String],
    network: bool,
    display: bool,
    environment: Option<&BTreeMap<String, String>>,
) -> Result<i32, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::{FreeSid, SID_AND_ATTRIBUTES};
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
    };
    use windows_sys::Win32::System::Console::GetStdHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject,
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS,
        EXTENDED_STARTUPINFO_PRESENT, INFINITE,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        STARTUPINFOEXW, STARTUPINFOW, STARTF_USESTDHANDLES,
    };

    unsafe fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe {
        let name = wide(profile_name);
        let mut container_sid: *mut core::ffi::c_void = std::ptr::null_mut();
        let display_name = wide("Nivora Launcher instance");
        let description = wide("Песочница игрового экземпляра Nivora Launcher");
        let mut result = CreateAppContainerProfile(
            name.as_ptr(),
            display_name.as_ptr(),
            description.as_ptr(),
            std::ptr::null(),
            0,
            &mut container_sid,
        );
        if result == 0x800700B7u32 as i32 {
            // макрос HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS): профиль уже есть
            result = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut container_sid);
        }
        if result != 0 || container_sid.is_null() {
            return Err(format!(
                "Не удалось создать профиль AppContainer {profile_name} (HRESULT 0x{:08X})",
                result as u32 & 0xFFFF_FFFF
            ));
        }

        let mut capability_sids: Vec<*mut core::ffi::c_void> = Vec::new();
        let mut attribute_list: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut job: HANDLE = std::ptr::null_mut();
        let mut spawned = false;
        let mut process_handle: HANDLE = std::ptr::null_mut();
        let mut thread_handle: HANDLE = std::ptr::null_mut();
        let outcome = (|| -> Result<i32, String> {
            for value in network_capabilities(network) {
                let wide_sid = wide(value);
                let mut sid: *mut core::ffi::c_void = std::ptr::null_mut();
                if ConvertStringSidToSidW(wide_sid.as_ptr(), &mut sid) == 0 {
                    return Err(format!("Некорректный SID {value}"));
                }
                capability_sids.push(sid);
            }
            let mut capabilities: Vec<SID_AND_ATTRIBUTES> = capability_sids
                .iter()
                .map(|sid| SID_AND_ATTRIBUTES { Sid: *sid, Attributes: 0x4 /* flag SE_GROUP_ENABLED */ })
                .collect();
            let mut security = windows_sys::Win32::Security::SECURITY_CAPABILITIES {
                AppContainerSid: container_sid,
                Capabilities: capabilities.as_mut_ptr(),
                CapabilityCount: capabilities.len() as u32,
                Reserved: 0,
            };

            let std_handles = [
                GetStdHandle(0xFFFF_FFF6u32 /* handle STD_INPUT_HANDLE */),
                GetStdHandle(0xFFFF_FFF5u32 /* handle STD_OUTPUT_HANDLE */),
                GetStdHandle(0xFFFF_FFF4u32 /* handle STD_ERROR_HANDLE */),
            ];
            let mut handles: Vec<HANDLE> = Vec::new();
            for value in std_handles.into_iter() {
                let invalid = value as isize == -1 || value.is_null();
                if invalid || handles.contains(&value) {
                    continue;
                }
                // флаг HANDLE_FLAG_INHERIT: младший бит набора флагов
                if windows_sys::Win32::Foundation::SetHandleInformation(value, 1, 1) != 0 {
                    handles.push(value);
                }
            }

            let attribute_count = if handles.is_empty() { 1 } else { 2 };
            let mut size: usize = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), attribute_count, 0, &mut size);
            let mut buffer = vec![0u8; size.max(16)];
            attribute_list = buffer.as_mut_ptr() as *mut core::ffi::c_void;
            if InitializeProcThreadAttributeList(attribute_list, attribute_count, 0, &mut size) == 0 {
                return Err("Не удалось подготовить атрибуты процесса".to_owned());
            }
            if UpdateProcThreadAttribute(
                attribute_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &mut security as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of_val(&security),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                return Err("Не удалось назначить AppContainer".to_owned());
            }
            if !handles.is_empty() {
                let mut handle_array: Vec<HANDLE> = handles.clone();
                if UpdateProcThreadAttribute(
                    attribute_list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handle_array.as_mut_ptr() as *mut core::ffi::c_void,
                    std::mem::size_of::<HANDLE>() * handle_array.len(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err("Не удалось ограничить наследуемые дескрипторы".to_owned());
                }
            }

            let mut startup: STARTUPINFOEXW = std::mem::zeroed();
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = attribute_list;
            if !handles.is_empty() {
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                let pick = |handle: HANDLE| -> HANDLE {
                    if handles.contains(&handle) { handle } else { std::ptr::null_mut() }
                };
                startup.StartupInfo.hStdInput = pick(std_handles[0]);
                startup.StartupInfo.hStdOutput = pick(std_handles[1]);
                startup.StartupInfo.hStdError = pick(std_handles[2]);
            }

            job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return Err("Не удалось создать Job Object".to_owned());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation as i32,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err("Не удалось ограничить Job Object".to_owned());
            }

            let mut command_line = wide(&quote_command(command));
            let block = environment_block(environment.unwrap_or(&host_environment()));
            let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | DETACHED_PROCESS;
            #[allow(clippy::missing_transmute_annotations)]
            let mut process_info: windows_sys::Win32::System::Threading::PROCESS_INFORMATION = std::mem::zeroed();
            if CreateProcessW(
                std::ptr::null(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                !handles.is_empty() as i32,
                flags,
                block.as_ptr() as *const core::ffi::c_void,
                std::ptr::null(),
                &startup as *const STARTUPINFOEXW as *const STARTUPINFOW,
                &mut process_info,
            ) == 0
            {
                let code = windows_sys::Win32::Foundation::GetLastError();
                if code == 5 {
                    return Err(
                        "Java недоступна из песочницы (нет доступа). Выберите Java в папке пользователя \
                         или Mojang runtime либо используйте профиль off для доверенной сборки."
                            .to_owned(),
                    );
                }
                return Err(format!("Не удалось запустить процесс в AppContainer (Win32 {code})"));
            }
            spawned = true;
            process_handle = process_info.hProcess;
            thread_handle = process_info.hThread;
            if AssignProcessToJobObject(job, process_info.hProcess) == 0 {
                TerminateProcess(process_info.hProcess, 1);
                return Err("Не удалось поместить процесс в Job Object".to_owned());
            }
            ResumeThread(process_info.hThread);
            WaitForSingleObject(process_info.hProcess, INFINITE);
            let mut exit_code: u32 = 1;
            GetExitCodeProcess(process_info.hProcess, &mut exit_code);
            Ok(exit_code as i32)
        })();

        if !thread_handle.is_null() {
            CloseHandle(thread_handle);
        }
        if !process_handle.is_null() {
            CloseHandle(process_handle);
        }
        let _ = spawned;
        if !attribute_list.is_null() {
            DeleteProcThreadAttributeList(attribute_list);
        }
        if !job.is_null() {
            CloseHandle(job);
        }
        for sid in capability_sids {
            LocalFree(sid as *mut core::ffi::c_void);
        }
        if !container_sid.is_null() {
            FreeSid(container_sid);
        }
        outcome
    }
}

#[cfg(windows)]
fn host_environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn spawn_in_appcontainer(
    _profile_name: &str,
    _command: &[String],
    _network: bool,
    _display: bool,
    _environment: Option<&BTreeMap<String, String>>,
) -> Result<i32, String> {
    Err("Windows-песочница доступна только на Windows".to_owned())
}

/// добавляет кавычки Windows только там, где они нужны
pub(crate) fn quote_command(command: &[String]) -> String {
    let mut parts = Vec::with_capacity(command.len());
    for argument in command {
        let (quotes, mut escaped) = (argument.contains(' ') || argument.contains('\t') || argument.is_empty(), String::new());
        let mut quote_seen = false;
        for character in argument.chars() {
            if character == '"' {
                escaped.push_str("\\\"");
                quote_seen = true;
            } else if character == '\\' {
                escaped.push(character);
            } else {
                escaped.push(character);
            }
        }
        if quotes {
            parts.push(format!("\"{escaped}\""));
        } else {
            let _ = quote_seen;
            parts.push(escaped);
        }
    }
    parts.join(" ")
}

#[cfg(windows)]
pub(crate) fn apply_acl(policy: &Value, sid: &str) -> Result<(), String> {
    let grantee = format!("*{sid}");
    let marker = acl_marker_path(&policy["profile_name"].as_str().unwrap_or_default());
    let instance_dir = policy["instance_dir"].as_str().unwrap_or_default().to_owned();
    let mut directories: Vec<String> = vec![instance_dir.clone()];
    for key in ["writable_dirs", "read_dirs"] {
        for item in policy[key].as_array().map(|values| values.iter()).unwrap_or_default() {
            if let Some(text) = item.as_str() {
                directories.push(text.to_owned());
            }
        }
    }
    let stamps: BTreeMap<String, i64> = directories
        .iter()
        .map(|path| (path.clone(), path_stamp(Path::new(path))))
        .collect();
    let state = acl_state(policy, sid, stamps);
    let previous = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if previous.as_ref() != Some(&state) {
        // сначала удаляем старое правило, иначе режим read сохранит запись
        icacls(&[instance_dir.clone(), "/remove:g".to_owned(), grantee.clone()])?;
        let access = policy["instance_access"].as_str().unwrap_or("read");
        for grant in instance_grants(&grantee, access) {
            icacls(&[instance_dir.clone(), "/grant".to_owned(), grant])?;
        }
        for directory in policy["writable_dirs"].as_array().into_iter().flatten() {
            if let Some(text) = directory.as_str() {
                icacls(&[text.to_owned(), "/grant".to_owned(), format!("{grantee}:(OI)(CI)(M)")])?;
            }
        }
        for directory in policy["read_dirs"].as_array().into_iter().flatten() {
            if let Some(text) = directory.as_str() {
                // установка Java в Program Files уже даёт доступ на чтение без ACL
                let _ = icacls(&[text.to_owned(), "/grant".to_owned(), format!("{grantee}:(OI)(CI)(RX)")]);
            }
        }
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("icacls: {error}"))?;
        }
        std::fs::write(&marker, serde_json::to_string(&state).unwrap_or_default())
            .map_err(|error| format!("icacls: {error}"))?;
    }
    for path in policy["read_files"].as_array().into_iter().flatten() {
        if let Some(text) = path.as_str() {
            icacls(&[text.to_owned(), "/grant".to_owned(), format!("{grantee}:(R)")])?;
        }
    }
    for path in policy["deny_write_files"].as_array().into_iter().flatten() {
        if let Some(text) = path.as_str() {
            icacls(&[text.to_owned(), "/deny".to_owned(), format!("{grantee}:{DENY_WRITE_RIGHTS}")])?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn icacls(arguments: &[String]) -> Result<(), String> {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let icacls_path = Path::new(&system_root).join("System32").join("icacls.exe");
    let mut call: Vec<String> = arguments.to_vec();
    call.push("/Q".to_owned());
    let output = crate::rust_engine::platform::command(&icacls_path)
        .args(&call)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();
    let result = output.map_err(|error| format!("icacls {}: {error}", arguments.first().map(String::as_str).unwrap_or("")))?;
    if !result.status.success() {
        let detail = String::from_utf8_lossy(if result.stderr.is_empty() { &result.stdout } else { &result.stderr });
        let detail: String = detail.trim().chars().take(300).collect();
        return Err(format!(
            "icacls {}: {}",
            arguments.first().map(String::as_str).unwrap_or(""),
            if detail.is_empty() { format!("код {}", result.status.code().unwrap_or_default()) } else { detail }
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn current_user_sid() -> Result<String, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err("Не удалось открыть токен процесса".to_owned());
        }
        let outcome = (|| -> Result<String, String> {
            let mut size: u32 = 0;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
            let mut buffer = vec![0u8; size.max(64) as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                size,
                &mut size,
            ) == 0
            {
                return Err("Не удалось прочитать пользователя токена".to_owned());
            }
            let user = &*(buffer.as_ptr() as *const TOKEN_USER);
            sid_to_string(user.User.Sid)
        })();
        CloseHandle(token);
        outcome
    }
}

#[cfg(windows)]
pub(crate) fn sid_to_string(sid: *mut core::ffi::c_void) -> Result<String, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    unsafe {
        let mut buffer: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut buffer) == 0 {
            return Err("Не удалось прочитать SID".to_owned());
        }
        let mut length = 0usize;
        while *buffer.add(length) != 0 {
            length += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(buffer, length));
        LocalFree(buffer as *mut core::ffi::c_void);
        Ok(text)
    }
}

#[cfg(windows)]
pub(crate) fn appcontainer_sid_string(profile_name: &str) -> Result<String, String> {
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
    };
    use windows_sys::Win32::Security::FreeSid;
    unsafe {
        let name: Vec<u16> = profile_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sid: *mut core::ffi::c_void = std::ptr::null_mut();
        let display = encode_utf16("Nivora Launcher instance");
        let description = encode_utf16("Песочница игрового экземпляра Nivora Launcher");
        let mut result = CreateAppContainerProfile(name.as_ptr(), display.as_ptr(), description.as_ptr(), std::ptr::null(), 0, &mut sid);
        if result == 0x800700B7u32 as i32 {
            result = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid);
        }
        if result != 0 || sid.is_null() {
            return Err(format!("Не удалось создать профиль AppContainer {profile_name}"));
        }
        let text = sid_to_string(sid);
        FreeSid(sid);
        text
    }
}

#[cfg(windows)]
fn encode_utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn path_stamp(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.created().or_else(|_| meta.modified()))
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(not(windows))]
fn path_stamp(_path: &Path) -> i64 {
    0
}

/// точка входа helper-режима до запуска Tauri
pub(crate) fn helper_main(arguments: &[String]) -> i32 {
    if arguments.len() < 3 || arguments[1] != "--" {
        eprintln!("usage: --win-sandbox <policy> -- <command...>");
        return 2;
    }
    let policy = match decode_policy(&arguments[0]) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("Песочница Windows: {error}");
            return 1;
        }
    };
    match run(&policy, &arguments[2..]) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Песочница Windows: {error}");
            1
        }
    }
}

pub(crate) fn run(policy: &Value, command: &[String]) -> Result<i32, String> {
    if command.is_empty() {
        return Err("Не передана команда для песочницы".to_owned());
    }
    #[cfg(windows)]
    {
        let profile = policy["profile_name"].as_str().unwrap_or_default();
        let sid = appcontainer_sid_string(profile)?;
        apply_acl(policy, &sid)?;
        let mut environment: BTreeMap<String, String> = std::env::vars().collect();
        for key in CELL_ENVIRONMENT_KEYS {
            let prefixed = format!("{SANDBOX_ENV_PREFIX}{key}");
            if let Some(value) = std::env::var(&prefixed).ok() {
                environment.insert(key.to_owned(), value);
            }
        }
        Ok(spawn_in_appcontainer(
            profile,
            command,
            policy["network"].as_bool().unwrap_or(false),
            policy["display"].as_bool().unwrap_or(false),
            Some(&environment),
        )?)
    }
    #[cfg(not(windows))]
    {
        let _ = (policy, command);
        Err("Windows-песочница доступна только на Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_policy() -> Value {
        json!({
            "version": POLICY_VERSION,
            "profile_name": format!("{PROFILE_PREFIX}abc123"),
            "instance_dir": "C:\\Games\\instance",
            "instance_access": "modify",
            "writable_dirs": [],
            "read_dirs": ["C:\\Program Files\\Java"],
            "read_files": [],
            "deny_write_files": ["C:\\Games\\instance\\.nivora-launcher-server.json"],
            "network": true,
            "display": true,
            "audio": false,
        })
    }

    #[test]
    fn policy_roundtrip_preserves_normalized_shape() {
        let encoded = encode_policy(&sample_policy()).expect("закодировать политику");
        let decoded = decode_policy(&encoded).expect("разобрать политику");
        assert_eq!(decoded, validate_policy(&sample_policy()).unwrap());
        assert!(decoded["writable_dirs"].is_array());
    }

    #[test]
    fn policy_rejects_foreign_profile_names_and_old_versions() {
        let mut policy = sample_policy();
        policy["profile_name"] = json!("Evil.Cell.123");
        assert_eq!(validate_policy(&policy).unwrap_err(), "Некорректное имя профиля AppContainer");
        let mut old = sample_policy();
        old["version"] = json!(0);
        assert_eq!(validate_policy(&old).unwrap_err(), "Неподдерживаемая политика песочницы");
        let mut relative = sample_policy();
        relative["instance_dir"] = json!("games\\instance");
        assert_eq!(
            validate_policy(&relative).unwrap_err(),
            "Папка экземпляра в политике должна быть абсолютным путём"
        );
    }

    #[test]
    fn policy_caps_path_lists() {
        let mut policy = sample_policy();
        let many: Vec<Value> = (0..MAX_POLICY_PATHS + 1).map(|index| json!(format!("C:\\dir{index}"))).collect();
        policy["read_files"] = Value::Array(many);
        assert_eq!(validate_policy(&policy).unwrap_err(), "Некорректный список путей: read_files");
    }

    #[test]
    fn appcontainer_name_is_stable_and_bounded() {
        let first = appcontainer_name(Path::new("C:\\Games\\My Instance"));
        let second = appcontainer_name(Path::new("c:/games/my instance"));
        assert_eq!(first, second);
        assert!(first.starts_with(PROFILE_PREFIX));
        assert!(first.len() <= 64);
    }

    #[test]
    fn child_environment_moves_prefixed_keys_over_host_values() {
        let mut environment = BTreeMap::new();
        environment.insert("HOME".to_owned(), "C:\\Users\\host".to_owned());
        environment.insert("NIVORA_SANDBOX_ENV_HOME".to_owned(), "C:\\cell\\home".to_owned());
        environment.insert("_MEIPASS1234".to_owned(), "junk".to_owned());
        let child = child_environment(&environment);
        assert_eq!(child.get("HOME").map(String::as_str), Some("C:\\cell\\home"));
        assert!(!child.contains_key("_MEIPASS1234"));
    }

    #[test]
    fn environment_block_is_sorted_and_double_terminated() {
        let mut environment = BTreeMap::new();
        environment.insert("B".to_owned(), "2".to_owned());
        environment.insert("A".to_owned(), "1".to_owned());
        let block = environment_block(&environment);
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "A=1\0B=2\0\0");
    }

    #[test]
    fn ui_restrictions_tighten_without_display() {
        let with_display = ui_restrictions(true);
        let without_display = ui_restrictions(false);
        assert_eq!(with_display & UILIMIT_READCLIPBOARD, 0);
        assert_ne!(without_display & UILIMIT_READCLIPBOARD, 0);
    }

    #[test]
    fn instance_grants_differ_between_read_and_modify() {
        assert_eq!(instance_grants("*S-1", "read"), vec!["*S-1:(OI)(CI)(RX)"]);
        let modify = instance_grants("*S-1", "modify");
        assert_eq!(modify.len(), 2);
        assert!(modify[0].contains("(IO)(M)"));
    }

    #[test]
    fn quote_command_matches_list2cmdline() {
        assert_eq!(quote_command(&["java".to_owned(), "-Xmx1G".to_owned()]), "java -Xmx1G");
        assert_eq!(quote_command(&["C:\\Program Files\\j".to_owned()]), "\"C:\\Program Files\\j\"");
        assert_eq!(quote_command(&[String::new()]), "\"\"");
    }

    #[test]
    fn network_capabilities_follow_access_flag() {
        assert!(network_capabilities(true).contains(&CAPABILITY_INTERNET_CLIENT));
        assert!(network_capabilities(false).is_empty());
    }
}
