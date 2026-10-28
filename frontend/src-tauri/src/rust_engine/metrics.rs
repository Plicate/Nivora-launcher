// системные метрики для верхней панели
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SNAPSHOT_CACHE_SECONDS: f64 = 1.0;
const GPU_CACHE_SECONDS: f64 = 3.0;

static CPU_PREVIOUS: Mutex<Option<CpuSamples>> = Mutex::new(None);
static SNAPSHOT_CACHE: Mutex<Option<(Instant, String, Value)>> = Mutex::new(None);
static GPU_CACHE: Mutex<Option<(Instant, Vec<GpuInfo>)>> = Mutex::new(None);

pub fn system_metrics(request: &Value) -> Result<Value, String> {
    let fresh = request
        .get("fresh")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let game_root = request
        .get("game_root")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if let Some(cached) = snapshot_cache(fresh, &game_root) {
        return Ok(cached);
    }
    let gpus = gpus_cached();
    let data = json!({
        "timestamp": epoch_seconds(),
        "cpu": cpu_snapshot(),
        "memory": memory_snapshot(),
        "disks": disks_snapshot(&game_root),
        "gpus": gpus.iter().map(GpuInfo::to_json).collect::<Vec<_>>(),
        "temperatures": temperatures_json(&gpus),
        "source": { "psutil": false, "nvidia_smi": !gpus.is_empty() },
    });
    *SNAPSHOT_CACHE.lock().unwrap_or_else(|poison| poison.into_inner()) = Some((Instant::now(), game_root, data.clone()));
    Ok(data)
}

fn snapshot_cache(fresh: bool, game_root: &str) -> Option<Value> {
    let cache = SNAPSHOT_CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    match cache.as_ref() {
        Some((at, cached_root, value)) if snapshot_cache_usable(fresh, *at, cached_root, game_root) => Some(value.clone()),
        _ => None,
    }
}

fn snapshot_cache_usable(fresh: bool, cached_at: Instant, cached_root: &str, game_root: &str) -> bool {
    !fresh && cached_root == game_root && cached_at.elapsed().as_secs_f64() < SNAPSHOT_CACHE_SECONDS
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CpuSamples {
    total: u64,
    idle: u64,
}

fn parse_proc_stat(text: &str) -> Option<CpuSamples> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next()? != "cpu" {
            continue;
        }
        let values: Vec<u64> = parts.filter_map(|value| value.parse::<u64>().ok()).collect();
        if values.len() < 5 {
            continue;
        }
        let idle = values[3] + values[4];
        let total = values.iter().sum();
        return Some(CpuSamples { total, idle });
    }
    None
}

// общую загрузку считаем по разнице между выборками; без предыдущей возвращаем `None`
fn cpu_percent(previous: &CpuSamples, current: &CpuSamples) -> Option<f64> {
    let delta_total = current.total.saturating_sub(previous.total);
    let delta_idle = current.idle.saturating_sub(previous.idle);
    if delta_total == 0 {
        return None;
    }
    Some(round1(((1.0 - delta_idle as f64 / delta_total as f64) * 100.0).clamp(0.0, 100.0)))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn cpu_snapshot() -> Value {
    #[cfg(unix)]
    {
        let text = match fs::read_to_string("/proc/stat") {
            Ok(value) => value,
            Err(_) => return json!({ "percent": null }),
        };
        let Some(current) = parse_proc_stat(&text) else {
            return json!({ "percent": null });
        };
        let mut previous = CPU_PREVIOUS.lock().unwrap_or_else(|poison| poison.into_inner());
        let result = previous.as_ref().and_then(|prev| cpu_percent(prev, &current)).map(|percent| json!({ "percent": percent }));
        *previous = Some(current);
        result.unwrap_or(json!({ "percent": null }))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetSystemTimes;
        let zero_filetime = || FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut idle, mut kernel, mut user) = (zero_filetime(), zero_filetime(), zero_filetime());
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return json!({ "percent": null });
        }
        let ticks = |value: FILETIME| ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
        let current = CpuSamples { total: ticks(kernel) + ticks(user), idle: ticks(idle) };
        let mut previous = CPU_PREVIOUS.lock().unwrap_or_else(|poison| poison.into_inner());
        let result = previous.as_ref().and_then(|prev| cpu_percent(prev, &current)).map(|percent| json!({ "percent": percent }));
        *previous = Some(current);
        result.unwrap_or(json!({ "percent": null }))
    }
    #[cfg(not(any(unix, windows)))]
    {
        json!({ "percent": null })
    }
}

struct MemoryInfo {
    total: u64,
    available: u64,
}

fn parse_meminfo(text: &str) -> Option<MemoryInfo> {
    let mut total = 0u64;
    let mut available = 0u64;
    let mut seen_total = false;
    for line in text.lines() {
        let (key, rest) = line.split_once(':')?;
        // /proc/meminfo отдаёт килобайты
        let kilobytes: u64 = rest.trim().split_whitespace().next()?.parse().ok()?;
        let bytes = kilobytes * 1024;
        match key.trim() {
            "MemTotal" => {
                total = bytes;
                seen_total = true;
            }
            "MemAvailable" => available = bytes,
            "MemFree" => {
                if available == 0 {
                    available = bytes;
                }
            }
            _ => {}
        }
    }
    if !seen_total {
        return None;
    }
    Some(MemoryInfo { total, available: available.min(total) })
}

fn memory_snapshot() -> Value {
    #[cfg(unix)]
    {
        let text = match fs::read_to_string("/proc/meminfo") {
            Ok(value) => value,
            Err(_) => return null_memory(),
        };
        match parse_meminfo(&text) {
            Some(info) if info.total > 0 => {
                json!({ "percent": round1((info.total - info.available) as f64 / info.total as f64 * 100.0) })
            }
            _ => null_memory(),
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 || status.ullTotalPhys == 0 {
            return null_memory();
        }
        let used = status.ullTotalPhys.saturating_sub(status.ullAvailPhys);
        json!({ "percent": round1(used as f64 / status.ullTotalPhys as f64 * 100.0) })
    }
    #[cfg(not(any(unix, windows)))]
    {
        null_memory()
    }
}

fn null_memory() -> Value {
    json!({ "percent": null })
}

const DISK_FILESYSTEMS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "zfs", "apfs",
];

