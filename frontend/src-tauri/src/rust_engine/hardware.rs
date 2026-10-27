use serde_json::{json, Map, Value};
use std::sync::OnceLock;

use super::platform;

/// общий тир машины: остаётся для совместимости с планом автонастройки
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tier {
    Low,
    Balanced,
    High,
}

impl Tier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Tier::Low => "low",
            Tier::Balanced => "balanced",
            Tier::High => "high",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Tier::Low => "слабое",
            Tier::Balanced => "сбалансированное",
            Tier::High => "мощное",
        }
    }
}

/// класс видеокарты: основной рычаг для дистанции рендера и качества графики
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum GpuClass {
    /// программный рендер (llvmpipe, без GPU)
    Software,
    /// встроенная графика и старые дискретные карты
    Entry,
    /// средние дискретные карты
    Mid,
    /// быстрые дискретные карты
    High,
    /// топовые карты: можно позволить шейдеры и дальность
    Enthusiast,
}

impl GpuClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            GpuClass::Software => "software",
            GpuClass::Entry => "entry",
            GpuClass::Mid => "mid",
            GpuClass::High => "high",
            GpuClass::Enthusiast => "enthusiast",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            GpuClass::Software => "программный рендер",
            GpuClass::Entry => "начальный",
            GpuClass::Mid => "средний",
            GpuClass::High => "высокий",
            GpuClass::Enthusiast => "топовый",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HardwareProfile {
    pub(crate) tier: Tier,
    pub(crate) cpu_model: String,
    pub(crate) cpu_cores: i64,
    pub(crate) memory_mib: i64,
    pub(crate) gpu_name: String,
    /// nvidia/amd/intel/virtual/unknown
    pub(crate) gpu_vendor: &'static str,
    pub(crate) gpu_discrete: bool,
    /// класс GPU: потолок отрисовки для настроек Minecraft
    pub(crate) gpu_class: GpuClass,
    /// объём видеопамяти в МиБ (0 — неизвестен, например у встроенной графики)
    pub(crate) vram_mib: i64,
    /// ширина основного дисплея в пикселях (0 — неизвестна)
    pub(crate) display_width: i64,
    pub(crate) display_height: i64,
    /// частота обновления основного дисплея, Гц (0 — неизвестна)
    pub(crate) display_hz: i64,
    pub(crate) reasons: Vec<String>,
}

