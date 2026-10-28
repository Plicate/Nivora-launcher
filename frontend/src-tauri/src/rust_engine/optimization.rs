// рекомендации по параметрам Java и графики
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::config::ConfigStore;
use super::instances::InstanceStore;

const AIKARS_FLAGS: &[&str] = &[
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=30",
    "-XX:G1MaxNewSizePercent=40",
    "-XX:G1HeapRegionSize=8M",
    "-XX:G1ReservePercent=20",
    "-XX:G1HeapWastePercent=5",
    "-XX:G1MixedGCCountTarget=4",
    "-XX:InitiatingHeapOccupancyPercent=15",
    "-XX:G1MixedGCLiveThresholdPercent=90",
    "-XX:G1RSetUpdatingPauseTimePercent=5",
    "-XX:SurvivorRatio=32",
    "-XX:+PerfDisableSharedMem",
    "-XX:MaxTenuringThreshold=1",
    "-Dusing.aikars.flags=https://mcflags.emc.gs",
    "-Daikars.new.flags=true",
];

const FPS_FLAGS: &[&str] = &[
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=50",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+DisableExplicitGC",
    "-Dfml.earlyprogresswindow=false",
];

const BALANCED_FLAGS: &[&str] = &[
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=100",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+DisableExplicitGC",
];

const COMPAT_FLAGS: &[&str] = &["-XX:+UseSerialGC", "-Dfml.earlyprogresswindow=false"];

const STREAMING_FLAGS: &[&str] = &[
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=100",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+DisableExplicitGC",
    "-XX:+PerfDisableSharedMem",
];

struct Preset {
    id: &'static str,
    name: &'static str,
    flags: &'static [&'static str],
}

const PRESETS: &[Preset] = &[
    Preset { id: "fps", name: "FPS (низкие паузы GC)", flags: FPS_FLAGS },
    Preset { id: "balanced", name: "Balanced", flags: BALANCED_FLAGS },
    Preset { id: "heavy_modpack", name: "Heavy Modpack (Aikar's)", flags: AIKARS_FLAGS },
    Preset { id: "compatibility", name: "Compatibility", flags: COMPAT_FLAGS },
    Preset { id: "streaming", name: "Streaming", flags: STREAMING_FLAGS },
    Preset { id: "none", name: "Без пресета", flags: &[] },
];

fn preset_by_id(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

struct OptimizerMod {
    name: &'static str,
    mod_id: &'static str,
    description: &'static str,
    url: &'static str,
    loaders: &'static [&'static str],
    min_mc_version: &'static str,
    category: &'static str,
    required_side: &'static str,
}