fn parse_mounts(text: &str) -> Vec<(String, String)> {
    // /proc/self/mounts: устройство точка_монтирования тип опции 0 0
    let mut result: Vec<(String, String)> = Vec::new();
    let mut seen_devices: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let device = match parts.next() {
            Some(value) if value.starts_with("/dev/") => value.to_owned(),
            _ => continue,
        };
        let Some(mount) = parts.next() else { continue };
        let Some(filesystem) = parts.next() else { continue };
        if !DISK_FILESYSTEMS.contains(&filesystem) {
            continue;
        }
        if seen_devices.iter().any(|item| item == &device) {
            continue;
        }
        seen_devices.push(device.clone());
        result.push((decode_mount_escape(mount), device));
    }
    result
}

fn decode_mount_escape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut result: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 4 <= bytes.len() {
            let octal = &value[index + 1..index + 4];
            if octal.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(code) = u8::from_str_radix(octal, 8) {
                    result.push(code);
                    index += 4;
                    continue;
                }
            }
        }
        result.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

#[cfg(unix)]
fn disk_usage(path: &Path) -> Option<(u64, u64)> {
    // statvfs: total = f_blocks * f_frsize, free = f_bavail * f_frsize
    // возвращаем место, доступное пользователю, а не свободное на диске
    let c_path = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    let status = unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) };
    if status != 0 || stats.f_frsize == 0 {
        return None;
    }
    let total = stats.f_blocks as u64 * stats.f_frsize as u64;
    let free = stats.f_bavail as u64 * stats.f_frsize as u64;
    if total == 0 {
        return None;
    }
    Some((total, free))
}

fn disks_snapshot(game_root: &str) -> Value {
    #[cfg(unix)]
    {
        let mut mounts: Vec<(String, String)> = Vec::new();
        if !game_root.is_empty() && Path::new(game_root).is_dir() {
            if let Some(mount) = mount_point_for(game_root) {
                mounts.push((mount.clone(), mount));
            }
        }
        if let Ok(text) = fs::read_to_string("/proc/self/mounts") {
            mounts.extend(parse_mounts(&text));
        }
        if mounts.is_empty() {
            if let Some((total, free)) = disk_usage(Path::new("/")) {
                return json!([{ "mount": "/", "total_bytes": total, "free_bytes": free, "percent": round1((total - free) as f64 / total as f64 * 100.0) }]);
            }
            return json!([]);
        }
        let mut result: Vec<Value> = Vec::new();
        for (mount, _) in mounts.iter().take(4) {
            if result.iter().any(|item| item["mount"].as_str() == Some(mount.as_str())) {
                continue;
            }
            let Some((total, free)) = disk_usage(Path::new(mount)) else { continue };
            result.push(json!({
                "mount": mount,
                "total_bytes": total,
                "free_bytes": free,
                "percent": round1((total - free) as f64 / total as f64 * 100.0),
            }));
            if result.len() >= 4 {
                break;
            }
        }
        Value::Array(result)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetLogicalDrives};

        let drive_mask = unsafe { GetLogicalDrives() };
        let preferred = windows_drive_letter(game_root);
        let mut drives: Vec<char> = (0..26)
            .filter(|index| drive_mask & (1u32 << *index as u32) != 0)
            .map(|index| (b'A' + index as u8) as char)
            .collect();
        if let Some(letter) = preferred {
            if let Some(index) = drives.iter().position(|drive| *drive == letter) {
                drives.remove(index);
                drives.insert(0, letter);
            }
        }

        let mut result = Vec::new();
        for letter in drives {
            let mount = format!("{letter}:\\");
            let path: Vec<u16> = mount.encode_utf16().chain(std::iter::once(0)).collect();
            let mut available = 0u64;
            let mut total = 0u64;
            let mut free = 0u64;
            let success = unsafe {
                GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free)
            };
            if success == 0 || total == 0 {
                continue;
            }
            let free = available.min(total);
            result.push(json!({
                "mount": mount,
                "total_bytes": total,
                "free_bytes": free,
                "percent": round1((total - free) as f64 / total as f64 * 100.0),
            }));
            if result.len() >= 4 {
                break;
            }
        }
        Value::Array(result)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = game_root;
        json!([])
    }
}

fn windows_drive_letter(path: &str) -> Option<char> {
    let bytes = path.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    Some((bytes[0] as char).to_ascii_uppercase())
}

#[cfg(unix)]
fn mount_point_for(path: &str) -> Option<String> {
    let mounts_text = fs::read_to_string("/proc/self/mounts").ok()?;
    let mut candidates: Vec<String> = parse_mounts(&mounts_text).into_iter().map(|(mount, _)| mount).collect();
    candidates.sort_by_key(|item| std::cmp::Reverse(item.len()));
    candidates.into_iter().find(|mount| path == mount || path.starts_with(&format!("{mount}/")))
}

#[derive(Clone, Debug)]
struct GpuInfo {
    index: usize,
    name: String,
    utilization_percent: Option<f64>,
    memory_used_mib: Option<f64>,
    memory_total_mib: Option<f64>,
    temperature_c: Option<f64>,
}

impl GpuInfo {
    fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "name": self.name,
            "utilization_percent": self.utilization_percent,
            "memory_used_mib": self.memory_used_mib,
            "memory_total_mib": self.memory_total_mib,
            "temperature_c": self.temperature_c,
        })
    }
}

fn parse_nvidia_csv(text: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() != 5 {
            continue;
        }
        gpus.push(GpuInfo {
            index,
            name: parts[0].chars().take(64).collect(),
            utilization_percent: parse_metric_number(parts[1]),
            memory_used_mib: parse_metric_number(parts[2]),
            memory_total_mib: parse_metric_number(parts[3]),
            temperature_c: parse_metric_number(parts[4]),
        });
    }
    gpus
}

/// недоступные значения `nvidia-smi` становятся `null`, а не нулём
fn parse_metric_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|number| *number >= 0.0)
}

fn gpus_cached() -> Vec<GpuInfo> {
    {
        let cache = GPU_CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some((at, gpus)) = cache.as_ref() {
            if at.elapsed().as_secs_f64() < GPU_CACHE_SECONDS {
                return gpus.clone();
            }
        }
    }
    let gpus = query_nvidia_smi();
    *GPU_CACHE.lock().unwrap_or_else(|poison| poison.into_inner()) = Some((Instant::now(), gpus.clone()));
    gpus
}

fn query_nvidia_smi() -> Vec<GpuInfo> {
    let output = super::platform::command("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            // таймаут 2 с: без nvidia-smi или с зависшим драйвером метрики
            // не должны подвешивать опрос UI
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        return if status.success() {
                            child.wait_with_output().map(|output| output.stdout)
                        } else {
                            Ok(Vec::new())
                        };
                    }
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(Vec::new());
                    }
                    Err(_) => return Ok(Vec::new()),
                }
            }
        });
    match output {
        Ok(bytes) if !bytes.is_empty() => parse_nvidia_csv(&String::from_utf8_lossy(&bytes)),
        _ => Vec::new(),
    }
}