impl HardwareProfile {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "tier": self.tier.as_str(),
            "tier_title": self.tier.title(),
            "cpu_model": self.cpu_model,
            "cpu_cores": self.cpu_cores,
            "memory_mib": self.memory_mib,
            "gpu": self.gpu_name,
            "gpu_vendor": self.gpu_vendor,
            "gpu_discrete": self.gpu_discrete,
            "gpu_class": self.gpu_class.as_str(),
            "gpu_class_title": self.gpu_class.title(),
            "vram_mib": self.vram_mib,
            "display_width": self.display_width,
            "display_height": self.display_height,
            "display_hz": self.display_hz,
            "reasons": self.reasons,
        })
    }

    /// потолок дистанции рендера: слабый GPU не должен получать 16 чанков
    pub(crate) fn render_distance_ceiling(&self) -> i64 {
        match self.gpu_class {
            GpuClass::Software => 4,
            GpuClass::Entry => 8,
            GpuClass::Mid => 12,
            GpuClass::High => 18,
            GpuClass::Enthusiast => 24,
        }
    }

    /// целевая дистанция рендера с учётом потоков CPU: чанки грузит не только GPU
    pub(crate) fn render_distance_target(&self) -> i64 {
        let by_cpu = match self.cpu_cores {
            0..=3 => 6,
            4..=5 => 8,
            6..=7 => 10,
            8..=11 => 12,
            12..=15 => 16,
            _ => 20,
        };
        by_cpu.min(self.render_distance_ceiling())
    }

    /// потолок дистанции симуляции (тики сущностей и редстоуна — это CPU)
    pub(crate) fn simulation_distance_ceiling(&self) -> i64 {
        match self.cpu_cores {
            0..=3 => 5,
            4..=5 => 6,
            6..=7 => 8,
            8..=11 => 10,
            12..=15 => 12,
            _ => 12,
        }
    }

    /// целевой лимит FPS: выше частоты дисплея кадры просто выбрасываются
    pub(crate) fn fps_target(&self) -> Option<i64> {
        if self.display_hz <= 0 {
            return None;
        }
        Some(self.display_hz.clamp(30, 480))
    }

    /// "тяжёлый" дисплей: 2560×1440 и выше заметно дороже для GPU
    pub(crate) fn pixel_count(&self) -> i64 {
        if self.display_width <= 0 || self.display_height <= 0 {
            0
        } else {
            self.display_width * self.display_height
        }
    }

    /// GUI-масштаб, при котором интерфейс читаемый, но не гигантский
    pub(crate) fn gui_scale(&self) -> i64 {
        if self.pixel_count() == 0 {
            return 0;
        }
        // Minecraft 1.20.2+: guiScale 0 = авто, 1..=4 — множитель
        let height = self.display_height;
        if height >= 2000 {
            4
        } else if height >= 1400 {
            3
        } else if height >= 1000 {
            2
        } else {
            1
        }
    }

    /// можно ли позволить себе шейдеры на этом GPU
    pub(crate) fn shaders_affordable(&self) -> bool {
        matches!(self.gpu_class, GpuClass::High | GpuClass::Enthusiast)
    }

    fn summary_reason(&self) -> String {
        let gpu = if self.gpu_name.is_empty() || self.gpu_name == "не определена" {
            format!("видеокарта не определена — профиль по CPU и ОЗУ")
        } else if self.vram_mib > 0 {
            format!("{}, {} МиБ VRAM", self.gpu_name, self.vram_mib)
        } else {
            self.gpu_name.clone()
        };
        let display = if self.pixel_count() > 0 {
            format!(
                ", дисплей {}×{}{}",
                self.display_width,
                self.display_height,
                if self.display_hz > 0 {
                    format!("@{}Гц", self.display_hz)
                } else {
                    String::new()
                }
            )
        } else {
            String::new()
        };
        format!("{gpu}{display}")
    }
}

/// кэш на время работы лаунчера: железо не меняется между планами
pub(crate) fn profile_cached() -> &'static HardwareProfile {
    static PROFILE: OnceLock<HardwareProfile> = OnceLock::new();
    PROFILE.get_or_init(detect)
}

fn detect() -> HardwareProfile {
    let memory_mib = super::config::system_memory_kib() / 1024;
    let (cpu_model, cpu_cores) = cpu_info();
    let gpu = gpu_info();
    let (display_width, display_height) = display_resolution();
    let display_hz = super::system::display_refresh_hz().unwrap_or(0);

    let gpu_class = gpu_class_for(&gpu.name, gpu.vendor, gpu.discrete, gpu.vram_mib);
    let gpu_points = match gpu_class {
        GpuClass::Software | GpuClass::Entry => 0,
        GpuClass::Mid => 1,
        GpuClass::High => 2,
        GpuClass::Enthusiast => 2,
    };
    // оценка складывается по трём осям: память 0-2, ядра 0-2, видеокарта 0-2
    let ram_points = if memory_mib <= 6144 {
        0
    } else if memory_mib <= 16384 {
        1
    } else {
        2
    };
    let cpu_points = if cpu_cores <= 4 {
        0
    } else if cpu_cores <= 8 {
        1
    } else {
        2
    };
    let score = ram_points + cpu_points + gpu_points;
    let tier = if score <= 1 {
        Tier::Low
    } else if score <= 4 {
        Tier::Balanced
    } else {
        Tier::High
    };

    let mut profile = HardwareProfile {
        tier,
        cpu_model,
        cpu_cores,
        memory_mib,
        gpu_name: gpu.name,
        gpu_vendor: gpu.vendor,
        gpu_discrete: gpu.discrete,
        gpu_class,
        vram_mib: gpu.vram_mib,
        display_width,
        display_height,
        display_hz,
        reasons: Vec::new(),
    };

    let mut reasons = Vec::new();
    reasons.push(format!("ОЗУ {} ГиБ", format_gib(memory_mib)));
    reasons.push(if cpu_cores > 0 {
        format!("{cpu_cores} потоков CPU")
    } else {
        "CPU не определён".to_owned()
    });
    reasons.push(profile.summary_reason());
    reasons.push(format!(
        "класс GPU: {} — дистанция рендера до {} чанков",
        gpu_class.title(),
        profile.render_distance_ceiling()
    ));
    profile.reasons = reasons;
    profile
}