const OPTIMIZER_MODS: &[OptimizerMod] = &[
    OptimizerMod { name: "Sodium", mod_id: "sodium", description: "Современный рендерер чанков: значительный рост FPS на клиенте.", url: "https://modrinth.com/mod/sodium", loaders: &["fabric", "quilt"], min_mc_version: "1.16", category: "fps", required_side: "client" },
    OptimizerMod { name: "Lithium", mod_id: "lithium", description: "Оптимизация игровой логики (физика, мобы, ИИ) без изменения поведения.", url: "https://modrinth.com/mod/lithium", loaders: &["fabric", "quilt"], min_mc_version: "1.16", category: "fps", required_side: "both" },
    OptimizerMod { name: "FerriteCore", mod_id: "ferrite-core", description: "Снижает потребление памяти данными блоков и предметов.", url: "https://modrinth.com/mod/ferrite-core", loaders: &["fabric", "quilt", "forge", "neoforge"], min_mc_version: "1.16", category: "memory", required_side: "both" },
    OptimizerMod { name: "Phosphor", mod_id: "phosphor", description: "Оптимизация системы освещения чанков.", url: "https://modrinth.com/mod/phosphor", loaders: &["fabric"], min_mc_version: "1.15", category: "fps", required_side: "both" },
    OptimizerMod { name: "Starlight", mod_id: "starlight", description: "Переписанный движок освещения — быстрее ванильного.", url: "https://modrinth.com/mod/starlight", loaders: &["fabric", "quilt"], min_mc_version: "1.17", category: "fps", required_side: "both" },
    OptimizerMod { name: "LazyDFU", mod_id: "lazydfu", description: "Откладывает компиляцию датапаков DFU — быстрее старт игры.", url: "https://modrinth.com/mod/lazydfu", loaders: &["fabric", "quilt"], min_mc_version: "1.16", category: "loading", required_side: "client" },
    OptimizerMod { name: "ModernFix", mod_id: "modernfix", description: "Комплексное ускорение загрузки и снижение памяти для больших сборок.", url: "https://modrinth.com/mod/modernfix", loaders: &["fabric", "quilt", "forge", "neoforge"], min_mc_version: "1.16", category: "loading", required_side: "both" },
    OptimizerMod { name: "Indium", mod_id: "indium", description: "Совместимость Sodium с Fabric Rendering API (нужно некоторым модам).", url: "https://modrinth.com/mod/indium", loaders: &["fabric", "quilt"], min_mc_version: "1.16", category: "fps", required_side: "client" },
    OptimizerMod { name: "ImmediatelyFast", mod_id: "immediatelyfast", description: "Ускоряет рендеринг сущностей и элементов HUD.", url: "https://modrinth.com/mod/immediatelyfast", loaders: &["fabric", "quilt"], min_mc_version: "1.18", category: "fps", required_side: "client" },
    OptimizerMod { name: "Entity Culling", mod_id: "entityculling", description: "Пропускает рендеринг невидимых сущностей.", url: "https://modrinth.com/mod/entityculling", loaders: &["fabric", "quilt", "forge", "neoforge"], min_mc_version: "1.16", category: "fps", required_side: "client" },
    OptimizerMod { name: "Clumps", mod_id: "clumps", description: "Склеивает дроп опыта в один объект — меньше нагрузки на сервер.", url: "https://modrinth.com/mod/clumps", loaders: &["fabric", "quilt", "forge", "neoforge"], min_mc_version: "1.16", category: "fps", required_side: "both" },
    OptimizerMod { name: "Krypton", mod_id: "krypton", description: "Оптимизация сетевого стека: пакеты и каналы.", url: "https://modrinth.com/mod/krypton", loaders: &["fabric", "quilt"], min_mc_version: "1.16", category: "network", required_side: "both" },
];

// --------------------------------------------------------- система и память ----

pub fn system_ram_kib() -> u64 {
    #[cfg(unix)]
    {
        // `sysconf` не везде возвращает число страниц, поэтому используем `/proc`
        if let Ok(text) = fs::read_to_string("/proc/meminfo") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    let value = rest.trim().split_whitespace().next().and_then(|token| token.parse::<u64>().ok());
                    if let Some(kib) = value {
                        return kib;
                    }
                }
            }
        }
        unsafe {
            let pages = libc::sysconf(libc::_SC_PHYS_PAGES);
            let page = libc::sysconf(libc::_SC_PAGESIZE);
            if pages > 0 && page > 0 {
                return (pages as u64 * page as u64) / 1024;
            }
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            return status.ullTotalPhys / 1024;
        }
    }
    0
}

fn count_mods(instance_dir: Option<&Path>) -> usize {
    let Some(dir) = instance_dir else { return 0 };
    let mods = dir.join("mods");
    let Ok(entries) = fs::read_dir(&mods) else { return 0 };
    let mut count = 0;
    for entry in entries.filter_map(|entry| entry.ok()) {
        if count >= 4096 {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if (name.ends_with(".jar") || name.ends_with(".litemod")) && path.is_file() {
            count += 1;
        }
    }
    count
}

fn classify_weight(mod_count: usize, loader: &str) -> &'static str {
    if loader.is_empty() || loader == "vanilla" {
        return "vanilla";
    }
    if mod_count == 0 {
        return "vanilla";
    }
    if mod_count < 20 {
        return "light";
    }
    if mod_count < 80 {
        return "medium";
    }
    "heavy"
}

const MEMORY_TABLE: &[(&str, u64, u64)] = &[
    ("vanilla", 0, 1536),
    ("vanilla", 8, 2048),
    ("light", 0, 2048),
    ("light", 8, 2048),
    ("light", 16, 3072),
    ("medium", 0, 3072),
    ("medium", 8, 4096),
    ("medium", 16, 6144),
    ("heavy", 0, 4096),
    ("heavy", 8, 6144),
    ("heavy", 16, 8192),
    ("heavy", 32, 10240),
    ("legacy", 0, 1024),
    ("legacy", 4, 1536),
];