fn temperatures_json(gpus: &[GpuInfo]) -> Value {
    let gpu_c = gpus.first().and_then(|gpu| gpu.temperature_c);
    json!({ "cpu_c": hwmon_cpu_temperature(), "gpu_c": gpu_c, "motherboard_c": null })
}

fn hwmon_cpu_temperature() -> Option<f64> {
    #[cfg(unix)]
    {
        let root = Path::new("/sys/class/hwmon");
        let mut directories: Vec<PathBuf> = match fs::read_dir(root) {
            Ok(entries) => entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect(),
            Err(_) => return None,
        };
        directories.sort();
        for directory in directories {
            let mut inputs: Vec<PathBuf> = match fs::read_dir(&directory) {
                Ok(entries) => entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                        name.starts_with("temp") && name.ends_with("_input")
                    })
                    .collect(),
                Err(_) => continue,
            };
            inputs.sort();
            for input in inputs {
                let label = input.with_file_name(
                    input.file_name().unwrap_or_default().to_string_lossy().replace("_input", "_label"),
                );
                let label_text = fs::read_to_string(&label).unwrap_or_default().to_lowercase();
                if !["cpu", "package", "core"].iter().any(|token| label_text.contains(token)) {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&input) {
                    if let Ok(milli) = raw.trim().parse::<f64>() {
                        let value = milli / 1000.0;
                        if (10.0..120.0).contains(&value) {
                            return Some(round1(value));
                        }
                    }
                }
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// метрики CPU, памяти и GPU процесса игры для бенчмарка шейдеров

const PROCESS_SAMPLE_MAX: usize = 512;
const PROCESS_GPU_CACHE_SECONDS: f64 = 2.0;

static PROCESS_GPU_CACHE: Mutex<Option<(Instant, i32, Value)>> = Mutex::new(None);
static DRM_GFX_PREVIOUS: Mutex<Option<std::collections::HashMap<i32, (Instant, u64)>>> = Mutex::new(None);

/// время CPU и RSS-байты дерева процессов от корневого PID
pub(crate) fn process_sample(pid: i32) -> Option<(f64, u64)> {
    #[cfg(unix)]
    {
        process_sample_from(pid, Path::new("/proc"))
    }
    #[cfg(windows)]
    {
        windows_process_sample(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        None
    }
}

#[cfg(unix)]
fn process_sample_from(pid: i32, proc_root: &Path) -> Option<(f64, u64)> {
    if pid <= 0 {
        return None;
    }
    let ticks = clock_ticks() as f64;
    let mut pending = vec![pid];
    let mut seen: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut root_found = false;
    let mut cpu_seconds_total = 0.0f64;
    let mut ram_bytes_total = 0u64;
    while let Some(current) = pending.pop() {
        if current <= 0 || !seen.insert(current) || seen.len() > PROCESS_SAMPLE_MAX {
            continue;
        }
        let proc = proc_root.join(current.to_string());
        let sample = single_process_sample(&proc, ticks);
        if current == pid {
            sample.as_ref()?;
            root_found = true;
        }
        if let Some((cpu_seconds, ram_bytes)) = sample {
            cpu_seconds_total += cpu_seconds;
            ram_bytes_total += ram_bytes;
        }
        if let Ok(children) = fs::read_to_string(proc.join("task").join(current.to_string()).join("children")) {
            pending.extend(
                children
                    .split_whitespace()
                    .filter_map(|value| value.parse::<i32>().ok()),
            );
        }
    }
    if !root_found {
        return None;
    }
    Some((cpu_seconds_total, ram_bytes_total))
}

#[cfg(unix)]
fn clock_ticks() -> i64 {
    // sysconf(SC_CLK_TCK): почти всегда 100, но берём честно
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1)
}

#[cfg(unix)]
fn single_process_sample(proc: &Path, ticks: f64) -> Option<(f64, u64)> {
    let stat_line = fs::read_to_string(proc.join("stat")).ok()?;
    let closing = stat_line.rfind(')')?;
    let fields: Vec<&str> = stat_line[closing + 2..].split_whitespace().collect();
    if fields.len() < 13 {
        return None;
    }
    let utime: f64 = fields[11].parse().ok()?;
    let stime: f64 = fields[12].parse().ok()?;
    let cpu_seconds = (utime + stime) / ticks.max(1.0);
    let status = fs::read_to_string(proc.join("status")).unwrap_or_default();
    let mut rss_kib: u64 = 0;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss_kib = rest.split_whitespace().next().and_then(|value| value.parse().ok()).unwrap_or(0);
            break;
        }
    }
    Some((cpu_seconds, rss_kib.saturating_mul(1024)))
}

/// на Windows учитывает только JVM; дочерние процессы оцениваются косвенно
#[cfg(windows)]
fn windows_process_sample(pid: i32) -> Option<(f64, u64)> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if handle.is_null() {
            return None;
        }
        let outcome = (|| {
            let mut creation = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let mut exit = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let mut kernel = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let mut user = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
                return None;
            }
            let to_seconds = |time: FILETIME| -> f64 {
                let ticks = ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
                ticks as f64 / 10_000_000.0
            };
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ram = if GetProcessMemoryInfo(handle, &mut counters, counters.cb) != 0 {
                counters.WorkingSetSize as u64
            } else {
                0
            };
            Some((to_seconds(kernel) + to_seconds(user), ram))
        })();
        CloseHandle(handle);
        outcome
    }
}

/// возвращает нагрузку GPU процесса игры или `null` во всех полях
pub(crate) fn gpu_process_sample(pid: i32) -> Value {
    if pid <= 0 {
        return json!({ "util_percent": null, "memory_bytes": null, "source": "none" });
    }
    let now = Instant::now();
    {
        let cache = PROCESS_GPU_CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some((at, cached_pid, sample)) = cache.as_ref() {
            if *cached_pid == pid && now.duration_since(*at).as_secs_f64() < PROCESS_GPU_CACHE_SECONDS {
                return sample.clone();
            }
        }
    }
    let mut sample = run_nvidia_pmon(pid);
    if sample.is_none() {
        if let Some(drm) = drm_fdinfo_sample(pid) {
            let mut previous = DRM_GFX_PREVIOUS.lock().unwrap_or_else(|poison| poison.into_inner());
            let history = previous.take().unwrap_or_default();
            let mut util: Option<f64> = None;
            if let Some((previous_at, previous_ns)) = history.get(&pid) {
                let elapsed = now.duration_since(*previous_at).as_secs_f64();
                if elapsed > 0.0 {
                    let delta_ns = drm.gfx_ns.saturating_sub(*previous_ns);
                    let percent = ((delta_ns as f64 / (elapsed * 1e9)) * 100.0).min(100.0);
                    util = Some(round1(percent));
                }
            }
            let mut updated = history;
            updated.insert(pid, (now, drm.gfx_ns));
            *previous = Some(updated);
            sample = Some(json!({
                "util_percent": util,
                "memory_bytes": if drm.vram_bytes > 0 { Some(drm.vram_bytes) } else { None },
                "source": "drm",
            }));
        }
    }
    let sample = sample.unwrap_or(json!({ "util_percent": null, "memory_bytes": null, "source": "none" }));
    *PROCESS_GPU_CACHE.lock().unwrap_or_else(|poison| poison.into_inner()) = Some((now, pid, sample.clone()));
    sample
}