/// имя, производитель, дискретность и объём VRAM
struct GpuInfo {
    name: String,
    vendor: &'static str,
    discrete: bool,
    vram_mib: i64,
}

fn format_gib(mib: i64) -> String {
    let gib = mib as f64 / 1024.0;
    let text = format!("{gib:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn cpu_info() -> (String, i64) {
    let cores = std::thread::available_parallelism()
        .map(|value| value.get() as i64)
        .unwrap_or(0);
    let model = if cfg!(target_os = "linux") {
        linux_cpu_model().unwrap_or_default()
    } else if cfg!(target_os = "windows") {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default()
    } else {
        String::new()
    };
    (model, cores)
}

fn linux_cpu_model() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("model name") {
            let value = rest.trim_start_matches(['\t', ' ', ':']).trim();
            if !value.is_empty() {
                return Some(normalize_spaces(value));
            }
        }
    }
    None
}

fn normalize_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// определяет имя, производителя, дискретность и VRAM видеокарты;
/// виртуальным картам не доверяет
fn gpu_info() -> GpuInfo {
    if let Some((name, vram_mib)) = nvidia_gpu() {
        return GpuInfo { name, vendor: "nvidia", discrete: true, vram_mib };
    }
    if cfg!(target_os = "linux") {
        if let Some(info) = linux_drm_gpu() {
            return info;
        }
    }
    if cfg!(target_os = "windows") {
        if let Some((name, vram_mib)) = windows_gpu() {
            let lowered = name.to_lowercase();
            let vendor = if lowered.contains("nvidia") || lowered.contains("geforce") || lowered.contains("rtx") {
                "nvidia"
            } else if lowered.contains("amd") || lowered.contains("radeon") {
                "amd"
            } else if lowered.contains("intel") {
                "intel"
            } else {
                "unknown"
            };
            let discrete = vendor == "nvidia" || (vendor == "amd" && !lowered.contains("graphics"));
            return GpuInfo { name, vendor, discrete, vram_mib };
        }
    }
    GpuInfo {
        name: "не определена".to_owned(),
        vendor: "unknown",
        discrete: false,
        vram_mib: 0,
    }
}

/// имя и объём VRAM из nvidia-smi (Linux и Windows)
fn nvidia_gpu() -> Option<(String, i64)> {
    let output = platform::command("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        return None;
    }
    let mut parts = first.split(',').map(str::trim);
    let name = parts.next().unwrap_or(first).to_owned();
    let vram_mib = parts
        .next()
        .map(|value| value.chars().filter(char::is_ascii_digit).collect::<String>())
        .and_then(|digits| digits.parse::<i64>().ok())
        .unwrap_or(0);
    Some((normalize_spaces(name.trim_matches('"')), vram_mib))
}

/// /sys/class/drm/card*/device → вендор, имя и объём VRAM
fn linux_drm_gpu() -> Option<GpuInfo> {
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue; // card0-HDMI-A-0 обозначает разъём, а не адаптер
        }
        let device = entry.path().join("device");
        let driver_name = std::fs::read_link(device.join("driver"))
            .ok()
            .and_then(|driver| driver.file_name().map(|value| value.to_string_lossy().to_string()))
            .unwrap_or_default();
        let vendor_id = std::fs::read_to_string(device.join("vendor"))
            .ok()
            .and_then(|text| text.trim().trim_start_matches("0x").parse::<u32>().ok().or_else(|| {
                u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
            }))
            .unwrap_or(0);
        let (vendor, label) = match (driver_name.as_str(), vendor_id) {
            ("nvidia", _) | (_, 0x10de) => ("nvidia", "NVIDIA"),
            ("amdgpu", _) | (_, 0x1002) => ("amd", "AMD Radeon Graphics"),
            ("i915", _) | ("xe", _) | (_, 0x8086) => ("intel", "Intel Graphics"),
            ("virtio_gpu", _) | ("vmwgfx", _) | ("qxl", _) => ("virtual", "виртуальная видеокарта"),
            _ => continue,
        };
        let vram_mib = std::fs::read_to_string(device.join("mem_info_vram_total"))
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .map(|bytes| bytes / (1024 * 1024))
            .unwrap_or(0);
        let product = std::fs::read_to_string(device.join("product_name"))
            .map(|value| normalize_spaces(value.trim()))
            .unwrap_or_default();
        let display_name = if product.is_empty() {
            label.to_owned()
        } else {
            format!("{label} {product}")
        };
        let discrete = vendor == "nvidia" || (vendor == "amd" && product.to_lowercase().contains("rx"));
        return Some(GpuInfo {
            name: display_name,
            vendor,
            discrete,
            vram_mib,
        });
    }
    None
}

