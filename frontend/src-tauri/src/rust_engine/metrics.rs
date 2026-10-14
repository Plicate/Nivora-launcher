// system_metrics — снимок системных метрик для топбара (опрос каждые
// пару секунд, всё читается локально):
//  - недоступные значения возвращаются null/[], а не нулями — интерфейс
//    их прячет;
//  - CPU: дельта /proc/stat между вызовами, первый вызов — percent=null;
//  - память: /proc/meminfo (MemAvailable с откатом на MemFree, swap при
//    наличии);
//  - диски: реальные блочные разделы из /proc/self/mounts + каталог игры,
//    не больше 4, дубликаты монтирований схлопываются;
//  - GPU: nvidia-smi с кешем на 3 с и таймаутом 2 с;
//  - температуры: hwmon по меткам cpu/package/core, диапазон 10..120 °C.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SNAPSHOT_CACHE_SECONDS: f64 = 1.0;
const GPU_CACHE_SECONDS: f64 = 3.0;

static CPU_PREVIOUS: Mutex<Option<CpuSamples>> = Mutex::new(None);
static SNAPSHOT_CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
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
    if let Some(cached) = snapshot_cache(fresh) {
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
    *SNAPSHOT_CACHE.lock().unwrap_or_else(|poison| poison.into_inner()) = Some((Instant::now(), data.clone()));
    Ok(data)
}

fn snapshot_cache(fresh: bool) -> Option<Value> {
    if fresh {
        return None;
    }
    let cache = SNAPSHOT_CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    match cache.as_ref() {
        Some((at, value)) if at.elapsed().as_secs_f64() < SNAPSHOT_CACHE_SECONDS => Some(value.clone()),
        _ => None,
    }
}

fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------- CPU ----

#[derive(Clone, Debug, PartialEq)]
struct CpuSamples {
    /// (метка "cpu"/"cpu0".., total, idle) в порядке следования в /proc/stat.
    entries: Vec<(String, u64, u64)>,
}

fn parse_proc_stat(text: &str) -> Option<CpuSamples> {
    let mut entries = Vec::new();
    for line in text.lines() {
        // Строки ядер: "cpu0 ..." (после "cpu" идёт цифра), "cpu" — агрегат.
        let mut parts = line.split_whitespace();
        let Some(label) = parts.next() else { continue };
        if !label.starts_with("cpu") {
            continue;
        }
        if label != "cpu" && !label[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let values: Vec<u64> = parts.filter_map(|value| value.parse::<u64>().ok()).collect();
        if values.len() < 5 {
            continue;
        }
        let idle = values[3] + values[4];
        let total = values.iter().sum();
        entries.push((label.to_owned(), total, idle));
    }
    if entries.is_empty() {
        None
    } else {
        Some(CpuSamples { entries })
    }
}

/// Дельта между выборками → (общий %, % по ядрам). Нет предыдущей выборки
/// или она неполна — None: интерфейс честно покажет «—», а не выдумает 0%.
fn cpu_percent(previous: &CpuSamples, current: &CpuSamples) -> Option<(f64, Vec<f64>)> {
    if current.entries.len() != previous.entries.len() || current.entries.is_empty() {
        return None;
    }
    let mut percents = Vec::with_capacity(current.entries.len());
    for (index, (label, total, idle)) in current.entries.iter().enumerate() {
        let (prev_label, prev_total, prev_idle) = previous.entries.get(index)?;
        if label != prev_label {
            return None;
        }
        let delta_total = total.saturating_sub(*prev_total);
        let delta_idle = idle.saturating_sub(*prev_idle);
        if delta_total == 0 {
            return None;
        }
        let percent = ((1.0 - delta_idle as f64 / delta_total as f64) * 100.0).clamp(0.0, 100.0);
        percents.push(round1(percent));
    }
    Some((percents[0], percents[1..].to_vec()))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn cpu_snapshot() -> Value {
    #[cfg(unix)]
    {
        let text = match fs::read_to_string("/proc/stat") {
            Ok(value) => value,
            Err(_) => return json!({ "percent": null, "per_core": [], "cores": 0 }),
        };
        let Some(current) = parse_proc_stat(&text) else {
            return json!({ "percent": null, "per_core": [], "cores": 0 });
        };
        let cores = current.entries.len().saturating_sub(1);
        let mut previous = CPU_PREVIOUS.lock().unwrap_or_else(|poison| poison.into_inner());
        let result = previous
            .as_ref()
            .and_then(|prev| cpu_percent(prev, &current))
            .map(|(total, per_core)| {
                json!({ "percent": total, "per_core": per_core.into_iter().take(32).collect::<Vec<_>>(), "cores": cores })
            });
        *previous = Some(current);
        result.unwrap_or(json!({ "percent": null, "per_core": [], "cores": cores }))
    }
    #[cfg(not(unix))]
    {
        json!({ "percent": null, "per_core": [], "cores": 0 })
    }
}

// ------------------------------------------------------------- Память ----

struct MemoryInfo {
    total: u64,
    available: u64,
    swap_total: u64,
    swap_free: u64,
}

fn parse_meminfo(text: &str) -> Option<MemoryInfo> {
    let mut total = 0u64;
    let mut available = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    let mut seen_total = false;
    for line in text.lines() {
        let (key, rest) = line.split_once(':')?;
        // /proc/meminfo отдаёт килобайты.
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
            "SwapTotal" => swap_total = bytes,
            "SwapFree" => swap_free = bytes,
            _ => {}
        }
    }
    if !seen_total {
        return None;
    }
    Some(MemoryInfo { total, available: available.min(total), swap_total, swap_free })
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
                let swap_percent = if info.swap_total > 0 {
                    json!(round1((info.swap_total - info.swap_free) as f64 / info.swap_total as f64 * 100.0))
                } else {
                    Value::Null
                };
                json!({
                    "used_bytes": info.total - info.available,
                    "total_bytes": info.total,
                    "percent": round1((info.total - info.available) as f64 / info.total as f64 * 100.0),
                    "swap_percent": swap_percent,
                })
            }
            _ => null_memory(),
        }
    }
    #[cfg(not(unix))]
    {
        null_memory()
    }
}