fn run_nvidia_pmon(pid: i32) -> Option<Value> {
    #[cfg(target_os = "linux")]
    {
        let mut child = super::platform::command("nvidia-smi")
            .args(["pmon", "-c", "1", "-s", "um"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return None;
                    }
                    break;
                }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    return None;
                }
            }
        }
        let output = child.wait_with_output().ok()?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if let Some(sample) = parse_pmon_row(line, pid) {
                return Some(sample);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

fn parse_pmon_row(line: &str, pid: i32) -> Option<Value> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 6 || fields[1].parse::<i32>().ok() != Some(pid) {
        return None;
    }
    let util_percent = fields[3].parse::<f64>().ok();
    let memory_bytes = fields[4].parse::<f64>().ok().map(|value| (value * 1024.0 * 1024.0) as u64);
    Some(json!({
        "util_percent": util_percent,
        "memory_bytes": memory_bytes,
        "source": "nvidia",
    }))
}

struct DrmSample {
    gfx_ns: u64,
    vram_bytes: u64,
}

/// mesa-стек (amdgpu/i915/panfrost...) пишет наработку движков DRM в
/// fdinfo открытых видеокарт; это даёт процент нагрузки и VRAM без nvidia
fn drm_fdinfo_sample(pid: i32) -> Option<DrmSample> {
    #[cfg(unix)]
    {
        let fd_dir = PathBuf::from(format!("/proc/{pid}/fd"));
        let mut gfx_ns = 0u64;
        let mut vram = 0u64;
        let mut descriptors = fs::read_dir(&fd_dir).ok()?;
        let mut seen = 0usize;
        while let Some(entry) = descriptors.next() {
            seen += 1;
            if seen > 256 {
                break;
            }
            let descriptor = entry.ok()?.path();
            let Ok(raw_target) = fs::read_link(&descriptor) else {
                continue;
            };
            if !raw_target.to_string_lossy().contains("/dev/dri/") {
                continue;
            }
            let name = descriptor.file_name()?.to_string_lossy().to_string();
            let Ok(content) = fs::read_to_string(format!("/proc/{pid}/fdinfo/{name}")) else {
                continue;
            };
            let value = |text: &str| -> Option<u64> {
                text.split(':')
                    .nth(1)?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()
            };
            for info_line in content.lines() {
                if info_line.starts_with("drm-engine-gfx:") {
                    gfx_ns += value(info_line).unwrap_or(0);
                } else if info_line.starts_with("drm-memory-vram:") || info_line.starts_with("drm-memory-gtt:") {
                    vram += value(info_line).unwrap_or(0);
                }
            }
        }
        if gfx_ns == 0 && vram == 0 {
            return None;
        }
        Some(DrmSample { gfx_ns, vram_bytes: vram })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

// сессионные метрики экземпляра

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

const METRICS_FILE: &str = ".trial-launcher-metrics.json";
const METRICS_MAX_BYTES: u64 = 512 * 1024;
const MAX_INSTANCE_LIVE_STATE_JSON_BYTES: u64 = 64 * 1024;
const MAX_MEASURED_FILES: usize = 20_000;
const MAX_SESSION_LOG_LINES: i64 = 500;
/// сколько замеров храним для средних значений (~10 минут при опросе раз в секунду)
const TRACKER_MAX_HISTORY: usize = 600;
const STORAGE_RESAMPLE_SECONDS: f64 = 5.0;

/// маркеры жизненного цикла игры в логе: событие фиксируется один раз
const SESSION_EVENT_MARKERS: &[(&str, &str)] = &[
    (r"Backend library: LWJGL", "LWJGL инициализирован"),
    (r"Backend library: OpenGL", "OpenGL-контекст создан"),
    (r"Setting user:", "Профиль игрока применён"),
    (r"Reloading ResourceManager", "Загрузка ресурсов"),
    (r"OpenAL initialized", "Звук (OpenAL) готов"),
    (r"Sound engine started", "Звуковой движок запущен"),
    (r"Created: \d+x\d+x\d+ [\w:/.-]+atlas", "Атлас текстур создан"),
    (r"Loading \d+ mods?", "Загрузка модов"),
    (r"Loaded \d+ mods?", "Моды загружены"),
    (r"Forge Mod Loader", "Forge загружается"),
    (r"Preparing start region", "Мир: подготовка стартового региона"),
    (r"Starting integrated server", "Запуск встроенного сервера (мир открыт)"),
    (r"Joining worker", "Подключение к серверу"),
    (r"Stopping!", "Игра останавливается"),
];

fn event_marker_patterns() -> &'static Vec<(regex::Regex, &'static str)> {
    static PATTERNS: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        SESSION_EVENT_MARKERS
            .iter()
            .map(|(pattern, title)| (regex::Regex::new(pattern).expect("session marker"), *title))
            .collect()
    })
}

fn error_line_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"/ERROR\]|\(FATAL\)|SEVERE").expect("error line"))
}

fn warn_line_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"/WARN\]").expect("warn line"))
}

fn mod_id_patterns() -> &'static [regex::Regex; 2] {
    static PATTERNS: OnceLock<[regex::Regex; 2]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            regex::Regex::new(r"\] \[([a-z0-9_-]{2,32})/\]").expect("forge mod id"),
            regex::Regex::new(r"\] \(([a-z0-9_-]{2,32})\)").expect("fabric mod id"),
        ]
    })
}

/// извлекает из лога игры события жизненного цикла и активность модов
#[derive(Default)]
pub(crate) struct SessionEvents {
    events: Vec<Value>,
    mod_stats: BTreeMap<String, (String, i64, i64, i64, f64)>, // mod, lines, warnings, errors, first_seen
    seen_markers: BTreeSet<&'static str>,
    last_line: Option<String>,
}