fn windows_gpu() -> Option<(String, i64)> {
    let output = platform::command("wmic")
        .args(["path", "win32_VideoController", "get", "name,AdapterRAM", "/format:csv"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.eq_ignore_ascii_case("name") || line.starts_with("Node,") {
            continue;
        }
        // формат csv: Node,AdapterRAM,Name — но порядок колонок зависит от WMI,
        // поэтому разбираем эвристически: число — это VRAM, остальное — имя
        let mut vram = 0i64;
        let mut parts: Vec<String> = Vec::new();
        for field in line.split(',').map(str::trim) {
            if !field.is_empty() && field.chars().all(|c| c.is_ascii_digit()) {
                vram = field.parse::<i64>().unwrap_or(0) / (1024 * 1024);
            } else if !field.is_empty() && field != "Node" {
                parts.push(field.to_owned());
            }
        }
        // первое поле — имя узла, имя адаптера идёт последним
        if let Some(name) = parts.pop() {
            return Some((normalize_spaces(&name), vram));
        }
    }
    None
}

/// классифицирует GPU по имени, вендору, дискретности и VRAM
pub(crate) fn gpu_class_for(name: &str, vendor: &str, discrete: bool, vram_mib: i64) -> GpuClass {
    let lowered = name.to_lowercase();
    if lowered.contains("llvmpipe")
        || lowered.contains("softpipe")
        || lowered.contains("swiftshader")
        || lowered.contains("microsoft basic render")
        || vendor == "virtual"
    {
        return GpuClass::Software;
    }
    if lowered.contains("rtx 50")
        || lowered.contains("rtx 40")
        || lowered.contains("rx 90")
        || lowered.contains("rx 79")
        || lowered.contains("arc b7")
        || vram_mib >= 16 * 1024
    {
        return GpuClass::Enthusiast;
    }
    if lowered.contains("rtx 30")
        || lowered.contains("rtx 20")
        || lowered.contains("gtx 1080")
        || lowered.contains("gtx 1070")
        || lowered.contains("rx 78")
        || lowered.contains("rx 68")
        || lowered.contains("rx 67")
        || lowered.contains("arc a7")
        || vram_mib >= 8 * 1024
    {
        return GpuClass::High;
    }
    if lowered.contains("gtx 16")
        || lowered.contains("gtx 1060")
        || lowered.contains("gtx 1050")
        || lowered.contains("gtx 970")
        || lowered.contains("rx 5")
        || lowered.contains("rx 6")
        || lowered.contains("rx 76")
        || lowered.contains("vega")
        || vram_mib >= 4 * 1024
    {
        return GpuClass::Mid;
    }
    if discrete && vendor == "nvidia" {
        // неопознанная дискретная NVIDIA — не хуже среднего уровня
        return GpuClass::Mid;
    }
    GpuClass::Entry
}

/// разрешение основного дисплея, если ОС его сообщает без графической сессии
fn display_resolution() -> (i64, i64) {
    #[cfg(target_os = "linux")]
    {
        return linux_display_resolution();
    }
    #[cfg(target_os = "windows")]
    {
        return windows_display_resolution();
    }
    #[allow(unreachable_code)]
    (0, 0)
}

#[cfg(target_os = "linux")]
fn linux_display_resolution() -> (i64, i64) {
    // xrandr отдаёт строку вида "   1920x1080     60.00*+"
    if std::env::var_os("DISPLAY").is_some() {
        if let Ok(output) = platform::command("xrandr").arg("--current").output() {
            if output.status.success() {
                if let Some(size) = parse_xrandr_resolution(&String::from_utf8_lossy(&output.stdout)) {
                    return size;
                }
            }
        }
    }
    // Wayland: wlr-randr печатает "current 2560x1440" в заголовке режима
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(output) = platform::command("wlr-randr").output() {
            if output.status.success() {
                if let Some(size) = parse_wxw_resolution(&String::from_utf8_lossy(&output.stdout)) {
                    return size;
                }
            }
        }
    }
    (0, 0)
}