fn recommend_memory_mib(weight: &str, ram_kib: u64, mod_count: usize) -> u64 {
    let sys_gib = ram_kib / (1024 * 1024);
    let mut best: u64 = 2048;
    for (candidate, min_gib, mib) in MEMORY_TABLE {
        if *candidate == weight && sys_gib >= *min_gib {
            best = *mib;
        }
    }
    if mod_count > 80 {
        let extra = ((mod_count - 80) / 20) as u64 * 256;
        best = (best + extra).min(16384);
    }
    if ram_kib > 0 {
        let ceiling = ram_kib * 6 / (10 * 1024);
        best = best.min(ceiling.max(1024));
    }
    best
}

pub fn recommended_memory_kib() -> u64 {
    let total = system_ram_kib();
    if total == 0 {
        return 2 * 1024 * 1024;
    }
    (total / 2 / 1024 * 1024).max(1024 * 1024)
}

// поддерживает auto и значения с единицами G, MiB и KiB
fn parse_memory_kib(value: &str) -> Option<u64> {
    let raw = value.trim().to_lowercase();
    if raw.is_empty() || raw == "auto" || raw == "automatic" {
        return Some(recommended_memory_kib());
    }
    let mut digits = String::new();
    let mut unit = String::new();
    for character in raw.chars() {
        if (character.is_ascii_digit() || character == '.') && unit.is_empty() {
            digits.push(character);
        } else if !character.is_whitespace() {
            unit.push(character);
        }
    }
    let number: f64 = digits.parse().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    // по умолчанию значения указаны в MiB; суффикс b означает байты
    let bytes = unit == "b";
    let unit = unit.trim_end_matches('b').trim_end_matches('i');
    let factor = match unit {
        "" if bytes => 1.0 / 1024.0,
        "" | "m" => 1024.0,
        "k" => 1.0,
        "g" => 1024.0 * 1024.0,
        "t" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let kib = (number * factor) as u64;
    if kib == 0 {
        return None;
    }
    Some(kib)
}

fn mc_minor_version(version: &str) -> u32 {
    let Some(rest) = version.trim().strip_prefix("1.") else { return 0 };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

fn recommend_optimizer_mods(loader: &str, mc_version: &str, installed: &[String]) -> Vec<Value> {
    let minor = mc_minor_version(mc_version);
    let mut results: Vec<Value> = Vec::new();
    for class_mod in OPTIMIZER_MODS {
        if minor < mc_minor_version(class_mod.min_mc_version) {
            continue;
        }
        if !loader.is_empty() && !class_mod.loaders.is_empty() && !class_mod.loaders.contains(&loader.to_lowercase().as_str()) {
            continue;
        }
        results.push(json!({
            "name": class_mod.name,
            "mod_id": class_mod.mod_id,
            "description": class_mod.description,
            "url": class_mod.url,
            "category": class_mod.category,
            "required_side": class_mod.required_side,
            "already_installed": installed.iter().any(|id| id == class_mod.mod_id),
        }));
    }
    let priority = |category: &str| match category {
        "fps" => 0,
        "memory" => 1,
        "loading" => 2,
        "network" => 3,
        _ => 9,
    };
    results.sort_by_key(|item| {
        let installed_flag = item["already_installed"].as_bool().unwrap_or(false);
        let category = item["category"].as_str().unwrap_or("");
        (installed_flag, priority(category))
    });
    results
}

// ------------------------------------------------------------------- gPU ----

#[derive(Default, Clone)]
struct GpuInfo {
    vendor: String,
    driver: String,
    renderer: String,
    vram_mib: u64,
    supports_vulkan: bool,
    is_hybrid: bool,
}

fn command_exists(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

fn detect_gpu() -> GpuInfo {
    #[cfg(unix)]
    {
        detect_gpu_linux()
    }
    #[cfg(not(unix))]
    {
        detect_gpu_windows()
    }
}

fn detect_gpu_linux() -> GpuInfo {
    let mut info = GpuInfo::default();
    let mut vendors: Vec<String> = Vec::new();
    if let Ok(cards) = fs::read_dir("/sys/class/drm") {
        for card in cards.filter_map(|entry| entry.ok()) {
            let Ok(text) = fs::read_to_string(card.path().join("device").join("vendor")) else { continue };
            let vendor = match text.trim().to_lowercase().as_str() {
                "0x10de" => "nvidia",
                "0x1002" => "amd",
                "0x8086" => "intel",
                _ => continue,
            };
            if !vendors.iter().any(|item| item == vendor) {
                vendors.push(vendor.to_owned());
            }
        }
    }
    for preferred in ["nvidia", "amd", "intel"] {
        if vendors.iter().any(|item| item == preferred) {
            info.vendor = preferred.to_owned();
            break;
        }
    }
    info.is_hybrid = vendors.len() >= 2;
    if info.vendor == "nvidia" && command_exists("nvidia-smi") {
        if let Ok(output) = super::platform::command("nvidia-smi")
            .args(["--query-gpu=memory.total,driver_version", "--format=csv,noheader,nounits"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = text.trim().split(',').collect();
                if let Some(first) = parts.first() {
                    info.vram_mib = first.trim().parse().unwrap_or(0);
                }
                if let Some(second) = parts.get(1) {
                    info.driver = second.trim().to_owned();
                }
            }
        }
    }
    if command_exists("glxinfo") {
        if let Ok(output) = super::platform::command("glxinfo")
            .arg("-B")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if line.to_lowercase().contains("opengl renderer") {
                        info.renderer = line.split(':').next_back().unwrap_or_default().trim().to_owned();
                        break;
                    }
                }
            }
        }
    }
    info.supports_vulkan = command_exists("vulkaninfo");
    info
}