impl SessionEvents {
    pub(crate) fn feed(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let fresh: Vec<String> = match &self.last_line {
            // хвост лога ограничен (последние N строк): считаем новыми все
            // строки после последнего знакомого совпадения
            Some(last) => match lines.iter().rposition(|line| line == last) {
                Some(position) => lines[position + 1..].to_vec(),
                None => lines.to_vec(),
            },
            None => lines.to_vec(),
        };
        self.last_line = Some(lines.last().cloned().unwrap_or_default());
        for line in fresh {
            self.consume(&line);
        }
    }

    pub(crate) fn snapshot(&self) -> Value {
        let mut mods: Vec<&(String, i64, i64, i64, f64)> = self.mod_stats.values().collect();
        mods.sort_by(|left, right| {
            let score = |item: &(String, i64, i64, i64, f64)| item.3 * 8 + item.2 * 3 + item.1 / 4;
            score(right).cmp(&score(left))
        });
        let mods = mods
            .into_iter()
            .map(|(mod_id, lines, warnings, errors, first_seen)| {
                json!({"mod": mod_id, "lines": lines, "warnings": warnings, "errors": errors, "first_seen": first_seen})
            })
            .collect::<Vec<_>>();
        json!({"events": self.events, "mods": mods})
    }

    fn consume(&mut self, line: &str) {
        let now = epoch_seconds();
        for (pattern, title) in event_marker_patterns() {
            if self.seen_markers.contains(title) {
                continue;
            }
            if pattern.is_match(line) {
                self.seen_markers.insert(title);
                self.events.push(json!({"at": now, "title": title}));
                break;
            }
        }
        if let Some(mod_id) = mod_id_from_line(line) {
            let entry = self
                .mod_stats
                .entry(mod_id.clone())
                .or_insert_with(|| (mod_id.clone(), 0, 0, 0, now));
            entry.1 += 1;
            if error_line_pattern().is_match(line) {
                entry.3 += 1;
            } else if warn_line_pattern().is_match(line) {
                entry.2 += 1;
            }
        }
    }
}

/// fabric: "[main/INFO] (modid) сообщение", Forge/NeoForge: "[main/INFO] [modid/] …"
fn mod_id_from_line(line: &str) -> Option<String> {
    for pattern in mod_id_patterns() {
        if let Some(captures) = pattern.captures(line) {
            if let Some(value) = captures.get(1).map(|m| m.as_str().to_owned()) {
                if !["main", "worker", "render", "server", "system", "minecraft"].contains(&value.as_str()) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// размер каталога экземпляра: без logs/cache/снапшотов, ≤20000 файлов
fn measured_directory_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else { continue };
        for entry in entries.filter_map(|entry| entry.ok()) {
            if count >= MAX_MEASURED_FILES {
                return total;
            }
            let item = entry.path();
            let name = item.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
            if ["logs", "cache", ".trial-launcher-snapshots"].contains(&name.as_str()) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&item) else { continue };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(item);
            } else if meta.is_file() {
                total += meta.len();
                count += 1;
            }
        }
    }
    total
}

pub(crate) fn read_metrics(root: &Path) -> Value {
    let path = root.join(METRICS_FILE);
    let Ok(meta) = fs::symlink_metadata(&path) else { return json!({}) };
    if meta.is_symlink() || !meta.is_file() || meta.len() > METRICS_MAX_BYTES {
        return json!({});
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(|value| value.is_object())
        .unwrap_or(json!({}))
}

fn write_last_snapshot(root: &Path, snapshot: &Value) -> Result<(), String> {
    if root.is_symlink() || !root.is_dir() {
        return Err("Каталог метрик экземпляра недоступен".to_owned());
    }
    let path = root.join(METRICS_FILE);
    let temporary = path.with_extension(format!("tmp-{}", super::snapshots::random_hex(4)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|error| format!("Каталог метрик экземпляра недоступен: {error}"))?;
    use std::io::Write as _;
    let body = serde_json::to_string(&json!({"last": snapshot})).unwrap_or_default();
    file.write_all(format!("{body}\n").as_bytes())
        .and_then(|_| file.flush())
        .map_err(|error| format!("Каталог метрик экземпляра недоступен: {error}"))?;
    drop(file);
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Каталог метрик экземпляра недоступен: {error}")
    })
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some((values.iter().sum::<f64>() / values.len() as f64 * 100.0).round() / 100.0)
}

pub(crate) struct ProcessMetricsTracker {
    root: PathBuf,
    pid: i32,
    started_at: f64,
    started_monotonic: Instant,
    previous_cpu: Option<f64>,
    previous_at: Option<Instant>,
    peak_ram: u64,
    peak_cpu: f64,
    peak_gpu: Option<f64>,
    storage_bytes: u64,
    storage_sampled_at: f64,
    cpu_history: Vec<f64>,
    ram_history: Vec<f64>,
    gpu_history: Vec<f64>,
    pub(crate) session_events: SessionEvents,
}

impl ProcessMetricsTracker {
    pub(crate) fn new(root: &Path, pid: i32, started_at: Option<f64>) -> Self {
        let started_at = started_at.filter(|value| *value > 0.0).unwrap_or_else(epoch_seconds);
        let started_monotonic = Instant::now() - Duration::from_secs_f64((epoch_seconds() - started_at).max(0.0));
        Self {
            root: root.to_path_buf(),
            pid,
            started_at,
            started_monotonic,
            previous_cpu: None,
            previous_at: None,
            peak_ram: 0,
            peak_cpu: 0.0,
            peak_gpu: Some(0.0),
            storage_bytes: 0,
            storage_sampled_at: 0.0,
            cpu_history: Vec::new(),
            ram_history: Vec::new(),
            gpu_history: Vec::new(),
            session_events: SessionEvents::default(),
        }
    }