/// ищет активный (с `*`) режим в выводе xrandr
pub(crate) fn parse_xrandr_resolution(text: &str) -> Option<(i64, i64)> {
    let mut connected = false;
    for line in text.lines() {
        if line.contains(" connected ") {
            connected = true;
            continue;
        }
        if line.contains(" disconnected ") {
            connected = false;
            continue;
        }
        if !connected || !line.starts_with(|c: char| c == ' ' || c == '\t') {
            continue;
        }
        let Some(token) = line.split_whitespace().next() else {
            continue;
        };
        if !token.contains('x') {
            continue;
        }
        // активный режим помечен звёздочкой
        if !line.contains('*') {
            continue;
        }
        if let Some(size) = parse_wxw(token) {
            return Some(size);
        }
    }
    None
}

/// ищет первое разрешение вида WxH в тексте (wlr-randr)
pub(crate) fn parse_wxw_resolution(text: &str) -> Option<(i64, i64)> {
    for line in text.lines() {
        let line = line.trim();
        if line.contains("current") {
            if let Some(size) = line.split_whitespace().find_map(parse_wxw) {
                return Some(size);
            }
        }
    }
    None
}

fn parse_wxw(token: &str) -> Option<(i64, i64)> {
    let (width, height) = token.split_once('x')?;
    let height = height.trim_end_matches(|c: char| !c.is_ascii_digit());
    let width = width.parse::<i64>().ok()?;
    let height = height.parse::<i64>().ok()?;
    if width < 640 || height < 480 || width > 16384 || height > 16384 {
        return None;
    }
    Some((width, height))
}

#[cfg(target_os = "windows")]
fn windows_display_resolution() -> (i64, i64) {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    };
    let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
    mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    if unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode) } == 0 {
        return (0, 0);
    }
    (mode.dmPelsWidth as i64, mode.dmPelsHeight as i64)
}