#[cfg(not(unix))]
fn detect_gpu_windows() -> GpuInfo {
    let mut info = GpuInfo::default();
    let mut adapters: Vec<(String, String, u64)> = Vec::new();
    if let Ok(output) = super::platform::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command",
            "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion | ConvertTo-Json -Compress"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        if output.status.success() {
            if let Ok(value) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout)) {
                let list = match value {
                    Value::Array(list) => list,
                    single => vec![single],
                };
                for item in list {
                    if !item.is_object() {
                        continue;
                    }
                    let name = item.get("Name").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let driver = item.get("DriverVersion").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let vram = item.get("AdapterRAM").and_then(Value::as_i64).unwrap_or(0) as u64 / (1024 * 1024);
                    adapters.push((name, driver, vram));
                }
            }
        }
    }
    let classify = |name: &str| -> &'static str {
        let lowered = name.to_lowercase();
        if lowered.contains("nvidia") {
            "nvidia"
        } else if lowered.contains("amd") || lowered.contains("radeon") {
            "amd"
        } else if lowered.contains("intel") {
            "intel"
        } else {
            "unknown"
        }
    };
    for preferred in ["nvidia", "amd", "intel"] {
        if let Some((name, driver, vram)) = adapters.iter().find(|(name, _, _)| classify(name) == preferred) {
            info.vendor = preferred.to_owned();
            info.renderer = name.clone();
            info.driver = driver.clone();
            info.vram_mib = *vram;
            break;
        }
    }
    let has_intel = adapters.iter().any(|(name, _, _)| classify(name) == "intel");
    let has_discrete = adapters.iter().any(|(name, _, _)| matches!(classify(name), "nvidia" | "amd"));
    info.is_hybrid = has_intel && has_discrete;
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    info.supports_vulkan = Path::new(&system_root).join("System32").join("vulkan-1.dll").is_file();
    info
}

// ------------------------------------------------------------- java-скан ----

fn modscan_script() -> Option<PathBuf> {
    super::platform::helper_path("ModScan.java", "NIVORA_MODSCAN").or_else(|| {
        let exe = std::env::current_exe().ok()?;
        let parent = exe.parent()?.parent()?;
        [
            parent.join("java").join("ModScan.java"),
            parent.join("nivora-launcher").join("java").join("ModScan.java"),
        ]
        .into_iter()
        .find(|path| path.is_file())
    })
}