    pub(crate) fn sample(&mut self, running: bool) -> Value {
        let now = epoch_seconds();
        let monotonic_now = Instant::now();
        let process = process_sample(self.pid);
        let mut cpu_percent = 0.0f64;
        let mut ram_bytes = 0u64;
        if let Some((cpu_seconds, sampled_ram)) = process {
            ram_bytes = sampled_ram;
            if let (Some(previous_cpu), Some(previous_at)) = (self.previous_cpu, self.previous_at) {
                let elapsed = (monotonic_now - previous_at).as_secs_f64().max(0.05);
                cpu_percent = ((cpu_seconds - previous_cpu) / elapsed * 100.0).clamp(0.0, 999.0);
            }
            self.previous_cpu = Some(cpu_seconds);
            self.previous_at = Some(monotonic_now);
        }
        let gpu = gpu_process_sample(if process.is_some() { self.pid } else { 0 });
        self.peak_ram = self.peak_ram.max(ram_bytes);
        self.peak_cpu = self.peak_cpu.max(cpu_percent);
        if let Some(util) = gpu.get("util_percent").and_then(Value::as_f64) {
            self.peak_gpu = Some(self.peak_gpu.unwrap_or(0.0).max(util));
        }
        if self.storage_sampled_at == 0.0 || now - self.storage_sampled_at >= STORAGE_RESAMPLE_SECONDS {
            self.storage_bytes = measured_directory_size(&self.root);
            self.storage_sampled_at = now;
        }
        self.cpu_history.push(cpu_percent);
        self.ram_history.push(ram_bytes as f64);
        self.gpu_history.push(gpu.get("util_percent").and_then(Value::as_f64).unwrap_or(0.0));
        let cutoff = self.cpu_history.len().saturating_sub(TRACKER_MAX_HISTORY);
        self.cpu_history.drain(..cutoff);
        let cutoff = self.ram_history.len().saturating_sub(TRACKER_MAX_HISTORY);
        self.ram_history.drain(..cutoff);
        let cutoff = self.gpu_history.len().saturating_sub(TRACKER_MAX_HISTORY);
        self.gpu_history.drain(..cutoff);
        let peak_gpu = self.peak_gpu.map(|value| (value * 100.0).round() / 100.0);
        json!({
            "timestamp": now,
            "pid": self.pid,
            "running": running && process.is_some(),
            "uptime_seconds": (monotonic_now - self.started_monotonic).as_secs(),
            "peak_ram_bytes": self.peak_ram,
            "peak_cpu_percent": (self.peak_cpu * 100.0).round() / 100.0,
            "peak_gpu_percent": peak_gpu,
            "avg_cpu_percent": average(&self.cpu_history),
            "avg_ram_bytes": average(&self.ram_history).map(|value| value as i64),
            "avg_gpu_percent": average(&self.gpu_history),
            "ram_bytes": ram_bytes,
            "cpu_percent": (cpu_percent * 100.0).round() / 100.0,
            "gpu": gpu,
            "storage_bytes": self.storage_bytes,
            "players": 0,
            "max_players": 0,
            "tps": 0.0,
        })
    }

    pub(crate) fn finish(&mut self) -> Result<Value, String> {
        let mut snapshot = self.sample(false);
        snapshot["ended_at"] = json!(epoch_seconds());
        snapshot["exit_code"] = Value::Null;
        snapshot["peak_ram_bytes"] = json!(self.peak_ram);
        snapshot["peak_cpu_percent"] = json!((self.peak_cpu * 100.0).round() / 100.0);
        snapshot["session"] = self.session_events.snapshot();
        write_last_snapshot(&self.root, &snapshot)?;
        Ok(snapshot)
    }
}

/// метрики покоя: игра не запущена
fn idle_metrics(root: &Path) -> Value {
    json!({
        "timestamp": epoch_seconds(),
        "pid": 0,
        "running": false,
        "uptime_seconds": 0,
        "ram_bytes": 0,
        "cpu_percent": 0.0,
        "storage_bytes": measured_directory_size(root),
        "players": 0,
        "max_players": 0,
        "tps": 0.0,
    })
}

pub(crate) fn metrics_for_instance(root: &Path, pid: i32, started_at: f64) -> Value {
    let mut pid = pid;
    let mut started_at = started_at;
    if pid <= 0 {
        // pid берётся из live-state экземпляра, если он жив
        let live_state = root.join(".trial-launcher-live.json");
        let readable = fs::symlink_metadata(&live_state)
            .map(|meta| !meta.is_symlink() && meta.is_file() && meta.len() <= MAX_INSTANCE_LIVE_STATE_JSON_BYTES)
            .unwrap_or(false);
        if readable {
            if let Ok(raw) = fs::read_to_string(&live_state).map_err(|_| ()).and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(|_| ())) {
                if raw.get("running").and_then(Value::as_bool).unwrap_or(false) {
                    pid = raw.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32;
                }
                started_at = raw.get("started_at").and_then(Value::as_f64).unwrap_or(0.0);
            }
        }
    }
    if pid > 0 {
        let mut tracker = ProcessMetricsTracker::new(root, pid, Some(started_at));
        return tracker.sample(process_sample(pid).is_some());
    }
    idle_metrics(root)
}

fn instance_metric_trackers() -> &'static Mutex<HashMap<String, ProcessMetricsTracker>> {
    static TRACKERS: OnceLock<Mutex<HashMap<String, ProcessMetricsTracker>>> = OnceLock::new();
    TRACKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// отслеживает процесс по имени экземпляра и сохраняет итоговый снимок после завершения игры
pub(crate) fn instance_metrics(name: &str, pid: i32, started_at: f64, running: bool, root: &Path, lines: Option<&[String]>) -> Value {
    if pid <= 0 {
        instance_metric_trackers().lock().unwrap().remove(name);
        let metrics = read_metrics(root);
        if let Some(last) = metrics.get("last") {
            if last.is_object() && !last.as_object().map(|object| object.is_empty()).unwrap_or(true) {
                let mut snapshot = last.clone();
                snapshot["pid"] = json!(0);
                snapshot["running"] = json!(false);
                return snapshot;
            }
        }
        return metrics_for_instance(root, 0, 0.0);
    }
    let mut trackers = instance_metric_trackers().lock().unwrap();
    let stale = match trackers.get(name) {
        Some(tracker) => tracker.pid != pid || (tracker.started_at - started_at).abs() > 0.5,
        None => true,
    };
    if stale {
        trackers.insert(name.to_owned(), ProcessMetricsTracker::new(root, pid, Some(started_at)));
    }
    let Some(tracker) = trackers.get_mut(name) else {
        return metrics_for_instance(root, pid, started_at);
    };
    if let Some(fresh) = lines {
        tracker.session_events.feed(fresh);
    }
    let mut snapshot = tracker.sample(running);
    let still_running = snapshot["running"].as_bool().unwrap_or(false);
    snapshot["session"] = tracker.session_events.snapshot();
    if !still_running && !running {
        // после завершения игры сохраняем финальный снимок последнего запуска
        if let Ok(finished) = tracker.finish() {
            snapshot = finished;
        }
        trackers.remove(name);
    }
    snapshot
}

// ── операция instance_log: живая лента лога с метриками ────────────