/// публичная сводка для плана автонастроек
pub(crate) fn summary() -> Map<String, Value> {
    let profile = profile_cached();
    match profile.to_json() {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_bounds_are_monotonic() {
        assert_eq!(Tier::Low.as_str(), "low");
        assert_eq!(Tier::Balanced.as_str(), "balanced");
        assert_eq!(Tier::High.as_str(), "high");
        assert!(!Tier::High.title().is_empty());
        assert!(GpuClass::Entry < GpuClass::Mid);
        assert!(GpuClass::Mid < GpuClass::High);
        assert!(GpuClass::High < GpuClass::Enthusiast);
    }

    #[test]
    fn profile_is_cached_and_consistent() {
        let first = profile_cached();
        let second = profile_cached();
        assert_eq!(first.tier, second.tier);
        assert_eq!(first.memory_mib, second.memory_mib);
        assert_eq!(first.gpu_class, second.gpu_class);
        let json = first.to_json();
        assert!(json["tier"].is_string());
        assert!(json["gpu_class"].is_string());
        assert!(json["reasons"].as_array().map(|items| !items.is_empty()).unwrap_or(false));
    }

    #[test]
    fn gpu_class_follows_name_and_vram() {
        assert_eq!(gpu_class_for("llvmpipe (LLVM 15.0.7, 256 bits)", "unknown", false, 0), GpuClass::Software);
        assert_eq!(gpu_class_for("NVIDIA GeForce RTX 4090", "nvidia", true, 24564), GpuClass::Enthusiast);
        assert_eq!(gpu_class_for("NVIDIA GeForce RTX 3060", "nvidia", true, 12288), GpuClass::High);
        assert_eq!(gpu_class_for("NVIDIA GeForce GTX 1650", "nvidia", true, 4096), GpuClass::Mid);
        assert_eq!(gpu_class_for("Intel Graphics", "intel", false, 0), GpuClass::Entry);
        // VRAM решает, если имя не распознано
        assert_eq!(gpu_class_for("Unknown Adapter", "amd", true, 8192), GpuClass::High);
        assert_eq!(gpu_class_for("виртуальная видеокарта", "virtual", false, 0), GpuClass::Software);
    }

    fn profile(gpu: GpuClass, cores: i64, hz: i64, width: i64, height: i64) -> HardwareProfile {
        HardwareProfile {
            tier: Tier::Balanced,
            cpu_model: "Test CPU".to_owned(),
            cpu_cores: cores,
            memory_mib: 16_384,
            gpu_name: "Test GPU".to_owned(),
            gpu_vendor: "unknown",
            gpu_discrete: true,
            gpu_class: gpu,
            vram_mib: 8192,
            display_width: width,
            display_height: height,
            display_hz: hz,
            reasons: Vec::new(),
        }
    }

    #[test]
    fn render_distance_is_limited_by_both_gpu_and_cpu() {
        // слабый GPU не получает 16 чанков даже на 16 потоках
        assert_eq!(profile(GpuClass::Entry, 16, 0, 0, 0).render_distance_target(), 8);
        // мощный GPU на 4 потоках ограничен CPU
        assert_eq!(profile(GpuClass::Enthusiast, 4, 0, 0, 0).render_distance_target(), 8);
        assert_eq!(profile(GpuClass::Enthusiast, 16, 0, 0, 0).render_distance_target(), 20);
        assert_eq!(profile(GpuClass::Software, 16, 0, 0, 0).render_distance_target(), 4);
        assert!(profile(GpuClass::High, 8, 0, 0, 0).simulation_distance_ceiling() <= 10);
    }

    #[test]
    fn fps_target_follows_display_and_gui_scale_follows_resolution() {
        assert_eq!(profile(GpuClass::High, 8, 0, 1920, 1080).fps_target(), None);
        assert_eq!(profile(GpuClass::High, 8, 144, 1920, 1080).fps_target(), Some(144));
        assert_eq!(profile(GpuClass::High, 8, 8, 1920, 1080).fps_target(), Some(30));
        assert_eq!(profile(GpuClass::High, 8, 144, 1920, 1080).gui_scale(), 2);
        assert_eq!(profile(GpuClass::High, 8, 144, 2560, 1440).gui_scale(), 3);
        assert_eq!(profile(GpuClass::High, 8, 144, 3840, 2160).gui_scale(), 4);
        assert_eq!(profile(GpuClass::High, 8, 144, 0, 0).gui_scale(), 0);
        assert!(profile(GpuClass::Enthusiast, 8, 144, 2560, 1440).shaders_affordable());
        assert!(!profile(GpuClass::Entry, 8, 144, 2560, 1440).shaders_affordable());
    }

    #[test]
    fn xrandr_resolution_prefers_active_mode_on_connected_output() {
        let sample = "\
Screen 0: minimum 8 x 8, current 2560 x 1440, maximum 32767 x 32767
DP-0 connected primary 2560x1440+0+0 (normal left inverted right x axis y axis) 597mm x 336mm
   2560x1440     144.00*+ 120.00    60.00
   1920x1080      60.00
HDMI-0 disconnected (normal left inverted right x axis y axis)
   3840x2160      30.00
";
        assert_eq!(parse_xrandr_resolution(sample), Some((2560, 1440)));
        assert_eq!(parse_xrandr_resolution("HDMI-0 disconnected\n   3840x2160 30*\n"), None);
        assert_eq!(parse_wxw_resolution("DP-1 \"Dell\"\n  Modes:\n    2560x1440, 143.912 Hz (current)\n"), Some((2560, 1440)));
        assert_eq!(parse_wxw("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_wxw("0x0"), None);
    }

    #[test]
    fn normalized_names_have_single_spaces() {
        assert_eq!(normalize_spaces("  AMD   Ryzen  5 "), "AMD Ryzen 5");
    }

    #[test]
    fn gib_formatting_drops_trailing_zero() {
        assert_eq!(format_gib(8192), "8");
        assert_eq!(format_gib(8192 + 512), "8.5");
    }
}