fn java_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NIVORA_JAVA_BIN") {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    for name in ["java", "java.exe"] {
        if let Ok(path) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    // ищем Java в рантаймах, которыми управляет лаунчер
    let home = std::env::var("HOME").ok()?;
    let runtimes = PathBuf::from(home).join(".local/share/nivora-launcher/runtimes");
    let Ok(entries) = fs::read_dir(&runtimes) else { return None };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let candidate = entry.path().join("bin").join("java");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

struct ModScanOutcome {
    java_required: Option<u64>,
    installed_ids: Vec<String>,
}

// один запуск Java сканирует все JAR и собирает версии и идентификаторы модов
fn run_modscan(jars: &[PathBuf]) -> Option<ModScanOutcome> {
    if jars.is_empty() {
        return Some(ModScanOutcome { java_required: None, installed_ids: Vec::new() });
    }
    let java = java_binary()?;
    let script = modscan_script()?;
    let output = super::platform::command(&java)
        .arg(&script)
        .args(jars)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut max_java: Option<u64> = None;
    let mut installed_ids: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(java_major) = value.get("java_major").and_then(Value::as_u64) {
            if java_major > 0 {
                max_java = Some(max_java.map_or(java_major, |current| current.max(java_major)));
            }
        }
        if let Some(mod_id) = value.get("mod_id").and_then(Value::as_str) {
            if !mod_id.is_empty() && !installed_ids.iter().any(|item| item == mod_id) {
                installed_ids.push(mod_id.to_owned());
            }
        }
    }
    Some(ModScanOutcome { java_required: max_java, installed_ids })
}

// идентификаторы модов из боковых файлов каталога mods
fn installed_sidecar_mod_ids(mods_dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let Ok(entries) = fs::read_dir(mods_dir) else { return ids };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_symlink() || !name.starts_with('.') || !name.ends_with(".trial-project.json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else { continue };
        if let Some(mod_id) = value.get("mod_id").and_then(Value::as_str) {
            if !mod_id.trim().is_empty() && !ids.iter().any(|item| item == mod_id) {
                ids.push(mod_id.trim().to_owned());
            }
        }
    }
    ids
}

// ------------------------------------------------------------ операция ----


const LAUNCH_HISTORY_FILE: &str = ".nivora-launcher-launch-history.json";
const MAX_LAUNCH_HISTORY_JSON_BYTES: u64 = 256 * 1024;
const MAX_HISTORY_ENTRIES: usize = 20;

pub(crate) fn record_launch_start(
    instance_dir: &Path,
    launch_id: &str,
    loader: &str,
    preset: &str,
    memory: &str,
    mc_version: &str,
) {
    let path = instance_dir.join(LAUNCH_HISTORY_FILE);
    let mut history = load_launch_history(&path);
    history.push(json!({
        "launch_id": launch_id,
        "started_at": epoch_seconds(),
        "loader": loader,
        "preset": preset,
        "memory": memory,
        "mc_version": mc_version,
        "exit_code": null,
        "elapsed_seconds": null,
        "time_to_window_seconds": null,
    }));
    save_launch_history(&path, &mut history);
}

pub(crate) fn record_launch_end(
    instance_dir: &Path,
    launch_id: &str,
    exit_code: i32,
    elapsed_seconds: f64,
    time_to_window_seconds: Option<f64>,
) {
    let path = instance_dir.join(LAUNCH_HISTORY_FILE);
    let mut history = load_launch_history(&path);
    for entry in history.iter_mut().rev() {
        if entry.get("launch_id").and_then(Value::as_str) == Some(launch_id) {
            entry["exit_code"] = json!(exit_code);
            entry["elapsed_seconds"] = json!((elapsed_seconds * 100.0).round() / 100.0);
            if let Some(seconds) = time_to_window_seconds {
                entry["time_to_window_seconds"] = json!((seconds * 100.0).round() / 100.0);
            }
            break;
        }
    }
    save_launch_history(&path, &mut history);
}

fn load_launch_history(path: &Path) -> Vec<Value> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Vec::new();
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_LAUNCH_HISTORY_JSON_BYTES {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(entries)) => entries.into_iter().filter(|entry| entry.is_object()).collect(),
        _ => Vec::new(),
    }
}

/// читает историю запусков; автонастройка использует её для поиска повторных сбоев
pub(crate) fn launch_history_public(instance_dir: &Path) -> Vec<Value> {
    load_launch_history(&instance_dir.join(LAUNCH_HISTORY_FILE))
}