fn null_memory() -> Value {
    json!({ "used_bytes": null, "total_bytes": null, "percent": null, "swap_percent": null })
}

// --------------------------------------------------------------- Диски ----

const DISK_FILESYSTEMS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "zfs", "apfs",
];

fn parse_mounts(text: &str) -> Vec<(String, String)> {
    // /proc/self/mounts: устройство точка_монтирования тип опции 0 0.
    // Берём только реальные блочные устройства знакомых ФС — псевдо-FS
    // (tmpfs, proc, sysfs, overlay у контейнеров) в метриках не нужны.
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

/// В /proc/self/mounts пробел и табуляция кодируются восьмеричными
/// последовательностями (\040, \011) — раскодируем, иначе путь не найдётся.
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
    // (доступно пользователю — как считал psutil.disk_usage).
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
    #[cfg(not(unix))]
    {
        let _ = game_root;
        json!([])
    }
}

/// Точка монтирования, на которой лежит путь: самый длинный префикс из
/// известных монтирований (каталог игры может быть на вложенном разделе).
#[cfg(unix)]
fn mount_point_for(path: &str) -> Option<String> {
    let mounts_text = fs::read_to_string("/proc/self/mounts").ok()?;
    let mut candidates: Vec<String> = parse_mounts(&mounts_text).into_iter().map(|(mount, _)| mount).collect();
    candidates.sort_by_key(|item| std::cmp::Reverse(item.len()));
    candidates.into_iter().find(|mount| path == mount || path.starts_with(&format!("{mount}/")))
}

// ---------------------------------------------------------------- GPU ----

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

/// nvidia-smi отдаёт "[N/A]" для недоступных значений — это null, не 0.
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
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            // Таймаут 2 с: без nvidia-smi или с зависшим драйвером метрики
            // не должны подвешивать опрос UI.
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

// -------------------------------------------------------- Температуры ----

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