pub(crate) fn instance_log(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    let config = super::config::ConfigStore::load()?;
    let store = super::instances::InstanceStore::new(&config)?;
    let info = store.get(name)?;
    let state_path = info.path.join(".trial-launcher-live.json");
    let (mut lines, mut running) = super::launcher::session_snapshot(name);
    let mut started_at = 0.0f64;
    let mut pid = 0i32;
    let mut log_id = String::new();
    let readable = fs::symlink_metadata(&state_path)
        .map(|meta| !meta.is_symlink() && meta.is_file() && meta.len() <= MAX_INSTANCE_LIVE_STATE_JSON_BYTES)
        .unwrap_or(false);
    if readable {
        if let Ok(raw) = fs::read_to_string(&state_path).map_err(|_| ()).and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(|_| ())) {
            running = raw.get("running").and_then(Value::as_bool).unwrap_or(false);
            pid = if running { raw.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32 } else { 0 };
            started_at = raw.get("started_at").and_then(Value::as_f64).unwrap_or(0.0);
            log_id = raw.get("log_id").and_then(Value::as_str).unwrap_or_default().to_owned();
            if pid > 0 {
                running = super::platform_support::process_exists(pid);
            }
        }
    }
    if !log_id.is_empty() {
        if let Ok(tail) = super::instance_files::InstanceFiles::new(&config, None)?.read_log_tail(name, &log_id, MAX_SESSION_LOG_LINES) {
            lines = tail;
        }
    }
    let metrics_root = if info.game_edition == "bedrock" {
        super::bedrock::profile_path_for(&info)?
    } else {
        info.path.clone()
    };
    let metrics = instance_metrics(name, pid, started_at, running, &metrics_root, Some(&lines));
    let last_metrics = read_metrics(&metrics_root).get("last").cloned().unwrap_or(json!({}));
    Ok(json!({
        "lines": lines,
        "running": running,
        "metrics": metrics,
        "last_metrics": last_metrics,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_delta_yields_expected_percent() {
        let previous = parse_proc_stat("cpu  100 0 0 100 0
cpu0 50 0 0 50 0").unwrap();
        // агрегат: total 200→300 (Δ=100), idle 100→100 (Δ=0) → 100%
        let current = parse_proc_stat("cpu  200 0 0 100 0
cpu0 150 0 0 75 0").unwrap();
        assert_eq!(cpu_percent(&previous, &current), Some(100.0));
        // при слишком частом опросе нулевая дельта даёт `null`, не 0%
        assert!(cpu_percent(&current, &current).is_none());
        // если данных по ядрам нет, используем только общий счётчик
        let solo_prev = parse_proc_stat("cpu  10 0 0 10 0").unwrap();
        let solo = parse_proc_stat("cpu  20 0 0 20 0").unwrap();
        assert_eq!(cpu_percent(&solo_prev, &solo), Some(50.0));
    }

    #[test]
    fn meminfo_semantics() {
        let info = parse_meminfo(
            "MemTotal:       1000 kB\nMemFree:         100 kB\nMemAvailable:    250 kB\nSwapTotal:       500 kB\nSwapFree:        250 kB\n",
        )
        .unwrap();
        assert_eq!(info.total, 1000 * 1024);
        assert_eq!(info.available, 250 * 1024);
        // если `MemAvailable` нет, используем `MemFree`
        let fallback = parse_meminfo("MemTotal: 100 kB\nMemFree: 40 kB\n").unwrap();
        assert_eq!(fallback.available, 40 * 1024);
        assert!(parse_meminfo("SwapTotal: 1 kB\n").is_none());
    }

    #[test]
    fn windows_drive_letter_uses_the_game_directory_when_available() {
        assert_eq!(windows_drive_letter(r"C:\Games"), Some('C'));
        assert_eq!(windows_drive_letter("d:/Games"), Some('D'));
        assert_eq!(windows_drive_letter(r"\\server\share"), None);
        assert_eq!(windows_drive_letter(""), None);
    }

    #[test]
    fn mounts_keep_real_block_devices_only() {
        let text = "/dev/nvme0n1p2 / ext4 rw,relatime 0 0\ntmpfs /run tmpfs rw 0 0\nproc /proc proc rw 0 0\n/dev/sda1 /data btrfs rw 0 0\n/dev/nvme0n1p2 /bindmount ext4 rw 0 0\n/dev/sdb1 /win ntfs3 rw 0 0\n";
        let mounts = parse_mounts(text);
        let names: Vec<&str> = mounts.iter().map(|(mount, _)| mount.as_str()).collect();
        // исключаем `tmpfs` и `proc`, а также повторы одного устройства
        assert_eq!(names, vec!["/", "/data", "/win"]);
        // восьмеричные экранирования в точке монтирования раскодируются
        assert_eq!(decode_mount_escape("/media/my\\040disk"), "/media/my disk");
    }

    #[test]
    fn session_events_feed_markers_and_mod_stats() {
        let mut events = SessionEvents::default();
        let lines: Vec<String> = vec![
            "[main/INFO]: Backend library: LWJGL".to_owned(),
            "[main/INFO]: Backend library: LWJGL".to_owned(),
            "[main/ERROR] [sodium/] Failed to bake".to_owned(),
            "[main/WARN] [sodium/] Slow render".to_owned(),
            "[main/INFO] (fabricmod) ready".to_owned(),
        ];
        events.feed(&lines);
        // повторная подача тех же строк ничего не добавляет
        events.feed(&lines);
        let snapshot = events.snapshot();
        let events_list = snapshot["events"].as_array().unwrap();
        assert_eq!(events_list.len(), 1);
        assert_eq!(events_list[0]["title"], json!("LWJGL инициализирован"));
        let mods = snapshot["mods"].as_array().unwrap();
        assert_eq!(mods.len(), 2);
        // sodium тяжелее: ошибка весит больше предупреждения
        assert_eq!(mods[0]["mod"], json!("sodium"));
        assert_eq!(mods[0]["errors"], json!(1));
        assert_eq!(mods[0]["warnings"], json!(1));
        assert_eq!(mods[1]["mod"], json!("fabricmod"));
        assert_eq!(mods[1]["lines"], json!(1));
    }

    #[test]
    fn session_events_tail_dedup_keeps_only_fresh() {
        let mut events = SessionEvents::default();
        events.feed(&["[main/INFO]: Backend library: LWJGL".to_owned(), "a".to_owned()]);
        events.feed(&[
            "[main/INFO]: Backend library: LWJGL".to_owned(),
            "a".to_owned(),
            "[main/INFO]: OpenAL initialized".to_owned(),
        ]);
        let snapshot = events.snapshot();
        let titles: Vec<&str> = snapshot["events"].as_array().unwrap().iter().map(|item| item["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["LWJGL инициализирован", "Звук (OpenAL) готов"]);
    }

    #[test]
    fn mod_id_extraction_variants() {
        assert_eq!(mod_id_from_line("[main/INFO] [sodium/] bake"), Some("sodium".to_owned()));        assert_eq!(mod_id_from_line("[main/INFO] (fabricapi) loaded"), Some("fabricapi".to_owned()));
        assert_eq!(mod_id_from_line("[main/INFO]: plain line"), None);
        assert_eq!(mod_id_from_line("[main/INFO] [main/] core"), None);
    }

    #[test]
    fn tracker_samples_and_finishes_for_dead_pid() {
        let root = std::env::temp_dir().join(format!("nivora-metrics-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs/latest.log"), b"xxx").unwrap();
        fs::write(root.join("world.dat"), b"yyy").unwrap();
        let mut tracker = ProcessMetricsTracker::new(&root, 999_999, Some(0.0));
        let snapshot = tracker.sample(true);
        assert_eq!(snapshot["running"], json!(false));
        assert_eq!(snapshot["pid"], json!(999_999));
        assert_eq!(snapshot["players"], json!(0));
        // logs не считается в storage
        assert_eq!(snapshot["storage_bytes"], json!(3));
        let finished = tracker.finish().unwrap();
        assert_eq!(finished["exit_code"], Value::Null);
        let persisted = read_metrics(&root);
        assert_eq!(persisted["last"]["pid"], json!(999_999));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn instance_metrics_uses_last_snapshot_for_stopped_game() {
        let root = std::env::temp_dir().join(format!("nivora-metrics-idle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_last_snapshot(&root, &json!({"pid": 42, "running": true, "peak_ram_bytes": 123})).unwrap();
        let snapshot = instance_metrics("demo", 0, 0.0, false, &root, None);
        assert_eq!(snapshot["pid"], json!(0));
        assert_eq!(snapshot["running"], json!(false));
        assert_eq!(snapshot["peak_ram_bytes"], json!(123));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn measured_directory_size_skips_excluded_names() {
        let root = std::env::temp_dir().join(format!("nivora-metrics-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::write(root.join("logs/a.log"), [0u8; 10]).unwrap();
        fs::write(root.join("mods/b.jar"), [0u8; 4]).unwrap();
        fs::write(root.join("options.txt"), [0u8; 3]).unwrap();
        assert_eq!(measured_directory_size(&root), 7);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nvidia_csv_maps_na_to_null() {        let gpus = parse_nvidia_csv("NVIDIA GeForce RTX 4070, 35, 4000, 12288, 45\nSome GPU, [N/A], [N/A], 8192, [N/A]\nbroken line\n");
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(gpus[0].utilization_percent, Some(35.0));
        assert_eq!(gpus[0].temperature_c, Some(45.0));
        assert_eq!(gpus[1].utilization_percent, None);
        assert_eq!(gpus[1].memory_total_mib, Some(8192.0));
    }

    #[test]
    fn process_sample_aggregates_tree_from_fake_proc() {
        // /proc-дерево на фейковом корне: родитель + ребёнок, VmRSS в KiB
        #[cfg(unix)]
        {
            let root = std::env::temp_dir().join(format!("nivora-procsample-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let parent = root.join("4242");
            let child = root.join("4243");
            fs::create_dir_all(parent.join("task/4242")).unwrap();
            fs::create_dir_all(&child).unwrap();
            fs::write(parent.join("stat"), "4242 (java -Xmx) S 1 4242 4242 0 -1 0 0 0 0 0 100 50").unwrap();
            fs::write(parent.join("status"), "VmRSS:\t1024 kB\n").unwrap();
            fs::write(parent.join("task/4242/children"), "4243").unwrap();
            fs::write(child.join("stat"), "4243 (java chi) S 4242 4242 4242 0 -1 0 0 0 0 0 0 0 0").unwrap();
            fs::write(child.join("status"), "VmRSS:\t512 kB\n").unwrap();
            let sample = process_sample_from(4242, &root).expect("замер дерева");
            // 150 тиков / 100 Гц = 1.5 CPU-секунды; RSS 1024+512 KiB в байтах
            assert_eq!(sample.1, 1536 * 1024);
            assert!((sample.0 - 1.5).abs() < 1e-6);
            assert!(process_sample_from(777, &root).is_none());
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn pmon_row_parses_matching_pid_only() {
        let sample = parse_pmon_row("0  4242  C  37  512  3300  java", 4242).expect("строка pmon");
        assert_eq!(sample["util_percent"], serde_json::json!(37.0));
        assert_eq!(sample["memory_bytes"], serde_json::json!(512 * 1024 * 1024));
        assert_eq!(sample["source"], "nvidia");
        assert!(parse_pmon_row("0  9999  C  37  512  3300  java", 4242).is_none());
        assert!(parse_pmon_row("# gpu type pid sm mem enc dec command", 4242).is_none());
    }

    #[test]
    fn gpu_sample_of_dead_pid_reports_none_source() {
        assert_eq!(gpu_process_sample(0)["source"], "none");
        let _ = gpu_process_sample(999_999_999);
    }

    #[test]
    fn snapshot_cache_does_not_cross_game_roots() {
        let cached_at = Instant::now();
        assert!(super::snapshot_cache_usable(false, cached_at, "/game/a", "/game/a"));
        assert!(!super::snapshot_cache_usable(false, cached_at, "/game/a", "/game/b"));
        assert!(!super::snapshot_cache_usable(true, cached_at, "/game/a", "/game/a"));
    }

    #[test]
    fn snapshot_shape_matches_frontend_contract() {
        // system_metrics возвращает данные без конверта; конверт
        // {"ok", "data"} надевает rust_engine::handle
        let data = super::system_metrics(&serde_json::json!({ "fresh": true })).expect("снимок метрик");
        for key in ["timestamp", "cpu", "memory", "disks", "gpus", "temperatures", "source"] {
            assert!(data.get(key).is_some(), "нет ключа {key}");
        }
        assert_eq!(data["source"]["psutil"], serde_json::json!(false));
        assert!(data["cpu"]["percent"].is_null() || data["cpu"]["percent"].is_f64());
        assert!(data["memory"]["percent"].is_null() || data["memory"]["percent"].is_f64());
        let cached = super::system_metrics(&serde_json::json!({ "game_root": "" })).expect("снимок для игрового пути");
        assert_eq!(cached["cpu"].is_object(), true);
    }
}