fn save_launch_history(path: &Path, history: &mut Vec<Value>) {
    if history.len() > MAX_HISTORY_ENTRIES {
        let drained = history.split_off(history.len() - MAX_HISTORY_ENTRIES);
        *history = drained;
    }
    let _ = super::net::atomic_json_write(path, &Value::Array(history.clone()), false);
}

fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn optimization_analyze(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let instance_name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let mut instance_dir: Option<PathBuf> = None;
    let mut loader = String::new();
    let mut mc_version = String::new();
    let mut current_memory = config
        .settings()
        .get("memory")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_owned();
    let mut current_preset = "none".to_owned();
    if !instance_name.is_empty() {
        let info = store.get(&instance_name)?;
        instance_dir = Some(info.path.clone());
        let settings = instance_settings(&info.path);
        // сначала учитываем явные настройки, затем метаданные экземпляра
        loader = settings
            .get("loader")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| info.loader.trim().to_lowercase());
        mc_version = settings
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| info.minecraft_version.trim().to_owned());
        if let Some(memory) = settings.get("memory").and_then(Value::as_str) {
            if !memory.trim().is_empty() {
                current_memory = memory.to_owned();
            }
        }
        current_preset = settings
            .get("jvm_preset")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .trim()
            .to_owned();
    }

    let ram = system_ram_kib();
    let mod_count = count_mods(instance_dir.as_deref());
    let mut weight = classify_weight(mod_count, &loader).to_owned();
    if !mc_version.is_empty() && mc_minor_version(&mc_version) < 13 {
        weight = "legacy".to_owned();
    }
    let rec_mib = recommend_memory_mib(&weight, ram, mod_count);
    let preset_id = match weight.as_str() {
        "legacy" => "compatibility",
        "heavy" => "heavy_modpack",
        "medium" => "balanced",
        _ => "fps",
    };

    let mut jar_list: Vec<PathBuf> = Vec::new();
    if let Some(dir) = instance_dir.as_deref() {
        let mods = dir.join("mods");
        if let Ok(entries) = fs::read_dir(&mods) {
            for entry in entries.filter_map(|entry| entry.ok()).take(512) {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if (name.ends_with(".jar") || name.ends_with(".jar.disabled")) && path.is_file() {
                    jar_list.push(path);
                }
            }
        }
    }
    let modscan = run_modscan(&jar_list).unwrap_or(ModScanOutcome { java_required: None, installed_ids: Vec::new() });
    let mut installed_ids = modscan.installed_ids;
    if let Some(dir) = instance_dir.as_deref() {
        for id in installed_sidecar_mod_ids(&dir.join("mods")) {
            if !installed_ids.iter().any(|item| item == &id) {
                installed_ids.push(id);
            }
        }
    }

    let mods = recommend_optimizer_mods(&loader, if mc_version.is_empty() { "1.21" } else { &mc_version }, &installed_ids);
    let gpu = detect_gpu();
    let cur_mib = parse_memory_kib(&current_memory).unwrap_or_else(recommended_memory_kib) / 1024;

    let mut warnings: Vec<String> = Vec::new();
    if cur_mib < 1024 {
        warnings.push("Выделено менее 1 ГиБ — игра может крашиться.".to_owned());
    } else if cur_mib + 512 < rec_mib {
        warnings.push(format!("Выделено {cur_mib} МиБ, рекомендуется {rec_mib} МиБ для данной сборки."));
    }
    if ram > 0 && cur_mib > (ram / 1024) * 75 / 100 {
        warnings.push("Выделено >75 % RAM — может замедлить систему.".to_owned());
    }
    if gpu.is_hybrid && gpu.vendor == "nvidia" {
        if cfg!(windows) {
            warnings.push("Обнаружен гибридный GPU (NVIDIA + Intel). Убедитесь, что в настройках NVIDIA выбрана 'Максимальная производительность' для java.exe.".to_owned());
        } else {
            warnings.push("Обнаружен гибридный GPU (NVIDIA + iGPU). Рекомендуется режим графики: nvidia.".to_owned());
        }
    }

    let preset = preset_by_id(preset_id);
    let report = json!({
        "system": {
            "ram_kib": ram,
            "ram_gib": (ram as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0,
            "cpu_count": std::thread::available_parallelism().map(|value| value.get()).unwrap_or(1),
            "os": std::env::consts::OS,
            "gpu_vendor": gpu.vendor,
            "gpu_renderer": gpu.renderer,
            "gpu_driver": gpu.driver,
            "gpu_vram_mib": gpu.vram_mib,
            "gpu_is_hybrid": gpu.is_hybrid,
        },
        "instance": {
            "loader": loader,
            "mc_version": mc_version,
            "mod_count": mod_count,
            "weight": weight,
            "java_required": modscan.java_required,
        },
        "recommendations": {
            "memory_mib": rec_mib,
            "preset_id": preset_id,
            "preset_name": preset.map(|preset| preset.name).unwrap_or_default(),
            "preset_flags": preset.map(|preset| preset.flags).unwrap_or_default(),
            "optimizer_mods": mods,
            "warnings": warnings,
        },
        "current": {
            "memory_mib": cur_mib,
            "preset_id": current_preset,
        },
    });
    Ok(json!({ "report": report }))
}