// --------------------------------------------------------------- Тесты ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_delta_yields_expected_percent() {
        let previous = parse_proc_stat("cpu  100 0 0 100 0\ncpu0 50 0 0 50 0\ncpu1 50 0 0 50 0").unwrap();
        // Агрегат: total 200→300 (Δ=100), idle 100→100 (Δ=0) → 100%.
        // Ядро 0: total 100→225 (Δ=125), idle 50→75 (Δ=25) → (1-25/125)=80%.
        // Ядро 1: total 100→300 (Δ=200), idle 50→100 (Δ=50) → (1-50/200)=75%.
        let current = parse_proc_stat("cpu  200 0 0 100 0\ncpu0 150 0 0 75 0\ncpu1 200 0 0 100 0").unwrap();
        let (total, per_core) = cpu_percent(&previous, &current).expect("дельта считается");
        assert_eq!(total, 100.0);
        assert_eq!(per_core, vec![80.0, 75.0]);
        // Разное число ядер между выборками (hotplug CPU) — честный null.
        let short = parse_proc_stat("cpu  1 0 0 1 0").unwrap();
        assert!(cpu_percent(&previous, &short).is_none());
        // Нулевая дельта (слишком частый опрос) — тоже null, не 0%.
        assert!(cpu_percent(&current, &current).is_none());
    }

    #[test]
    fn meminfo_semantics() {
        let info = parse_meminfo(
            "MemTotal:       1000 kB\nMemFree:         100 kB\nMemAvailable:    250 kB\nSwapTotal:       500 kB\nSwapFree:        250 kB\n",
        )
        .unwrap();
        assert_eq!(info.total, 1000 * 1024);
        assert_eq!(info.available, 250 * 1024);
        assert_eq!(info.swap_total, 500 * 1024);
        // Нет MemAvailable — откат на MemFree.
        let fallback = parse_meminfo("MemTotal: 100 kB\nMemFree: 40 kB\n").unwrap();
        assert_eq!(fallback.available, 40 * 1024);
        assert!(parse_meminfo("SwapTotal: 1 kB\n").is_none());
    }

    #[test]
    fn mounts_keep_real_block_devices_only() {
        let text = "/dev/nvme0n1p2 / ext4 rw,relatime 0 0\ntmpfs /run tmpfs rw 0 0\nproc /proc proc rw 0 0\n/dev/sda1 /data btrfs rw 0 0\n/dev/nvme0n1p2 /bindmount ext4 rw 0 0\n/dev/sdb1 /win ntfs3 rw 0 0\n";
        let mounts = parse_mounts(text);
        let names: Vec<&str> = mounts.iter().map(|(mount, _)| mount.as_str()).collect();
        // tmpfs/proc отброшены; повторное монтирование того же устройства — тоже.
        assert_eq!(names, vec!["/", "/data", "/win"]);
        // Восьмеричные экранирования в точке монтирования раскодируются.
        assert_eq!(decode_mount_escape("/media/my\\040disk"), "/media/my disk");
    }

    #[test]
    fn nvidia_csv_maps_na_to_null() {
        let gpus = parse_nvidia_csv("NVIDIA GeForce RTX 4070, 35, 4000, 12288, 45\nSome GPU, [N/A], [N/A], 8192, [N/A]\nbroken line\n");
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(gpus[0].utilization_percent, Some(35.0));
        assert_eq!(gpus[0].temperature_c, Some(45.0));
        assert_eq!(gpus[1].utilization_percent, None);
        assert_eq!(gpus[1].memory_total_mib, Some(8192.0));
    }

    #[test]
    fn snapshot_shape_matches_frontend_contract() {
        // system_metrics возвращает данные без конверта; конверт
        // {"ok", "data"} надевает rust_engine::handle.
        let data = super::system_metrics(&serde_json::json!({ "fresh": true })).expect("снимок метрик");
        for key in ["timestamp", "cpu", "memory", "disks", "gpus", "temperatures", "source"] {
            assert!(data.get(key).is_some(), "нет ключа {key}");
        }
        assert_eq!(data["source"]["psutil"], serde_json::json!(false));
        assert!(data["cpu"]["cores"].is_u64());
        assert!(data["memory"]["used_bytes"].is_null() || data["memory"]["used_bytes"].is_u64());
        // Горячий кеш: второй вызов без fresh возвращает тот же снимок.
        let cached = super::system_metrics(&serde_json::json!({})).expect("кеш");
        assert_eq!(cached, data);
    }
}