// читаем только поля, нужные для рекомендаций, не изменяя файл
fn instance_settings(instance_dir: &Path) -> Map<String, Value> {
    let path = instance_dir.join(".trial-launcher.json");
    if path.is_symlink() || !path.is_file() {
        return Map::new();
    }
    let Ok(raw) = fs::read_to_string(&path) else { return Map::new() };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// применяет пресет и заданные параметры памяти и графики
pub fn optimization_apply(request: &Value) -> Result<Value, String> {
    let instance_name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let preset_id = request.get("preset_id").and_then(Value::as_str).unwrap_or("none").trim().to_owned();
    let memory_mib = request.get("memory_mib").and_then(Value::as_i64);
    let graphics_mode = request.get("graphics_mode").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if preset_by_id(&preset_id).is_none() {
        return Err(format!("Неизвестный пресет: {preset_id}"));
    }
    let memory_kib = |mib: i64| -> String {
        super::config::format_memory_kib(mib.clamp(512, 32_768) * 1024)
    };
    if !instance_name.is_empty() {
        let mut updates = Map::new();
        updates.insert("jvm_preset".into(), json!(preset_id));
        if let Some(mib) = memory_mib {
            updates.insert("memory".into(), json!(memory_kib(mib)));
        }
        if matches!(graphics_mode.as_str(), "auto" | "nvidia" | "mesa" | "software") {
            updates.insert("graphics_mode".into(), json!(graphics_mode));
        }
        // общий путь сохранения нормализует настройки и защищает служебные поля
        let workspace = super::instance_workspace::InstanceWorkspace::new()?;
        workspace.save_settings(&instance_name, &Value::Object(updates))?;
    } else {
        let mut config = ConfigStore::load()?;
        let mut values = Map::new();
        if let Some(mib) = memory_mib {
            values.insert("memory".into(), json!(memory_kib(mib)));
        }
        if matches!(graphics_mode.as_str(), "auto" | "nvidia" | "mesa" | "software") {
            values.insert("graphics_mode".into(), json!(graphics_mode));
        }
        // глобальный пресет хранится в активном профиле запуска
        let profiles = config
            .settings()
            .get("launcher_profiles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let active_id = config.str_setting("active_launcher_profile_id");
        let mut updated = profiles;
        for profile in &mut updated {
            if profile.get("id").and_then(Value::as_str) == Some(active_id.as_str()) {
                profile["jvm_preset"] = json!(preset_id);
                break;
            }
        }
        values.insert("launcher_profiles".into(), Value::Array(updated));
        config.set_values(&Value::Object(values))?;
    }
    Ok(json!({ "ok": true, "preset_id": preset_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_history_start_and_end_roundtrip() {
        let root = std::env::temp_dir().join(format!("nivora-launchhistory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        record_launch_start(&root, "launch-1", "fabric-1.20.1", "balanced", "4096M", "1.20.1");
        record_launch_start(&root, "launch-2", "fabric-1.20.1", "fps", "2048M", "1.20.1");
        record_launch_end(&root, "launch-1", 0, 123.456, Some(12.34));
        let text = fs::read_to_string(root.join(LAUNCH_HISTORY_FILE)).unwrap();
        let entries: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(entries.len(), 2);
        let first = entries.iter().find(|entry| entry["launch_id"] == json!("launch-1")).unwrap();
        assert_eq!(first["exit_code"], json!(0));
        assert_eq!(first["elapsed_seconds"], json!(123.46));
        assert_eq!(first["time_to_window_seconds"], json!(12.34));
        let second = entries.iter().find(|entry| entry["launch_id"] == json!("launch-2")).unwrap();
        assert_eq!(second["exit_code"], json!(null));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn launch_history_trims_to_bound() {
        let root = std::env::temp_dir().join(format!("nivora-launchhistory-trim-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for index in 0..(MAX_HISTORY_ENTRIES + 5) {
            record_launch_start(&root, &format!("launch-{index}"), "vanilla", "none", "auto", "1.21");
        }
        let text = fs::read_to_string(root.join(LAUNCH_HISTORY_FILE)).unwrap();
        let entries: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(entries.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(entries.last().unwrap()["launch_id"], json!(format!("launch-{}", MAX_HISTORY_ENTRIES + 4)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn weight_classification_follows_mod_count() {
        assert_eq!(classify_weight(0, "fabric"), "vanilla");
        assert_eq!(classify_weight(10, "fabric"), "light");
        assert_eq!(classify_weight(50, "fabric"), "medium");
        assert_eq!(classify_weight(200, "fabric"), "heavy");
        assert_eq!(classify_weight(200, ""), "vanilla");
        assert_eq!(classify_weight(200, "vanilla"), "vanilla");
    }

    #[test]
    fn memory_recommendation_respects_table_and_cap() {
        assert_eq!(recommend_memory_mib("vanilla", 0, 0), 1536);
        assert_eq!(recommend_memory_mib("vanilla", 16 * 1024 * 1024, 0), 2048);
        assert_eq!(recommend_memory_mib("heavy", 32 * 1024 * 1024, 0), 10240);
        // потолок 60 % RAM: 4 ГиБ машины → не больше 2457 МиБ
        assert_eq!(recommend_memory_mib("heavy", 4 * 1024 * 1024, 0), 2457);
    }

    #[test]
    fn memory_parsing_matches_python_units() {
        assert_eq!(parse_memory_kib("auto"), Some(recommended_memory_kib()));
        assert_eq!(parse_memory_kib("2GiB"), Some(2 * 1024 * 1024));
        assert_eq!(parse_memory_kib("1536MiB"), Some(1536 * 1024));
        assert_eq!(parse_memory_kib("512KiB"), Some(512));
        assert_eq!(parse_memory_kib("2G"), Some(2 * 1024 * 1024));
        assert_eq!(parse_memory_kib("5b"), None);
        assert_eq!(parse_memory_kib("abc"), None);
    }

    #[test]
    fn minor_version_extracts_number() {
        assert_eq!(mc_minor_version("1.21.1"), 21);
        assert_eq!(mc_minor_version("1.8.9"), 8);
        assert_eq!(mc_minor_version("23w14a"), 0);
    }

    #[test]
    fn mod_recommendations_filter_by_loader_and_version() {
        let mods = recommend_optimizer_mods("fabric", "1.21.1", &[]);
        assert!(mods.iter().any(|item| item["mod_id"] == json!("sodium")));
        assert!(!mods.iter().any(|item| item["mod_id"] == json!("clumps") && item["already_installed"] == json!(true)));
        let old = recommend_optimizer_mods("fabric", "1.12.2", &[]);
        assert!(!old.iter().any(|item| item["mod_id"] == json!("starlight")));
        let installed = recommend_optimizer_mods("fabric", "1.21.1", &["sodium".to_owned()]);
        let sodium = installed.iter().find(|item| item["mod_id"] == json!("sodium")).unwrap();
        assert_eq!(sodium["already_installed"], json!(true));
    }

    #[test]
    fn presets_table_is_complete() {
        assert_eq!(PRESETS.len(), 6);
        assert!(preset_by_id("heavy_modpack").is_some());
        assert!(preset_by_id("none").unwrap().flags.is_empty());
        assert!(preset_by_id("missing").is_none());
    }
}
