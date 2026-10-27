use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use super::config;
use super::hardware::{GpuClass, HardwareProfile, Tier};
use super::instance_manifest::build_manifest;
use super::instance_workspace::InstanceWorkspace;
use super::instances::InstanceStore;
use super::runtime;

// пороги в МиБ: минимум для ванильлы, шаг на мод, потолки
const MIN_HEAP_MIB: i64 = 1024;
const VANILLA_HEAP_MIB: i64 = 2048;
const HEAP_PER_MOD_MIB: i64 = 24;
const HEAP_PER_1000_CLASSES_MIB: i64 = 12;
const SHADER_HEAP_MIB: i64 = 512;
const MAX_HEAP_MIB: i64 = 32768;
// система должна оставлять себе и GPU хотя бы четверть памяти
const SYSTEM_MEMORY_SHARE: f64 = 0.75;
// на слабом железе и малом объёме ОЗУ системе остаётся больше
const MOBILE_SYSTEM_MEMORY_SHARE: f64 = 0.65;
// шаг округления кучи
const HEAP_STEP_MIB: i64 = 512;
const MOBILE_TOTAL_MIB: i64 = 6144;
const MOBILE_HEAP_STEP_MIB: i64 = 256;
// на Linux не подбираем память, которой сейчас нет
const AVAILABLE_MEMORY_SHARE: f64 = 0.9;
// сколько последних упавших запусков считаем сигналом для совета
const CRASH_WARNING_THRESHOLD: usize = 2;

// большая дистанция рендера добавляет давления на кучу и GPU
const RENDER_DISTANCE_FREE: i64 = 12;
const RENDER_DISTANCE_HEAP_STEP_MIB: i64 = 48;
const SIMULATION_DISTANCE_FREE: i64 = 10;
const SIMULATION_DISTANCE_HEAP_STEP_MIB: i64 = 32;

const PERF_MOD_IDS: [&str; 19] = [
    "sodium", "rubidium", "embeddium", "xenon", "vulkanmod",
    "lithium", "phosphor", "starlight", "krypton", "ferritecore",
    "modernfix", "immediatelyfast", "entityculling", "cullleaves",
    "memoryleakfix", "lazydfu", "smoothboot", "chunky", "modmenu",
];
const SHADER_MOD_IDS: [&str; 3] = ["iris", "oculus", "optifine"];
const WORLDGEN_MOD_IDS: [&str; 11] = [
    "terralith", "tectonic", "biomesoplenty", "biomes_o_plenty",
    "oh_the_biomes_youll_go", "byg", "regions_unexplored", "wythers",
    "continents", "nullscape", "incendium",
];
const HEAVY_CONTENT_MOD_IDS: [&str; 13] = [
    "create", "mekanism", "gregtech", "gtceu", "thermal", "immersiveengineering",
    "botania", "appliedenergistics2", "ae2", "rftools", "twilightforest",
    "iceandfire", "ad_astra",
];


/// режим автонастройки: что важнее — картинка или кадры
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TuneMode {
    /// компромисс: настройки под железо без крайностей
    Balanced,
    /// упор в качество: максимум картинки, который железо тянет без потери FPS
    Quality,
    /// упор в оптимизацию: максимум кадров на этом железе
    Performance,
}

impl TuneMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            TuneMode::Balanced => "balanced",
            TuneMode::Quality => "quality",
            TuneMode::Performance => "performance",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            TuneMode::Balanced => "Сбалансированный",
            TuneMode::Quality => "Упор в качество",
            TuneMode::Performance => "Упор в оптимизацию",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            TuneMode::Balanced => "Настройки под железо без крайностей: ни лишней нагрузки, ни мыла",
            TuneMode::Quality => {
                "Максимум картинки, который тянет это железо: дальность, эффекты и облака включаются, но без просадок"
            }
            TuneMode::Performance => {
                "Максимум кадров: дистанции и эффекты урезаются до безопасного минимума, включается всё дешёвое"
            }
        }
    }

    pub(crate) fn from_request(request: &Value) -> TuneMode {
        Self::from_str(request.get("mode").and_then(Value::as_str).unwrap_or_default())
    }

    pub(crate) fn from_str(raw: &str) -> TuneMode {
        match raw.trim().to_lowercase().as_str() {
            "quality" | "high" => TuneMode::Quality,
            "performance" | "fps" | "optimization" => TuneMode::Performance,
            _ => TuneMode::Balanced,
        }
    }

    /// поправка к целевой дистанции рендера: качество смотрит выше, оптимизация ниже
    fn render_delta(self) -> i64 {
        match self {
            TuneMode::Balanced => 0,
            TuneMode::Quality => 2,
            TuneMode::Performance => -4,
        }
    }

    /// поправка к дистанции симуляции
    fn simulation_delta(self) -> i64 {
        match self {
            TuneMode::Balanced => 0,
            TuneMode::Quality => 2,
            TuneMode::Performance => -4,
        }
    }
}

fn version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"1\.(\d+)(?:\.(\d+))?").unwrap())
}

fn minecraft_major(version: &str) -> Option<i64> {
    version_regex()
        .captures(version)
        .and_then(|captures| captures.get(1))
        .and_then(|group| group.as_str().parse::<i64>().ok())
}

fn minecraft_minor(version: &str) -> i64 {
    version_regex()
        .captures(version)
        .and_then(|captures| captures.get(2))
        .and_then(|group| group.as_str().parse::<i64>().ok())
        .unwrap_or(0)
}

pub(crate) fn required_java_major(minecraft_version: &str) -> i64 {
    let Some(major) = minecraft_major(minecraft_version) else {
        return 17;
    };
    if major >= 21 || (major == 20 && minecraft_minor(minecraft_version) >= 5) {
        return 21;
    }
    if major >= 18 {
        return 17;
    }
    if major == 17 {
        return 16;
    }
    8
}

#[derive(Default)]
pub(crate) struct ModBuckets {
    pub(crate) perf: Vec<String>,
    pub(crate) shader: Vec<String>,
    pub(crate) worldgen: Vec<String>,
    pub(crate) heavy: Vec<String>,
}

/// раскладывает моды по весовым категориям: perf/shader/worldgen/heavy
fn classify_mods(mods: Option<&BTreeMap<String, Value>>) -> ModBuckets {
    let mut buckets = ModBuckets::default();
    let Some(mods) = mods else {
        return buckets;
    };
    for entry in mods.values() {
        let normalize = |value: &str| value.to_lowercase().replace(['-', ' '], "_");
        let mod_id = normalize(
            entry
                .get("mod_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let name = normalize(entry.get("name").and_then(Value::as_str).unwrap_or_default());
        let probe = format!("{mod_id} {name}");
        let label = if mod_id.is_empty() { name.clone() } else { mod_id.clone() };
        if PERF_MOD_IDS.iter().any(|token| probe.contains(token)) {
            buckets.perf.push(label.clone());
        }
        if SHADER_MOD_IDS.iter().any(|token| probe.contains(token)) {
            buckets.shader.push(label.clone());
        }
        if WORLDGEN_MOD_IDS.iter().any(|token| probe.contains(token)) {
            buckets.worldgen.push(label.clone());
        }
        if HEAVY_CONTENT_MOD_IDS.iter().any(|token| probe.contains(token)) {
            buckets.heavy.push(label);
        }
    }
    buckets
}

fn stat_i64(stats: &Map<String, Value>, key: &str) -> i64 {
    stats.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// читает целое значение параметра; некорректное значение считается нулём
fn option_i64(options: &Map<String, Value>, key: &str) -> i64 {
    match options.get(key) {
        None => 0,
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                0
            } else {
                text.parse().unwrap_or(0)
            }
        }
        Some(_) => 0,
    }
}

/// доля памяти системе и шаг кучи; слабое железо, малый объём ОЗУ и встроенная
/// графика оставляют системе больше памяти
fn memory_profile(system_total_kib: i64, hardware: &HardwareProfile) -> (f64, i64) {
    let total_mib = system_total_kib / 1024;
    let low_memory = total_mib > 0 && total_mib <= MOBILE_TOTAL_MIB;
    let shared_memory = !hardware.gpu_discrete || hardware.gpu_class <= GpuClass::Entry;
    if low_memory || hardware.tier == Tier::Low || shared_memory {
        (MOBILE_SYSTEM_MEMORY_SHARE, MOBILE_HEAP_STEP_MIB)
    } else {
        (SYSTEM_MEMORY_SHARE, HEAP_STEP_MIB)
    }
}

/// система и видеодрайвер тоже живут в ОЗУ: у встроенной графики резервируется
/// до 4 ГиБ под кадровые буферы, у дискретной хватает 2 ГиБ на ОС
fn system_reserve_mib(hardware: &HardwareProfile, total_mib: i64) -> i64 {
    if total_mib <= 0 {
        return 0;
    }
    if hardware.gpu_discrete {
        2048.min(total_mib / 4)
    } else {
        (2048 + (total_mib / 4).min(4096)).min(total_mib / 2)
    }
}

/// профиль применяется на любом устройстве с объёмом ОЗУ до 6 ГиБ
fn is_low_memory(system_total_kib: i64) -> bool {
    let total_mib = system_total_kib / 1024;
    total_mib > 0 && total_mib <= MOBILE_TOTAL_MIB
}

/// расчёт кучи по содержимому сборки и железу
pub(crate) fn estimate_heap_mib(
    stats: &Map<String, Value>,
    system_total_kib: i64,
    mods: Option<&BTreeMap<String, Value>>,
    options: Option<&Map<String, Value>>,
    hardware: &HardwareProfile,
) -> i64 {
    let mods_count = stat_i64(stats, "mod_count");
    let classes = stat_i64(stats, "class_count");
    let shaders = stat_i64(stats, "shader_count");
    let mut heap = VANILLA_HEAP_MIB;
    heap += mods_count * HEAP_PER_MOD_MIB;
    heap += (classes / 1000) * HEAP_PER_1000_CLASSES_MIB;
    if shaders > 0 {
        heap += SHADER_HEAP_MIB;
    }
    // драйвер GPU держит буферы в нативной памяти, но JVM резервирует место
    // под вызовы нативной графики: чем мощнее карта и больше VRAM, тем больше
    // одновременных текстур и мешей она тянет
    heap += match hardware.gpu_class {
        GpuClass::Software => 512,
        GpuClass::Entry => 256,
        GpuClass::Mid => 256,
        GpuClass::High => 384,
        GpuClass::Enthusiast => 512,
    };
    // категорийные веса: генерация мира и крупный контент держат больше
    // объектов в памяти, чем "средний" мод
    if let Some(mods) = mods {
        let buckets = classify_mods(Some(mods));
        heap += buckets.worldgen.len().min(4) as i64 * 256;
        heap += buckets.heavy.len().min(4) as i64 * 384;
    }
    if let Some(options) = options {
        // если пользователь держит дистанции выше профиля железа, память под них
        // всё равно нужна: чанки и тики живут в куче
        let render_distance = option_i64(options, "renderDistance");
        if render_distance > RENDER_DISTANCE_FREE {
            heap += (render_distance - RENDER_DISTANCE_FREE) * RENDER_DISTANCE_HEAP_STEP_MIB;
        }
        let simulation_distance = option_i64(options, "simulationDistance");
        if simulation_distance > SIMULATION_DISTANCE_FREE {
            heap += (simulation_distance - SIMULATION_DISTANCE_FREE) * SIMULATION_DISTANCE_HEAP_STEP_MIB;
        }
    }
    let (share, step_mib) = memory_profile(system_total_kib, hardware);
    let total_mib = system_total_kib / 1024;
    if total_mib > 0 {
        let share_cap = (total_mib as f64 * share) as i64;
        let reserve_cap = total_mib - system_reserve_mib(hardware, total_mib);
        let cap = share_cap.min(reserve_cap);
        heap = heap.min(cap.max(MIN_HEAP_MIB));
    }
    heap = heap.clamp(MIN_HEAP_MIB, MAX_HEAP_MIB);
    // округляем вверх до ближайшего шага, чтобы получить удобное для JVM значение
    ((heap + step_mib - 1) / step_mib) * step_mib
}

/// ограничивает рекомендацию доступной памятью Linux
fn availability_cap_mib(system_total_kib: i64) -> Option<i64> {
    if system_total_kib <= 0 {
        return None;
    }
    let available_kib = config::mem_available_kib()?;
    Some(((available_kib / 1024) as f64 * AVAILABLE_MEMORY_SHARE) as i64)
}

/// режим графики лаунчера: на Linux с дискретной NVIDIA имеет смысл PRIME-offload,
/// на программном рендере — принудительный software-режим
pub(crate) fn recommended_graphics_mode(
    hardware: &HardwareProfile,
    current: &str,
    shaders: i64,
) -> (String, String) {
    let current = current.trim();
    let keep = if current.is_empty() { "auto".to_owned() } else { current.to_owned() };
    if cfg!(target_os = "linux") && hardware.gpu_vendor == "nvidia" && hardware.gpu_discrete {
        if current == "nvidia" {
            let mut reason = String::from("PRIME-offload уже включён: игра запускается на дискретной NVIDIA");
            if shaders > 0 && hardware.gpu_class >= GpuClass::High {
                reason.push_str(", шейдеры укладываются в этот класс GPU");
            }
            return (keep, reason);
        }
        return (
            "nvidia".to_owned(),
            format!(
                "Найдена дискретная NVIDIA ({}): режим nvidia запускает игру через PRIME-offload, иначе кадры может рисовать встроенная графика",
                hardware.gpu_name
            ),
        );
    }
    if hardware.gpu_class == GpuClass::Software && current != "software" {
        return (
            "software".to_owned(),
            "Аппаратный GPU не определён: software-режим даёт предсказуемый запуск вместо падения драйвера".to_owned(),
        );
    }
    if current == "software" && hardware.gpu_class > GpuClass::Software {
        return (
            "auto".to_owned(),
            format!(
                "Установлен программный рендер, хотя найдена {}: auto вернёт аппаратную отрисовку и снимет нагрузку с CPU",
                hardware.gpu_name
            ),
        );
    }
    (keep, String::new())
}

pub(crate) fn suggest_wrapper(stats: &Map<String, Value>, current: &str) -> (String, String) {
    if cfg!(windows) || !current.trim().is_empty() {
        return (current.to_owned(), String::new());
    }
    let mods = stat_i64(stats, "mod_count");
    let shaders = stat_i64(stats, "shader_count");
    if (mods >= 20 || shaders > 0) && super::turbo::find_in_path("gamemoderun").is_some() {
        return (
            "gamemoderun".to_owned(),
            "GameMode найден в системе: поднимет приоритет CPU/GPU на время игры".to_owned(),
        );
    }
    (current.to_owned(), String::new())
}

/// возвращает значение только для распознанных числовых параметров options.txt
fn bounded_numeric_option(options: &Map<String, Value>, key: &str, min: i64, max: i64) -> Option<i64> {
    let raw = options.get(key)?.as_str()?.trim();
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = raw.parse::<i64>().ok()?;
    (min..=max).contains(&value).then_some(value)
}

fn option_text<'a>(options: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    options.get(key)?.as_str().map(str::trim)
}

fn bounded_float_option(options: &Map<String, Value>, key: &str, min: f64, max: f64) -> Option<f64> {
    let raw = option_text(options, key)?;
    let value = raw.parse::<f64>().ok()?;
    (value.is_finite() && (min..=max).contains(&value)).then_some(value)
}

fn push_text_change(
    changes: &mut BTreeMap<String, String>,
    options: &Map<String, Value>,
    key: &str,
    value: &str,
) {
    if option_text(options, key) != Some(value) {
        changes.insert(key.to_owned(), value.to_owned());
    }
}


/// ставит числовую настройку, только если ключ уже есть в options.txt
fn set_numeric(
    changes: &mut BTreeMap<String, String>,
    options: &Map<String, Value>,
    key: &str,
    min: i64,
    max: i64,
    target: i64,
) {
    if let Some(current) = bounded_numeric_option(options, key, min, max) {
        if current != target {
            changes.insert(key.to_owned(), target.to_string());
        }
    }
}

/// дробное значение в том виде, как его пишет игра: 1.0, 0.5, 0.75
fn format_option_float(value: f64) -> String {
    let one_decimal = format!("{value:.1}");
    if (one_decimal.parse::<f64>().unwrap_or(value) - value).abs() < f64::EPSILON {
        one_decimal
    } else {
        // 0.75 и подобные требуют двух знаков, иначе теряется смысл
        let two = format!("{value:.2}");
        two.trim_end_matches('0').to_owned()
    }
}

/// ставит дробную настройку (Minecraft пишет их как "1.0", "0.5" и т.п.)
fn set_float(
    changes: &mut BTreeMap<String, String>,
    options: &Map<String, Value>,
    key: &str,
    min: f64,
    max: f64,
    target: f64,
) {
    if bounded_float_option(options, key, min, max).is_some() {
        let text = format_option_float(target);
        if option_text(options, key) != Some(text.as_str()) {
            changes.insert(key.to_owned(), text);
        }
    }
}

/// к какому типу значения игра готова: тип берём из того, что уже лежит в файле
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ValueKind {
    /// 0..=N — целое число в диапазоне
    Integer,
    /// true/false
    Boolean,
    /// число с плавающей точкой в узком диапазоне
    Float,
    /// фиксированный словарь значений
    Enum,
}

/// какие ключи вообще существуют в options.txt и какого они типа
fn value_kind_for(key: &str) -> Option<ValueKind> {
    match key {
        "renderDistance"
        | "simulationDistance"
        | "maxFps"
        | "mipmapLevels"
        | "biomeBlendRadius"
        | "guiScale"
        | "chatLineSpacing"
        | "chatHeightFocused"
        | "glDebugVerbosity" => Some(ValueKind::Integer),
        "ao" | "entityShadows" | "vsync" | "enableVsync" | "pauseOnLostFocus"
        | "renderClouds" => Some(ValueKind::Boolean),
        "entityDistanceScaling" | "chatOpacity" | "textBackgroundOpacity"
        | "screenEffectScale" => Some(ValueKind::Float),
        "particles" | "graphicsMode" | "cloudStatus" | "clouds" => Some(ValueKind::Enum),
        _ => None,
    }
}

/// законный диапазон целого значения; уже сохранённое значение расширяет
/// диапазон вверх, потому что игра его приняла
fn integer_limit(key: &str) -> i64 {
    match key {
        "renderDistance" | "simulationDistance" => 32,
        "maxFps" => 10_000,
        "mipmapLevels" => 4,
        "biomeBlendRadius" => 7,
        "guiScale" => 4,
        "chatLineSpacing" | "chatHeightFocused" => 2,
        "glDebugVerbosity" => 5,
        _ => 1_000_000,
    }
}

/// допустимые значения перечислений: и новые числовые форматы, и старые
/// текстовые — конвертацию между форматами автонастройка не делает
fn allowed_tokens(key: &str) -> &'static [&'static str] {
    match key {
        "particles" => &["0", "1", "2", "minimal", "decreased", "all"],
        "graphicsMode" => &["0", "1", "2", "fast", "fancy", "fabulous"],
        "cloudStatus" => &["0", "1", "2", "3", "true", "false", "fast"],
        "clouds" => &["true", "false", "fast"],
        _ => &[],
    }
}

/// числовая и текстовая формы одного и того же параметра несовместимы:
/// игра, хранящая `cloudStatus:fast`, значение `0` может понять неверно
fn same_shape(current: &str, value: &str) -> bool {
    digits_only(current) == digits_only(value)
}

fn digits_only(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// проверяет, что значение безопасно писать в options.txt: ключ известен,
/// тип совпадает с тем, что игра уже использует, значение в законном диапазоне
pub(crate) fn option_value_is_safe(options: &Map<String, Value>, key: &str, value: &str) -> bool {
    let Some(kind) = value_kind_for(key) else {
        // незнакомый ключ не пишем никогда: чужая строка в options.txt не даёт
        // пользы, а риск записи мусора в файл игры вполне реален
        return false;
    };
    let Some(current) = option_text(options, key) else {
        return false;
    };
    match kind {
        ValueKind::Integer => {
            if !digits_only(value) || !digits_only(current) {
                return false;
            }
            match (value.parse::<i64>(), current.parse::<i64>()) {
                (Ok(new), Ok(old)) => new >= 0 && new <= integer_limit(key).max(old),
                _ => false,
            }
        }
        ValueKind::Boolean => {
            matches!(value, "true" | "false") && matches!(current, "true" | "false")
        }
        ValueKind::Float => match (value.parse::<f64>(), current.parse::<f64>()) {
            (Ok(new), Ok(old)) => new.is_finite() && (0.0..=5.0).contains(&new) && old.is_finite(),
            _ => false,
        },
        ValueKind::Enum => {
            let allowed = allowed_tokens(key);
            if allowed.is_empty() {
                return false;
            }
            same_shape(current, value) && allowed.contains(&value) && allowed.contains(&current)
        }
    }
}

/// набор настроек Minecraft под режим и железо.
/// Меняются только существующие ключи: если игра не пишет параметр в этой
/// версии, автонастройка его не создаёт
pub(crate) fn recommended_minecraft_options(
    options: &Map<String, Value>,
    hardware: &HardwareProfile,
    shader_packs: i64,
    fps_cap: i64,
    minecraft_version: &str,
    mode: TuneMode,
) -> BTreeMap<String, String> {
    let Some(version) = minecraft_major(minecraft_version) else {
        return BTreeMap::new();
    };
    let class = hardware.gpu_class;
    let weak = class <= GpuClass::Entry;
    let software = class == GpuClass::Software;
    let strong = class >= GpuClass::High;
    let shaders = shader_packs > 0;
    let quality = mode == TuneMode::Quality;
    let perf = mode == TuneMode::Performance;
    let mut changes = BTreeMap::new();

    // ---------- дистанции: потолок задаёт железо, режим сдвигает цель ----------
    if version >= 5 {
        let mut target = hardware.render_distance_target() + mode.render_delta();
        if quality && !shaders {
            // качество без шейдеров может позволить верхнюю границу класса
            target = target.max(hardware.render_distance_ceiling() - i64::from(weak));
        }
        // шейдеры стоят кадров: в оптимизации урезаем сильнее
        if shaders {
            target -= if perf { 4 } else { 2 };
        }
        let target = target
            .clamp(4, hardware.render_distance_ceiling().max(4))
            .max(4);
        if let Some(current) = bounded_numeric_option(options, "renderDistance", 2, 1024) {
            if current > target {
                push_text_change(&mut changes, options, "renderDistance", &target.to_string());
            } else if current < target {
                // поднимаем, но не больше чем на 4 чанка за прогон
                let raised = (current + 4).min(target);
                push_text_change(&mut changes, options, "renderDistance", &raised.to_string());
            }
        }
    }

    // симуляция: тики сущностей и редстоуна — это потоки CPU
    let render_after = changes
        .get("renderDistance")
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| bounded_numeric_option(options, "renderDistance", 2, 1024))
        .unwrap_or(hardware.render_distance_target());
    if version >= 18 {
        let target = (hardware.simulation_distance_ceiling() + mode.simulation_delta())
            .clamp(4, render_after.max(4));
        if let Some(current) = bounded_numeric_option(options, "simulationDistance", 2, 1024) {
            if current > target {
                push_text_change(&mut changes, options, "simulationDistance", &target.to_string());
            } else if current < target {
                let raised = (current + 4).min(target);
                push_text_change(&mut changes, options, "simulationDistance", &raised.to_string());
            }
        }
    }

    if version < 5 {
        return changes;
    }

    // ---------- ядро картинки: качество/оптимизация решают по-разному ----------
    // лимит FPS: выше частоты экрана кадры не показываются
    let refresh = if fps_cap > 0 {
        fps_cap.clamp(24, 480)
    } else {
        hardware.fps_target().unwrap_or(0)
    };
    if refresh > 0 {
        match bounded_numeric_option(options, "maxFps", 0, 10_000) {
            // безлимит на слабом железе — гарантированные рывки и нагрев,
            // на мощном это осознанный выбор пользователя
            Some(0) if weak || perf => {
                push_text_change(&mut changes, options, "maxFps", &refresh.to_string())
            }
            Some(current) if current > refresh => {
                push_text_change(&mut changes, options, "maxFps", &refresh.to_string())
            }
            _ => {}
        }
    }

    // мип-уровни: 4 — это меньше выборок на дальних блоках и меньше мерцания,
    // поэтому их максимум полезен во всех режимах
    set_numeric(&mut changes, options, "mipmapLevels", 0, 4, 4);

    // графический режим: fabulous дорог, но на мощном GPU это и есть "качество"
    match option_text(options, "graphicsMode") {
        Some("fabulous") if perf || class <= GpuClass::Mid => {
            push_text_change(&mut changes, options, "graphicsMode", "fancy")
        }
        Some("fast") if quality && strong => push_text_change(&mut changes, options, "graphicsMode", "fancy"),
        Some("fancy") if perf && weak => push_text_change(&mut changes, options, "graphicsMode", "fast"),
        _ => {}
    }

    // плавное освещение: красивый, но дорогой проход на каждый блок
    match option_text(options, "ao") {
        Some("false") if quality && !weak => push_text_change(&mut changes, options, "ao", "true"),
        Some("true") if perf || software => push_text_change(&mut changes, options, "ao", "false"),
        _ => {}
    }

    // смешивание биомов: 7 — это 7×7 выборок цвета на блок
    if let Some(current) = bounded_numeric_option(options, "biomeBlendRadius", 0, 7) {
        let target = if perf {
            match class {
                GpuClass::Software => 0,
                GpuClass::Entry => 1,
                _ => 2,
            }
        } else if quality {
            match class {
                GpuClass::Software => 0,
                GpuClass::Entry => 2,
                GpuClass::Mid => 4,
                _ => 7,
            }
        } else {
            match class {
                GpuClass::Software => 0,
                GpuClass::Entry => 2,
                GpuClass::Mid => 4,
                _ => 5,
            }
        };
        if current > target {
            push_text_change(&mut changes, options, "biomeBlendRadius", &target.to_string());
        } else if current < target && (quality || !weak) {
            push_text_change(&mut changes, options, "biomeBlendRadius", &target.to_string());
        }
    }

    // дальность прорисовки существ: модели и анимации считает CPU
    let entity_target = if perf {
        match class {
            GpuClass::Software => 0.5,
            GpuClass::Entry => 0.5,
            _ => 0.75,
        }
    } else if quality {
        match class {
            GpuClass::Software => 0.75,
            GpuClass::Entry => 1.0,
            GpuClass::Mid => 1.5,
            _ if hardware.cpu_cores >= 8 => 2.0,
            _ => 1.5,
        }
    } else {
        match class {
            GpuClass::Software => 0.5,
            GpuClass::Entry => 0.75,
            GpuClass::Mid if hardware.cpu_cores <= 6 => 1.0,
            _ => 1.25,
        }
    };
    if let Some(current) = bounded_float_option(options, "entityDistanceScaling", 0.5, 5.0) {
        if (current - entity_target).abs() > 0.01 {
            // без шейдеров и на сильном GPU можно доверять анимациям дальше
            let target = if shaders && quality {
                entity_target.min(1.5)
            } else {
                entity_target
            };
            push_text_change(
                &mut changes,
                options,
                "entityDistanceScaling",
                &format_option_float(target),
            );
        }
    }

    // тени существ — второй проход отрисовки всех моделей
    match option_text(options, "entityShadows") {
        Some("true") if perf || weak => push_text_change(&mut changes, options, "entityShadows", "false"),
        Some("false") if quality && strong => {
            push_text_change(&mut changes, options, "entityShadows", "true")
        }
        _ => {}
    }

    // частицы: 0/1/2 в новых версиях, текстовые значения — в старых
    if let Some(current) = option_text(options, "particles") {
        let numeric = match (mode, class) {
            (TuneMode::Performance, GpuClass::Software) => Some("0"),
            (TuneMode::Performance, _) => Some("1"),
            // качество не включает максимальные частицы тому, кто их не тянет
            (TuneMode::Quality, GpuClass::Software) | (TuneMode::Quality, GpuClass::Entry) => Some("1"),
            (TuneMode::Quality, _) => Some("2"),
            (TuneMode::Balanced, GpuClass::Software) => Some("0"),
            (TuneMode::Balanced, GpuClass::Entry) => Some("1"),
            (TuneMode::Balanced, _) => None,
        };
        let legacy = match (mode, class) {
            (TuneMode::Performance, _) => Some("decreased"),
            (TuneMode::Quality, GpuClass::Software) | (TuneMode::Quality, GpuClass::Entry) => {
                Some("decreased")
            }
            (TuneMode::Quality, _) => Some("all"),
            (TuneMode::Balanced, GpuClass::Software) => Some("minimal"),
            (TuneMode::Balanced, GpuClass::Entry) => Some("decreased"),
            (TuneMode::Balanced, _) => None,
        };
        let target = if current.chars().all(|c| c.is_ascii_digit()) {
            numeric
        } else {
            legacy
        };
        if let Some(target) = target {
            if current != target {
                push_text_change(&mut changes, options, "particles", target);
            }
        }
    }

    // облака: объёмные — полноэкранные слои
    for key in ["cloudStatus", "clouds"] {
        let current = match option_text(options, key) {
            Some(value) => value,
            None => continue,
        };
        let text_style = !current.chars().all(|c| c.is_ascii_digit());
        let target = if perf {
            Some("false")
        } else if quality {
            // качество включает объёмные облака только тому, кто их тянет
            Some(if strong { "true" } else { "fast" })
        } else if software {
            Some("false")
        } else if class == GpuClass::Entry {
            Some("fast")
        } else {
            None
        };
        if let Some(target) = target {
            if current != target {
                push_text_change(&mut changes, options, key, target);
            }
        }
        // в 1.21+ cloudStatus хранится числом 0..3 — конвертацию значений
        // между форматами не делаем, чтобы не получить неизвестное игре значение
        let _ = text_style;
    }

    // вертикальная синхронизация: без лимита FPS она сглаживает рывки,
    // с лимитом — только добавляет задержку
    let has_limit = bounded_numeric_option(options, "maxFps", 0, 10_000).unwrap_or(0) > 0
        || changes.contains_key("maxFps");
    for key in ["vsync", "enableVsync"] {
        match option_text(options, key) {
            Some("false") if !has_limit && (weak || perf) => {
                push_text_change(&mut changes, options, key, "true")
            }
            Some("true") if has_limit && quality => push_text_change(&mut changes, options, key, "false"),
            _ => {}
        }
    }

    // ---------- то, что раньше не трогалось вовсе ----------
    // меню паузы рисуется поверх игры: на слабом железе оно заметно роняет кадры
    match option_text(options, "pauseOnLostFocus") {
        Some("true") if perf => push_text_change(&mut changes, options, "pauseOnLostFocus", "false"),
        _ => {}
    }

    // тонкие настройки масштабирования и прозрачности интерфейса
    set_float(&mut changes, options, "chatOpacity", 0.0, 1.0, if quality { 0.9 } else { 1.0 });
    set_float(&mut changes, options, "textBackgroundOpacity", 0.0, 1.0, if perf { 0.3 } else { 0.5 });
    set_float(&mut changes, options, "screenEffectScale", 0.0, 1.0, if perf { 0.5 } else { 1.0 });
    set_numeric(&mut changes, options, "glDebugVerbosity", 0, 5, if perf { 0 } else { 1 });
    set_numeric(&mut changes, options, "chatLineSpacing", 0, 2, 0);
    set_numeric(&mut changes, options, "chatHeightFocused", 0, 2, 1);

    // GUI-масштаб: слишком мелкий интерфейс — это не "качество", а нечитаемость
    let gui_target = hardware.gui_scale();
    if gui_target > 0 {
        if let Some(current) = bounded_numeric_option(options, "guiScale", 0, 4) {
            if current != 0 && current < gui_target {
                push_text_change(&mut changes, options, "guiScale", &gui_target.to_string());
            }
        }
    }

    // автопрыжок и полный экран — вкусовые, их не трогаем; но renderClouds
    // в старых версиях дублирует clouds
    if version < 18 {
        if let Some(current) = option_text(options, "renderClouds") {
            let target = if perf {
                Some("false")
            } else if quality {
                Some("true")
            } else {
                None
            };
            if let Some(target) = target.filter(|target| *target != current) {
                push_text_change(&mut changes, options, "renderClouds", target);
            }
        }
    }

    // финальный фильтр: ни одно значение не уходит в файл без проверки типа
    // и диапазона — лучше не тронуть настройку, чем сломать запуск игры
    changes.retain(|key, value| option_value_is_safe(options, key, value));
    changes
}

/// параметр и причина изменения в options.txt
fn minecraft_change_note(
    key: &str,
    to: &str,
    options: &Map<String, Value>,
    hardware: &HardwareProfile,
    mode: TuneMode,
) -> (String, String) {
    let mode_note = format!("режим «{}»", mode.title());
    let from = options.get(key).and_then(Value::as_str).unwrap_or("—");
    let added = from == "—";
    let lowered = to.parse::<i64>().ok().and_then(|target| {
        from.parse::<i64>().ok().map(|current| current > target)
    }).unwrap_or(false);
    let direction = if added {
        "профиль дописал"
    } else if lowered {
        "снижено"
    } else {
        "поднято"
    };
    let gpu = hardware.gpu_class.title();
    let cores = hardware.cpu_cores;
    match key {
        "renderDistance" => (
            "Дистанция рендера".to_owned(),
            format!(
                "Чанков: {from} → {to} — {direction}: потолок для класса GPU «{gpu}» составляет {} чанков, а на {cores} потоках CPU целится в {}",
                hardware.render_distance_ceiling(),
                hardware.render_distance_target()
            ),
        ),
        "simulationDistance" => (
            "Дистанция симуляции".to_owned(),
            format!(
                "Чанков: {from} → {to} — тики сущностей и редстоуна считает CPU: на {cores} потоках потолок {} и не выше дистанции рендера",
                hardware.simulation_distance_ceiling()
            ),
        ),
        "maxFps" => (
            "Лимит FPS".to_owned(),
            if hardware.display_hz > 0 {
                format!(
                    "{to} кадров/с — частота дисплея {} Гц: больше кадров экран не покажет, а рендер впустую греет GPU",
                    hardware.display_hz
                )
            } else {
                format!("{to} кадров/с — лимит для класса GPU «{gpu}»: без потолка слабое железо греется и роняет кадры")
            },
        ),
        "mipmapLevels" => (
            "MIP-текстурирование".to_owned(),
            format!("Уровень {to}: мип-уровни уменьшают число выборок текстуры на дальних блоках — меньше мерцания и меньше нагрузки на GPU (класс «{gpu}»)"),
        ),
        "graphicsMode" => (
            "Качество графики".to_owned(),
            format!("{from} → {to}: «fabulous» использует второй кадровый буфер и дорог для класса GPU «{gpu}»; fancy визуально почти не отличается"),
        ),
        "ao" => (
            "Плавное освещение".to_owned(),
            format!("{from} → {to}: программный рендер не тянет дополнительный проход освещения на каждый блок"),
        ),
        "biomeBlendRadius" => (
            "Смешивание биомов".to_owned(),
            format!("Радиус {from} → {to}: каждый блок смешивания — дополнительные выборки цвета, для класса «{gpu}» это лишняя нагрузка"),
        ),
        "entityDistanceScaling" => (
            "Дальность прорисовки существ".to_owned(),
            format!("{from} → {to}: модели и анимации существ считает CPU, на {cores} потоках дальний радиус отнимает кадры"),
        ),
        "entityShadows" => (
            "Тени существ".to_owned(),
            format!("{from} → {to}: каждая тень — второй проход отрисовки всех существ, класс «{gpu}» его не окупает"),
        ),
        "particles" => (
            "Частицы".to_owned(),
            format!("{from} → {to}: плотность частиц под класс «{gpu}» — взрывы, дождь и эффекты дешевле по кадру"),
        ),
        "cloudStatus" | "clouds" => (
            "Облака".to_owned(),
            format!("{from} → {to}: объёмные облака — полноэкранные слои, для класса «{gpu}» это заметная часть бюджета кадра"),
        ),
        "vsync" | "enableVsync" => (
            "Вертикальная синхронизация".to_owned(),
            format!("{from} → {to}: лимита FPS нет, а vsync не даёт GPU рисовать кадры, которые экран не покажет"),
        ),
        "pauseOnLostFocus" => (
            "Пауза при потере фокуса".to_owned(),
            format!("{from} → {to}: без паузы игра продолжает тикать и рисовать за другими окнами — {mode_note} не платит за это кадрами"),
        ),
        "guiScale" => (
            "Масштаб интерфейса".to_owned(),
            format!("{from} → {to}: при высоте {} пикселей меньший масштаб даёт нечитаемый интерфейс", hardware.display_height),
        ),
        "chatOpacity" => (
            "Прозрачность чата".to_owned(),
            format!("{from} → {to}: непрозрачный фон чата читается без напряжения ({mode_note})"),
        ),
        "textBackgroundOpacity" => (
            "Фон текста".to_owned(),
            format!("{from} → {to}: плотный фон под подписями — читаемость без влияния на FPS"),
        ),
        "screenEffectScale" => (
            "Эффекты экрана".to_owned(),
            format!("{from} → {to}: искажения от тошноты и порталов дешевле при уменьшенном радиусе"),
        ),
        "glDebugVerbosity" => (
            "Отладочный вывод OpenGL".to_owned(),
            format!("{from} → {to}: диагностика драйвера по умолчанию пишет лишние строки в лог"),
        ),
        "chatLineSpacing" | "chatHeightFocused" => (
            "Оформление чата".to_owned(),
            format!("{from} → {to}: компактный чат занимает меньше места на экране"),
        ),
        "renderClouds" => (
            "Облака".to_owned(),
            format!("{from} → {to}: облака — полноэкранный слой, режим «{}» решает по классу GPU «{gpu}»", mode.title()),
        ),
        other => (
            other.to_owned(),
            format!("Значение подобрано под класс GPU «{gpu}» ({mode_note})"),
        ),
    }
}

/// (to, reason) для пресета JVM; пользовательские аргументы не заменяются
fn recommended_jvm_preset(
    current_preset: &str,
    flags: &[String],
    java_major: i64,
    heap_mib: i64,
    mods: i64,
    buckets: &ModBuckets,
    hardware: &HardwareProfile,
    mode: TuneMode,
) -> (String, String) {
    let current = current_preset.trim();
    if !flags.is_empty() || !current.is_empty() && current != "none" {
        return (
            current.to_owned(),
            "Пользовательские JVM-аргументы не заменяются: пресет подобран вручную".to_owned(),
        );
    }
    if java_major <= 8 {
        return (
            "compatibility".to_owned(),
            "Java 8: пресет compatibility использует SerialGC — на старых JVM он стабильнее экспериментальных флагов G1"
                .to_owned(),
        );
    }
    let heavy = mods >= 40 || buckets.heavy.len() >= 3 || buckets.worldgen.len() >= 2;
    if heavy && heap_mib >= 6144 {
        return (
            "heavy_modpack".to_owned(),
            format!(
                "{mods} модов и куча {heap_mib} МиБ: Aikar-флаги под G1 держат паузы GC короткими на больших сборках"
            ),
        );
    }
    // оптимизация просит у G1 короткие паузы даже на мощном железе
    if mode == TuneMode::Performance && hardware.cpu_cores >= 4 {
        return (
            "fps".to_owned(),
            format!(
                "Режим «{}»: пресет fps просит у G1 паузу 50 мс вместо 200 мс по умолчанию",
                mode.title()
            ),
        );
    }
    if mods >= 15 {
        return (
            "balanced".to_owned(),
            format!("{mods} модов: G1 с паузой 100 мс и параллельной обработкой ссылок уменьшает подвисания"),
        );
    }
    if hardware.gpu_class <= GpuClass::Entry || hardware.cpu_cores <= 4 {
        return (
            "fps".to_owned(),
            format!(
                "Класс GPU «{}» и {} потоков CPU: пресет fps просит у G1 паузы по 50 мс вместо 200 мс по умолчанию",
                hardware.gpu_class.title(),
                hardware.cpu_cores
            ),
        );
    }
    ("none".to_owned(), String::new())
}

/// (to, reason, note) для поля Java: авто vs явный путь vs несовместимость
fn java_runtime_report(java_major: i64, current_java_path: &str) -> (String, String, String) {
    let current_java_path = current_java_path.trim();
    if current_java_path.is_empty() {
        return (
            String::new(),
            format!("Автовыбор: лаунчер подберёт рантайм Java {java_major}+ под требование версии"),
            String::new(),
        );
    }
    let probed = runtime::java_version(std::path::Path::new(current_java_path));
    match probed {
        None => (
            String::new(),
            format!("Указанный Java не отвечает — возвращаем автовыбор (нужна Java {java_major}+)"),
            String::new(),
        ),
        Some(version) if version < java_major => (
            String::new(),
            format!(
                "Указанная Java {version} не подходит: версии нужна Java {java_major}+ — возвращаем автовыбор"
            ),
            String::new(),
        ),
        Some(version) => (
            current_java_path.to_owned(),
            format!("Указанная Java {version} подходит (нужно {java_major}+) — оставляем"),
            String::new(),
        ),
    }
}

/// всё, что нужно для советов: сборка, железо и текущие настройки
struct PlanContext<'a> {
    stats: &'a Map<String, Value>,
    options: &'a Map<String, Value>,
    meta: &'a Map<String, Value>,
    buckets: &'a ModBuckets,
    hardware: &'a HardwareProfile,
    heap_mib: i64,
    current_heap_mib: Option<i64>,
    available_heap_cap_mib: Option<i64>,
    crash_failures: usize,
    current_java_path: &'a str,
    current_preset: &'a str,
    flags: &'a [String],
}

/// информационные советы: чего не хватает сборке; не применяются
fn build_recommendations(ctx: &PlanContext<'_>) -> Vec<Value> {
    let mut recommendations: Vec<Value> = Vec::new();
    let stats = ctx.stats;
    let hardware = ctx.hardware;
    let mods = stat_i64(stats, "mod_count").max(0);
    let classes = stat_i64(stats, "class_count").max(0);
    let shaders = stat_i64(stats, "shader_count").max(0);
    let shader_programs = stat_i64(stats, "shader_programs").max(0);
    let resource_files = stat_i64(stats, "resourcepack_count").max(0);
    let loader = ctx
        .meta
        .get("loader")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let minecraft_version = ctx
        .meta
        .get("minecraft_version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let major = minecraft_major(minecraft_version).unwrap_or(0);
    let java_required = required_java_major(minecraft_version);

    if !ctx.current_java_path.trim().is_empty() {
        match runtime::java_version(std::path::Path::new(ctx.current_java_path.trim())) {
            Some(version) if version < java_required => recommendations.push(json!({
                "title": "Java не соответствует версии сборки",
                "detail": format!("Указанный рантайм Java {version}, а Minecraft {minecraft_version} требует Java {java_required}+. План вернёт выбор Java в автоматический режим; пользовательские JVM-пресеты и остальные аргументы не меняются."),
            })),
            None => recommendations.push(json!({
                "title": "Указанный рантайм Java не удалось проверить",
                "detail": format!("Путь Java задан вручную, но версию прочитать не удалось. Требуется Java {java_required}+ для Minecraft {minecraft_version}; план предложит автоматический выбор рантайма."),
            })),
            _ => {}
        }
    }

    // шейдеры: главный вопрос — тянет ли их класс GPU
    if shaders > 0 {
        if hardware.shaders_affordable() {
            recommendations.push(json!({
                "title": "Шейдеры укладываются в класс GPU",
                "detail": format!(
                    "Класс GPU «{}» ({}): найдено {shaders} файлов шейдеров и {shader_programs} объявлений программ. Дистанция рендера снижена на 2 чанка, чтобы шейдеры не отнимали кадры; точную частоту кадров покажет только запуск.",
                    hardware.gpu_class.title(),
                    hardware.gpu_name
                ),
            }));
        } else if hardware.vram_mib > 0 && hardware.vram_mib < 4096 {
            recommendations.push(json!({
                "title": "Шейдеры не для этой видеокарты",
                "detail": format!(
                    "Обнаружено {shaders} шейдерпаков, но у GPU «{}» всего {} МиБ VRAM при классе «{}». Пакеты с буферами теней займут больше памяти, чем есть: ожидайте подгрузку текстур и просадки. Автонастройка не выключает ваш пак.",
                    hardware.gpu_name,
                    hardware.vram_mib,
                    hardware.gpu_class.title()
                ),
            }));
        } else {
            recommendations.push(json!({
                "title": "Шейдеры выше класса GPU",
                "detail": format!(
                    "Обнаружено {shaders} шейдерпаков, а класс GPU «{}» рассчитан на настройки без шейдеров. Пак можно оставить, но дистанция рендера снижена на 2 чанка; если кадры проваливаются — отключите пак, а не понижайте настройки дальше.",
                    hardware.gpu_class.title()
                ),
            }));
        }
        if shaders > 0 && ctx.buckets.shader.is_empty() && matches!(loader.as_str(), "fabric" | "quilt" | "neoforge" | "forge") && major >= 16 {
            recommendations.push(json!({
                "title": "Шейдеры без обнаруженного рантайма",
                "detail": format!("В каталоге шейдеров обнаружено {shaders} файлов, но среди распознанных модов нет Iris/Oculus. Для Minecraft {minecraft_version} и загрузчика {loader} проверьте совместимый рантайм шейдеров; сам факт наличия файлов не означает, что пак активен."),
            }));
        }
    }

    if mods >= 40 && ctx.buckets.perf.is_empty() {
        let perf_names = match loader.as_str() {
            "fabric" | "quilt" => "Sodium, Lithium, FerriteCore и ImmediatelyFast",
            "forge" | "neoforge" => "Embeddium/Rubidium, ModernFix и FerriteCore",
            _ => "оптимизационные моды, совместимые с этим загрузчиком",
        };
        let class_note = if classes > 0 {
            format!("; манифест также насчитал {classes} классов модов")
        } else {
            String::new()
        };
        recommendations.push(json!({
            "title": "Много модов без распознанного оптимизатора",
            "detail": format!("В сборке обнаружено {mods} модов{class_note}, но ни один не распознан как мод производительности. Можно проверить совместимые с Minecraft {minecraft_version} и загрузчиком {loader} варианты: {perf_names}. Это рекомендация, а не гарантия прироста FPS; моды автоматически не устанавливаются."),
        }));
    }
    if mods >= 40 && !ctx.buckets.perf.is_empty() && !ctx.buckets.perf.join(" ").contains("ferritecore") {
        recommendations.push(json!({
            "title": "Проверьте поддержку FerriteCore для этой версии",
            "detail": format!("В манифесте найден оптимизатор ({}), но не распознан FerriteCore. Он может уменьшать дублирование данных блоков в куче; доступность и совместимость зависят от Minecraft {minecraft_version} и загрузчика {loader}, поэтому сначала проверьте соответствующий выпуск мода.", ctx.buckets.perf.join(", ")),
        }));
    }
    if !ctx.buckets.worldgen.is_empty() {
        recommendations.push(json!({
            "title": "Моды генерации мира могут замедлить исследование",
            "detail": format!(
                "В сборке обнаружены моды генерации ({}). Они меняют создание чанков; если задержки проявляются при первом исследовании, прегенерация мира совместимым инструментом может перенести эту работу заранее.",
                ctx.buckets.worldgen.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            ),
        }));
    }

    // дисплей: высокое разрешение дороже для GPU, а GUI-масштаб — читаемость
    if hardware.pixel_count() >= 2560 * 1440 && hardware.gpu_class <= GpuClass::Mid {
        recommendations.push(json!({
            "title": "Высокое разрешение при среднем GPU",
            "detail": format!(
                "Дисплей {}×{} — это {} мегапикселей. Класс GPU «{}» ориентирован на 1080p–1440p: если кадров не хватает, снизьте разрешение окна или включите масштабирование экрана, а не только дистанцию рендера.",
                hardware.display_width,
                hardware.display_height,
                hardware.pixel_count() / 1_000_000,
                hardware.gpu_class.title()
            ),
        }));
    }
    let gui_target = hardware.gui_scale();
    if gui_target > 0 {
        if let Some(current) = bounded_numeric_option(ctx.options, "guiScale", 0, 4) {
            if current != 0 && current < gui_target {
                recommendations.push(json!({
                    "title": "Мелкий интерфейс для этого экрана",
                    "detail": format!(
                        "При высоте {} пикселей GUI-масштаб {current} даёт очень мелкий интерфейс. Значение {gui_target} читается без прищуривания; игра применит его сама, если выставить guiScale в 0 (авто).",
                        hardware.display_height
                    ),
                }));
            }
        }
    }

    let render_distance = bounded_numeric_option(ctx.options, "renderDistance", 2, 1024);
    let render_target = hardware.render_distance_target();
    if render_distance.is_some_and(|current| current > render_target) {
        recommendations.push(json!({
            "title": "Дистанция рендера выше профиля железа",
            "detail": format!(
                "Сейчас задано {} чанков; класс GPU «{}» и {} потоков CPU рекомендуют не более {render_target}. Снижение уменьшает объём отрисовки и работы по чанкам.",
                render_distance.unwrap(),
                hardware.gpu_class.title(),
                hardware.cpu_cores
            ),
        }));
    }

    let current_fps = bounded_numeric_option(ctx.options, "maxFps", 0, 10_000);
    if hardware.display_hz > 0 {
        if let Some(current) = current_fps {
            if current > hardware.display_hz {
                recommendations.push(json!({
                    "title": "Лимит FPS выше частоты дисплея",
                    "detail": format!("Текущий лимит — {current} кадров/с, дисплей работает на {} Гц. Кадры сверх этого не показываются, но GPU продолжает их рисовать.", hardware.display_hz),
                }));
            }
        }
    }

    if let Some(current_heap) = ctx.current_heap_mib {
        if current_heap < ctx.heap_mib {
            recommendations.push(json!({
                "title": "Память JVM увеличена по расчёту",
                "detail": format!("Было задано {current_heap} МиБ, расчёт по сборке и железу даёт {} МиБ. План поднимет лимит в пределах доли ОЗУ; если игра падала с ошибкой нехватки памяти, этого должно хватить.", ctx.heap_mib),
            }));
        } else if current_heap > ctx.heap_mib + 2048 {
            recommendations.push(json!({
                "title": "Память JVM с большим запасом",
                "detail": format!("Задано {current_heap} МиБ при расчётных {} МиБ: лишняя куча удлиняет паузы GC при полной сборке и отнимает память у системы. План не уменьшает ваше значение без нужды — но при фризах есть смысл вернуть «auto».", ctx.heap_mib),
            }));
        }
    }
    if let Some(available_cap) = ctx.available_heap_cap_mib.filter(|cap| *cap > 0) {
        let effective_cap = available_cap.max(MIN_HEAP_MIB);
        if available_cap < MIN_HEAP_MIB || effective_cap < ctx.heap_mib {
            let detail = if available_cap < MIN_HEAP_MIB {
                format!("Снимок доступной системной памяти даёт расчётный лимит {available_cap} МиБ — ниже минимальной кучи лаунчера {MIN_HEAP_MIB} МиБ. План сохраняет минимум; закройте ресурсоёмкие приложения и пересчитайте перед запуском.")
            } else {
                format!("Снимок доступной системной памяти ограничил расчётную кучу значением около {available_cap} МиБ: это свободная память сейчас, а не объём ОЗУ. После закрытия тяжёлых приложений пересчитайте план.")
            };
            recommendations.push(json!({
                "title": "Рекомендация ограничена текущей доступной памятью",
                "detail": detail,
            }));
        }
    }

    if resource_files > 0 {
        recommendations.push(json!({
            "title": "Проверка установленных ресурспаков",
            "detail": format!("Манифест обнаружил {resource_files} файлов в каталоге ресурспаков. Он не определяет активный пак и разрешение текстур, поэтому по этому счётчику нельзя достоверно оценить RAM или VRAM; при проблемах сравните игру с отключёнными пользовательскими паками."),
        }));
    }

    if !matches!(hardware.gpu_vendor, "unknown" | "virtual")
        && !hardware.gpu_name.trim().is_empty()
        && hardware.gpu_name != "не определена"
    {
        let vram = if hardware.vram_mib > 0 {
            format!(", {} МиБ VRAM", hardware.vram_mib)
        } else {
            String::new()
        };
        recommendations.push(json!({
            "title": "Профиль использует обнаруженную видеокарту",
            "detail": format!(
                "Система сообщила GPU «{}» ({}), класс «{}»{vram}. По этому классу выбраны дистанция рендера ({}), качество графики и шейдерный запас; частота кадров в игре не измеряется.",
                hardware.gpu_name,
                hardware.gpu_vendor,
                hardware.gpu_class.title(),
                hardware.render_distance_target()
            ),
        }));
    }

    if !ctx.flags.is_empty() && !ctx.current_preset.trim().is_empty() {
        recommendations.push(json!({
            "title": "JVM-аргументы заданы вручную",
            "detail": format!("Найдено {} аргументов JVM и пресет «{}». Автонастройка их не заменяет: уберите аргументы, если хотите, чтобы пресет подбирался по железу автоматически.", ctx.flags.len(), ctx.current_preset),
        }));
    }

    if ctx.crash_failures >= CRASH_WARNING_THRESHOLD {
        recommendations.push(json!({
            "title": "Последние запуски падали",
            "detail": format!(
                "{0} запусков подряд завершились с ошибкой. Память повышена только по расчёту сборки; проверьте лог экземпляра, Java и совместимость модов.", ctx.crash_failures
            ),
        }));
    }
    recommendations
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_plan(
    stats: &Map<String, Value>,
    options: &Map<String, Value>,
    meta: &Map<String, Value>,
    current_settings: &Map<String, Value>,
    system_total_kib: i64,
    mods: Option<&BTreeMap<String, Value>>,
    crash_failures: usize,
    instance_path: Option<&std::path::Path>,
    fps_cap: i64,
    mode: TuneMode,
) -> Value {
    let minecraft_version = meta
        .get("minecraft_version")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| meta.get("version_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let buckets = classify_mods(mods);
    let hardware = super::hardware::profile_cached();
    let setting_str = |key: &str, default: &str| -> String {
        current_settings
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default)
            .to_owned()
    };

    // --- настройки Minecraft считаем первыми: дистанции из плана определяют
    // сколько чанков и тиков JVM придётся держать в куче
    let minecraft_options = recommended_minecraft_options(
        options,
        hardware,
        stat_i64(stats, "shader_count"),
        fps_cap,
        &minecraft_version,
        mode,
    );
    let mut planned_options = options.clone();
    for (key, value) in &minecraft_options {
        planned_options.insert(key.clone(), json!(value));
    }

    // --- память: оценка по сборке и железу, подъём до неё, но не выше доли ОЗУ
    let available_heap_cap_mib = availability_cap_mib(system_total_kib);
    let mut heap_mib = estimate_heap_mib(stats, system_total_kib, mods, Some(&planned_options), hardware);
    if let Some(cap) = available_heap_cap_mib {
        heap_mib = heap_mib.min(cap.max(MIN_HEAP_MIB));
    }
    let current_heap_mib = config::parse_memory_kib(&setting_str("memory", "auto"))
        .ok()
        .map(|kib| (kib / 1024).max(1));
    if is_low_memory(system_total_kib) {
        heap_mib = heap_mib.min(MOBILE_TOTAL_MIB * 3 / 4);
    }
    // уже выбранное пользователем большее значение не урезаем: это его запас,
    // а не ошибка профиля
    if let Some(current_heap_mib) = current_heap_mib {
        heap_mib = heap_mib.max(current_heap_mib);
    }
    heap_mib = heap_mib.clamp(MIN_HEAP_MIB, MAX_HEAP_MIB);

    let (share, _) = memory_profile(system_total_kib, hardware);
    let crash_note = if crash_failures >= CRASH_WARNING_THRESHOLD {
        format!("; последние {crash_failures} запусков падали")
    } else {
        String::new()
    };
    let java_major = required_java_major(&minecraft_version);
    let preset = setting_str("jvm_preset", "none");
    let flags: Vec<String> = current_settings
        .get("jvm_arguments")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let adjusted_flags =
        clamp_oversized_xms_arguments(&flags, heap_mib * 1024).unwrap_or_else(|| flags.clone());
    let (graphics, graphics_note) =
        recommended_graphics_mode(hardware, &setting_str("graphics_mode", "auto"), stat_i64(stats, "shader_count"));
    let (java_to, java_reason, _java_note) =
        java_runtime_report(java_major, &setting_str("java_path", ""));
    let (wrapper_to, wrapper_reason) = suggest_wrapper(stats, &setting_str("launch_wrapper", ""));

    let mods_count = stat_i64(stats, "mod_count");
    let classes = stat_i64(stats, "class_count");
    let shaders = stat_i64(stats, "shader_count");
    let (preset_to, preset_reason) = recommended_jvm_preset(
        &preset,
        &flags,
        java_major,
        heap_mib,
        mods_count,
        &buckets,
        hardware,
        mode,
    );

    let mut worldgen_suffix = String::new();
    if !buckets.worldgen.is_empty() {
        worldgen_suffix = format!(", worldgen ×{}", buckets.worldgen.len());
    }
    let heap_reason = if current_heap_mib.is_some_and(|current| current < heap_mib) {
        format!(
            "{mods_count} модов, {classes} классов, {shaders} файлов шейдеров{worldgen_suffix}; куча поднята до расчёта по сборке и классу GPU «{}»{crash_note}",
            hardware.gpu_class.title()
        )
    } else {
        format!(
            "{mods_count} модов, {classes} классов, {shaders} файлов шейдеров{worldgen_suffix}; класс GPU «{}», {} потоков CPU, доступно не более {:.0}% ОЗУ{crash_note}",
            hardware.gpu_class.title(),
            hardware.cpu_cores,
            share * 100.0
        )
    };
    let mut changes: Vec<Value> = vec![
        json!({
            "key": "memory",
            "category": "Память",
            "label": "Память JVM",
            "from": setting_str("memory", "auto"),
            "to": format!("{heap_mib}MiB"),
            "reason": heap_reason,
        }),
        json!({
            "key": "java_path",
            "category": "Java",
            "label": "Рантайм Java",
            "from": if setting_str("java_path", "").is_empty() { "авто".to_owned() } else { setting_str("java_path", "") },
            "to": if java_to.is_empty() { "авто".to_owned() } else { java_to.clone() },
            "reason": java_reason,
        }),
    ];
    if !preset_reason.is_empty() && preset_to != preset {
        changes.push(json!({
            "key": "jvm_preset",
            "category": "JVM",
            "label": "Пресет JVM",
            "from": preset.clone(),
            "to": preset_to.clone(),
            "reason": preset_reason,
        }));
    }
    if adjusted_flags != flags {
        changes.push(json!({
            "key": "jvm_arguments",
            "category": "JVM",
            "label": "Начальная память JVM",
            "from": flags,
            "to": adjusted_flags,
            "reason": "-Xms выше новой максимальной кучи; уменьшаем только этот конфликтующий флаг, остальные аргументы сохраняются",
        }));
    }
    if !graphics_note.is_empty() && graphics != setting_str("graphics_mode", "auto") {
        changes.push(json!({
            "key": "graphics_mode",
            "category": "Графика",
            "label": "Режим графики лаунчера",
            "from": setting_str("graphics_mode", "auto"),
            "to": graphics.clone(),
            "reason": graphics_note,
        }));
    }
    if !wrapper_reason.is_empty() && !wrapper_to.is_empty() {
        changes.push(json!({
            "key": "launch_wrapper",
            "category": "Запуск",
            "label": "Обёртка запуска",
            "from": if setting_str("launch_wrapper", "").is_empty() { "нет".to_owned() } else { setting_str("launch_wrapper", "") },
            "to": wrapper_to,
            "reason": wrapper_reason,
        }));
    }

    for (key, value) in &minecraft_options {
        let (label, reason) = minecraft_change_note(key, value, options, hardware, mode);
        changes.push(json!({
            "key": key,
            "category": "Minecraft",
            "label": label,
            "from": options.get(key).and_then(Value::as_str).unwrap_or("—"),
            "to": value,
            "reason": reason,
        }));
    }
    let options_changes = {
        let mut fields = Map::new();
        for (key, value) in &minecraft_options {
            fields.insert(key.clone(), json!(value));
        }
        Value::Object(fields)
    };

    let ctx = PlanContext {
        stats,
        options,
        meta,
        buckets: &buckets,
        hardware,
        heap_mib,
        current_heap_mib,
        available_heap_cap_mib,
        crash_failures,
        current_java_path: &setting_str("java_path", ""),
        current_preset: &preset,
        flags: &flags,
    };
    let mut recommendations = build_recommendations(&ctx);
    // дополнительно находим повреждённые архивы и моды для другого загрузчика
    let loader = meta
        .get("loader")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let mods_scan = instance_path.map(super::auto_repair::scan_mods).unwrap_or_default();
    if !mods_scan.is_empty() {
        let corrupted: Vec<&str> = mods_scan
            .iter()
            .filter(|mod_entry| mod_entry.enabled && mod_entry.corrupted)
            .map(|mod_entry| mod_entry.file.as_str())
            .collect();
        if !corrupted.is_empty() {
            recommendations.push(json!({
                "title": "Повреждённые файлы модов",
                "detail": format!("Не открываются как архив: {} — вероятно, загрузка оборвалась. Скачайте их заново на вкладке «Моды».", corrupted.iter().take(4).copied().collect::<Vec<_>>().join(", ")),
            }));
        }
        let scan_ids: Vec<&str> = mods_scan.iter().filter(|mod_entry| mod_entry.enabled).map(|mod_entry| mod_entry.mod_id.as_str()).collect();
        let connector = scan_ids.contains(&"connector");
        let wrong_loader: Vec<&str> = mods_scan
            .iter()
            .filter(|mod_entry| mod_entry.enabled && super::auto_repair::wrong_loader(mod_entry, &loader, connector))
            .map(|mod_entry| mod_entry.file.as_str())
            .collect();
        if !wrong_loader.is_empty() && !loader.is_empty() && loader != "vanilla" {
            recommendations.push(json!({
                "title": "Моды не для этого загрузчика",
                "detail": format!("{} — не загрузятся или уронят игру на {}.", wrong_loader.iter().take(4).copied().collect::<Vec<_>>().join(", "), loader),
            }));
        }
    }
    json!({
        "mode": mode.as_str(),
        "mode_title": mode.title(),
        "mode_description": mode.description(),
        "heap_mib": heap_mib,
        "memory": format!("{heap_mib}MiB"),
        "jvm_preset": preset_to,
        "jvm_arguments": adjusted_flags,
        "graphics_mode": graphics,
        "java_path": java_to,
        "java_major_required": java_major,
        "launch_wrapper": wrapper_to,
        "changes": changes,
        "recommendations": recommendations,
        "mod_buckets": {
            "perf": buckets.perf.len(),
            "shader": buckets.shader.len(),
            "worldgen": buckets.worldgen.len(),
            "heavy": buckets.heavy.len(),
        },
        "stats": Value::Object(stats.clone()),
        "system_memory_kib": system_total_kib,
        // профиль железа, по которому подобраны настройки Minecraft
        "hardware": super::hardware::summary(),
        // правки options.txt: ключ → значение; применяются вместе с
        // настройками лаунчера, каждая показана в changes
        "options_changes": options_changes,
        "notes": [
            format!("Версии нужна Java {java_major}+; авто-выбор рантайма сам подтянет подходящий"),
            format!(
                "Система: {} — под игру отведено не более {:.0}%",
                config::format_memory_kib(system_total_kib),
                share * 100.0
            ),
            format!(
                "Железо: класс GPU «{}», {} потоков CPU, дисплей {}×{}{}",
                hardware.gpu_class.title(),
                hardware.cpu_cores,
                hardware.display_width,
                hardware.display_height,
                if hardware.display_hz > 0 { format!("@{}Гц", hardware.display_hz) } else { String::new() }
            ),
            "Пользовательские JVM-аргументы и пресет сохраняются, кроме -Xms выше нового -Xmx".to_owned(),
        ],
    })
}

fn clamped_xms_argument(argument: &str, max_heap_kib: i64) -> Option<String> {
    let initial = config::parse_memory_kib(argument.strip_prefix("-Xms")?).ok()?;
    if initial <= max_heap_kib {
        return None;
    }
    let max_heap = config::format_memory_kib(max_heap_kib);
    Some(format!("-Xms{}", config::java_memory_value(&max_heap).ok()?))
}

fn clamp_oversized_xms_arguments(
    arguments: &[String],
    max_heap_kib: i64,
) -> Option<Vec<String>> {
    let mut changed = false;
    let adjusted = arguments
        .iter()
        .map(|argument| {
            if let Some(clamped) = clamped_xms_argument(argument, max_heap_kib) {
                changed = true;
                clamped
            } else {
                argument.clone()
            }
        })
        .collect();
    changed.then_some(adjusted)
}

/// набор полей для InstanceWorkspace.save_settings по плану
pub(crate) fn settings_from_plan(
    plan: &Value,
    current: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let plan_str = |key: &str| plan.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut values = Map::new();
    let planned_memory = plan_str("memory");
    let planned_memory_kib = config::parse_memory_kib(&planned_memory)
        .map_err(|_| "Автонастройка рассчитала некорректный размер памяти".to_owned())?;
    let current_memory = current
        .get("memory")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("auto");
    let current_memory_kib = config::parse_memory_kib(current_memory).ok();
    if current_memory_kib != Some(planned_memory_kib) {
        values.insert("memory".to_owned(), json!(planned_memory));
    }
    if let Some(arguments) = current.get("jvm_arguments").and_then(Value::as_array) {
        let mut changed = false;
        let adjusted: Vec<Value> = arguments
            .iter()
            .map(|argument| {
                let Some(raw) = argument.as_str() else {
                    return argument.clone();
                };
                if let Some(clamped) = clamped_xms_argument(raw, planned_memory_kib) {
                    changed = true;
                    json!(clamped)
                } else {
                    argument.clone()
                }
            })
            .collect();
        if changed {
            values.insert("jvm_arguments".to_owned(), json!(adjusted));
        }
    }
    // пресет задаём только когда аргументов нет: иначе пользовательские флаги
    // конфликтуют с пресетом при запуске
    let plan_preset = plan_str("jvm_preset");
    let current_preset = current
        .get("jvm_preset")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_owned();
    let has_arguments = current
        .get("jvm_arguments")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    if !plan_preset.is_empty()
        && plan_preset != current_preset
        && !has_arguments
        && current.get("jvm_preset").and_then(Value::as_str) != Some("custom")
    {
        values.insert("jvm_preset".to_owned(), json!(plan_preset));
    }
    let plan_java = plan_str("java_path");
    let current_java = current
        .get("java_path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if plan_java != current_java {
        // пустая строка = вернуть автовыбор (когда указанный путь несовместим)
        values.insert("java_path".to_owned(), json!(plan_java));
    }
    let plan_graphics = plan_str("graphics_mode");
    let current_graphics = current.get("graphics_mode").and_then(Value::as_str).unwrap_or_default();
    if !plan_graphics.is_empty() && plan_graphics != current_graphics {
        values.insert("graphics_mode".to_owned(), json!(plan_graphics));
    }
    let plan_wrapper = plan_str("launch_wrapper");
    let current_wrapper = current
        .get("launch_wrapper")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !plan_wrapper.is_empty() && plan_wrapper != current_wrapper {
        values.insert("launch_wrapper".to_owned(), json!(plan_wrapper));
    }
    Ok(values)
}

/// сколько последних запусков подряд упали (exit_code есть и != 0)
fn consecutive_launch_failures(instance_dir: &std::path::Path) -> usize {
    let history = super::optimization::launch_history_public(instance_dir);
    let mut failures = 0usize;
    for entry in history.iter().rev() {
        match entry.get("exit_code") {
            Some(Value::Number(code)) if code.as_i64() != Some(0) => failures += 1,
            Some(Value::Null) | None => continue,
            _ => break,
        }
    }
    failures
}

pub(crate) fn instance_autotune(request: &Value) -> Result<Value, String> {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let store = InstanceStore::new(&super::config::ConfigStore::load()?)?;
    let info = store.get(&name)?;
    if info.game_edition != "java" || info.kind != "client" {
        return Err("Автонастройка доступна для клиентских экземпляров Java Edition".to_owned());
    }
    let manifest = build_manifest(&info.path)?;
    let stats = manifest.stats();
    let workspace = InstanceWorkspace::new()?;
    let settings = workspace
        .settings(&name)?
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mode = TuneMode::from_request(request);
    let crash_failures = consecutive_launch_failures(&info.path);
    // ограничение FPS задаёт нативная частота дисплея; при отсутствии данных (0)
    // автонастройка берёт её из профиля железа
    let fps_cap = super::system::display_refresh_hz().unwrap_or(0);
    let mut plan = build_plan(
        &stats,
        &manifest.options,
        &manifest.meta,
        &settings,
        config::system_memory_kib(),
        Some(&manifest.mods),
        crash_failures,
        Some(info.path.as_path()),
        fps_cap,
        mode,
    );
    plan["instance_name"] = json!(info.name);
    plan["mode"] = json!(mode.as_str());
    plan["mode_title"] = json!(mode.title());
    plan["mode_description"] = json!(mode.description());

    if request.get("apply").and_then(Value::as_bool) == Some(true) {
        if info.running {
            return Err("Остановите экземпляр перед применением настроек".to_owned());
        }
        let values = settings_from_plan(&plan, &settings)?;
        let saved = workspace.save_settings(&name, &Value::Object(values))?;
        // настройки Minecraft: пишем в options.txt только ключи из плана
        let options_changes: BTreeMap<String, String> = plan
            .get("options_changes")
            .and_then(Value::as_object)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let mut minecraft_options = Map::new();
        let mut skipped_options: Vec<String> = Vec::new();
        if !options_changes.is_empty() {
            // options.txt появляется после первого запуска игры: на новом
            // экземпляре настройки лаунчера применяются, а правки options.txt
            // переносятся на следующий запуск
            match super::options_file::OptionsFile::apply(&info.path, &options_changes) {
                Ok(()) => {
                    for (key, value) in &options_changes {
                        minecraft_options.insert(key.clone(), json!(value));
                    }
                }
                Err(error) if error.contains("не найден") => {
                    skipped_options = options_changes.keys().cloned().collect();
                }
                Err(error) => return Err(error),
            }
        }
        super::events::emit_status_event("workspace-changed", "Автонастройка применена.");
        return Ok(json!({
            "plan": plan,
            "applied": true,
            "settings": saved,
            "minecraft_options": Value::Object(minecraft_options),
            // ключи, которые не записаны: options.txt ещё не создан игрой
            "skipped_minecraft_options": skipped_options,
        }));
    }
    Ok(json!({ "plan": plan, "applied": false, "settings": settings, "minecraft_options": {} }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(mod_count: i64, class_count: i64, shader_count: i64) -> Map<String, Value> {
        let mut stats = Map::new();
        stats.insert("mod_count".to_owned(), json!(mod_count));
        stats.insert("class_count".to_owned(), json!(class_count));
        stats.insert("shader_count".to_owned(), json!(shader_count));
        stats
    }

    fn mod_entry(mod_id: &str, name: &str) -> Value {
        json!({ "mod_id": mod_id, "name": name })
    }

    fn test_hardware(tier: Tier, gpu: GpuClass, cores: i64, hz: i64, width: i64, height: i64) -> HardwareProfile {
        HardwareProfile {
            tier,
            cpu_model: "Test CPU".to_owned(),
            cpu_cores: cores,
            memory_mib: 16_384,
            gpu_name: "NVIDIA Test Adapter".to_owned(),
            gpu_vendor: "nvidia",
            gpu_discrete: gpu >= GpuClass::Mid,
            gpu_class: gpu,
            vram_mib: 8192,
            display_width: width,
            display_height: height,
            display_hz: hz,
            reasons: Vec::new(),
        }
    }

    fn low() -> HardwareProfile {
        test_hardware(Tier::Low, GpuClass::Entry, 4, 60, 1366, 768)
    }

    fn mid() -> HardwareProfile {
        test_hardware(Tier::Balanced, GpuClass::Mid, 8, 75, 1920, 1080)
    }

    fn high() -> HardwareProfile {
        test_hardware(Tier::High, GpuClass::High, 12, 144, 2560, 1440)
    }

    #[test]
    fn java_major_follows_minecraft_timeline() {
        assert_eq!(required_java_major("1.21.4"), 21);
        // неизвестный формат версии использует Java 17
        assert_eq!(required_java_major("20.5"), 17);
        assert_eq!(required_java_major("1.20.5"), 21);
        assert_eq!(required_java_major("1.20.4"), 17);
        assert_eq!(required_java_major("1.18.2"), 17);
        assert_eq!(required_java_major("1.17.1"), 16);
        assert_eq!(required_java_major("1.16.5"), 8);
        assert_eq!(required_java_major("b1.7.3"), 8);
        assert_eq!(required_java_major(""), 17);
    }

    #[test]
    fn heap_estimation_grows_with_build_and_gpu_class() {
        // ванильла на среднем GPU: 2048 + 256 → округление до 2560
        let heap = estimate_heap_mib(&stats(0, 0, 0), 0, None, None, &mid());
        assert_eq!(heap, 2560);
        // 40 модов, 12000 классов, шейдеры на мощном GPU:
        // 2048 + 960 + 144 + 512 + 384 = 4048 → 4096
        let heap = estimate_heap_mib(&stats(40, 12_000, 1), 64 * 1024 * 1024, None, None, &high());
        assert_eq!(heap, 4096);
        // потолок доли системы: 4 ГиБ × 0.75 = 3072
        let small = test_hardware(Tier::Balanced, GpuClass::Mid, 8, 60, 1920, 1080);
        let heap = estimate_heap_mib(&stats(400, 0, 3), 4 * 1024 * 1024, None, None, &small);
        assert!(heap <= 3072, "куча {heap} не превышает долю системы");
        // дистанции выше профиля добавляют памяти на чанки и тики
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("32"));
        options.insert("simulationDistance".to_owned(), json!("16"));
        let far = estimate_heap_mib(&stats(0, 0, 0), 0, None, Some(&options), &mid());
        let base = estimate_heap_mib(&stats(0, 0, 0), 0, None, None, &mid());
        assert!(far > base, "дальние дистанции должны требовать больше кучи");
        // слабое железо с встроенной графикой: 8 ГиБ, резерв системы 4 ГиБ → не выше 4096
        let web = estimate_heap_mib(&stats(60, 30_000, 0), 8 * 1024 * 1024, None, None, &low());
        assert!(web <= 4096 && web >= MIN_HEAP_MIB, "ноутбук на 8 ГиБ: {web}");
        let weak = estimate_heap_mib(&stats(0, 0, 0), 4 * 1024 * 1024, None, None, &low());
        assert!(weak <= 3072 && weak >= MIN_HEAP_MIB, "слабое железо: {weak}");
    }

    #[test]
    fn memory_profile_leaves_more_to_system_on_weak_machines() {
        assert_eq!(memory_profile(4 * 1024 * 1024, &mid()).0, MOBILE_SYSTEM_MEMORY_SHARE);
        assert_eq!(memory_profile(16 * 1024 * 1024, &high()), (SYSTEM_MEMORY_SHARE, HEAP_STEP_MIB));
        // слабый тир оставляет системе больше даже при большом объёме ОЗУ
        let weak_tier = test_hardware(Tier::Low, GpuClass::High, 12, 60, 1920, 1080);
        assert_eq!(memory_profile(32 * 1024 * 1024, &weak_tier).0, MOBILE_SYSTEM_MEMORY_SHARE);
        // встроенная графика делит ОЗУ с игрой — тоже щадящая доля
        let igpu = test_hardware(Tier::High, GpuClass::Entry, 12, 60, 1920, 1080);
        assert_eq!(memory_profile(32 * 1024 * 1024, &igpu).0, MOBILE_SYSTEM_MEMORY_SHARE);
        assert_eq!(memory_profile(16 * 1024 * 1024, &mid()), (SYSTEM_MEMORY_SHARE, HEAP_STEP_MIB));
    }

    #[test]
    fn heap_never_exceeds_linux_available_memory() {
        // потолок доступности применяется в плане, а не в формуле оценки
        let Some(available_kib) = config::mem_available_kib() else {
            return;
        };
        let cap = availability_cap_mib(128 * 1024 * 1024).unwrap();
        let expected_mib = (available_kib / 1024) as f64 * AVAILABLE_MEMORY_SHARE;
        assert!((cap as f64 - expected_mib).abs() < 1.0, "cap {cap} != {expected_mib:.0}");
        assert!(availability_cap_mib(0).is_none());
    }

    #[test]
    fn classify_mods_buckets_by_ids_and_names() {
        let mut mods = BTreeMap::new();
        mods.insert("mods/sodium.jar".to_owned(), mod_entry("sodium", "Sodium"));
        mods.insert("mods/iris.jar".to_owned(), mod_entry("iris", "Iris"));
        mods.insert("mods/terralith.jar".to_owned(), mod_entry("terralith", "Terralith"));
        mods.insert("mods/create.jar".to_owned(), mod_entry("create", "Create"));
        let buckets = classify_mods(Some(&mods));
        assert_eq!(buckets.perf, vec!["sodium"]);
        assert_eq!(buckets.shader, vec!["iris"]);
        assert_eq!(buckets.worldgen, vec!["terralith"]);
        assert_eq!(buckets.heavy, vec!["create"]);
    }

    #[test]
    fn minecraft_options_follow_gpu_cpu_and_display() {
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("32"));
        options.insert("simulationDistance".to_owned(), json!("20"));
        options.insert("maxFps".to_owned(), json!("260"));
        options.insert("mipmapLevels".to_owned(), json!("0"));
        options.insert("graphicsMode".to_owned(), json!("fabulous"));
        options.insert("ao".to_owned(), json!("true"));
        options.insert("biomeBlendRadius".to_owned(), json!("7"));
        options.insert("entityDistanceScaling".to_owned(), json!("2.0"));
        options.insert("entityShadows".to_owned(), json!("true"));
        options.insert("particles".to_owned(), json!("2"));
        options.insert("cloudStatus".to_owned(), json!("true"));
        options.insert("vsync".to_owned(), json!("false"));

        // слабое железо: 4 потока, встроенный GPU, 60 Гц
        let weak = recommended_minecraft_options(&options, &low(), 0, 60, "1.20.1", TuneMode::Balanced);
        assert_eq!(weak.get("renderDistance").map(String::as_str), Some("8"));
        assert_eq!(weak.get("simulationDistance").map(String::as_str), Some("6"));
        assert_eq!(weak.get("maxFps").map(String::as_str), Some("60"));
        assert_eq!(weak.get("mipmapLevels").map(String::as_str), Some("4"));
        assert_eq!(weak.get("graphicsMode").map(String::as_str), Some("fancy"));
        assert_eq!(weak.get("biomeBlendRadius").map(String::as_str), Some("2"));
        assert_eq!(weak.get("entityDistanceScaling").map(String::as_str), Some("0.75"));
        assert_eq!(weak.get("entityShadows").map(String::as_str), Some("false"));
        assert_eq!(weak.get("particles").map(String::as_str), Some("1"));
        assert_eq!(weak.get("cloudStatus").map(String::as_str), Some("fast"));
        // плавное освещение на встроенной графике остаётся: его отключает только software
        assert!(!weak.contains_key("ao"));
        // vsync включается, только если лимит FPS не задан явно
        assert!(!weak.contains_key("vsync"));

        // мощное железо: 12 потоков, 144 Гц, 1440p
        let strong = recommended_minecraft_options(&options, &high(), 0, 144, "1.20.1", TuneMode::Balanced);
        assert_eq!(strong.get("renderDistance").map(String::as_str), Some("16"));
        assert_eq!(strong.get("simulationDistance").map(String::as_str), Some("12"));
        assert_eq!(strong.get("maxFps").map(String::as_str), Some("144"));
        // на мощном GPU fabulous остаётся выбором пользователя
        assert!(!strong.contains_key("graphicsMode"));
        assert!(!strong.contains_key("entityShadows"));
    }

    #[test]
    fn minecraft_options_raise_idle_hardware_but_only_in_steps() {
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("4"));
        options.insert("simulationDistance".to_owned(), json!("4"));
        let raised = recommended_minecraft_options(&options, &high(), 0, 0, "1.20.1", TuneMode::Balanced);
        // шаг не больше 4 чанков за один прогон
        assert_eq!(raised.get("renderDistance").map(String::as_str), Some("8"));
        assert_eq!(raised.get("simulationDistance").map(String::as_str), Some("8"));
    }

    #[test]
    fn minecraft_options_never_create_missing_or_unsupported_keys() {
        let mut unusual = Map::new();
        unusual.insert("maxFps".to_owned(), json!("0"));
        unusual.insert("renderDistance".to_owned(), json!("not-a-number"));
        unusual.insert("simulationDistance".to_owned(), json!(20));
        unusual.insert("graphicsMode".to_owned(), json!("1"));
        unusual.insert("entityDistanceScaling".to_owned(), json!("nope"));
        let changes = recommended_minecraft_options(&unusual, &low(), 0, 0, "1.20.1", TuneMode::Balanced);
        for key in changes.keys() {
            assert!(unusual.contains_key(key), "создан новый ключ {key}");
        }
        // без версии не трогаем ничего
        assert!(recommended_minecraft_options(&unusual, &low(), 0, 60, "", TuneMode::Balanced).is_empty());
        // simulationDistance появился в 1.18
        let mut old = Map::new();
        old.insert("simulationDistance".to_owned(), json!("20"));
        old.insert("renderDistance".to_owned(), json!("32"));
        let old_changes = recommended_minecraft_options(&old, &low(), 0, 60, "1.17.1", TuneMode::Balanced);
        assert!(!old_changes.contains_key("simulationDistance"));
        assert!(old_changes.contains_key("renderDistance"));
    }

    #[test]
    fn minecraft_options_handle_unlimited_fps_and_shaders() {
        let mut options = Map::new();
        options.insert("maxFps".to_owned(), json!("0"));
        options.insert("renderDistance".to_owned(), json!("16"));
        // безлимит на слабом GPU превращается в лимит по частоте дисплея
        let weak = recommended_minecraft_options(&options, &low(), 0, 0, "1.20.1", TuneMode::Balanced);
        assert_eq!(weak.get("maxFps").map(String::as_str), Some("60"));
        // на мощном остаётся безлимит пользователя
        let strong = recommended_minecraft_options(&options, &high(), 0, 0, "1.20.1", TuneMode::Balanced);
        assert!(!strong.contains_key("maxFps"));
        // шейдеры снижают дистанцию на 2 чанка
        let with_shaders = recommended_minecraft_options(&options, &high(), 2, 0, "1.20.1", TuneMode::Balanced);
        assert_eq!(with_shaders.get("renderDistance").map(String::as_str), Some("14"));
        let mut far = Map::new();
        far.insert("renderDistance".to_owned(), json!("24"));
        let far_shaders = recommended_minecraft_options(&far, &high(), 1, 0, "1.20.1", TuneMode::Balanced);
        assert_eq!(far_shaders.get("renderDistance").map(String::as_str), Some("14"));
    }

    #[test]
    fn software_renderer_gets_the_cheapest_profile() {
        let software = test_hardware(Tier::Low, GpuClass::Software, 2, 60, 1280, 720);
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("16"));
        options.insert("ao".to_owned(), json!("true"));
        options.insert("cloudStatus".to_owned(), json!("true"));
        options.insert("particles".to_owned(), json!("2"));
        options.insert("biomeBlendRadius".to_owned(), json!("5"));
        let changes = recommended_minecraft_options(&options, &software, 0, 60, "1.20.1", TuneMode::Balanced);
        assert_eq!(changes.get("renderDistance").map(String::as_str), Some("4"));
        assert_eq!(changes.get("ao").map(String::as_str), Some("false"));
        assert_eq!(changes.get("cloudStatus").map(String::as_str), Some("false"));
        assert_eq!(changes.get("particles").map(String::as_str), Some("0"));
        assert_eq!(changes.get("biomeBlendRadius").map(String::as_str), Some("0"));
    }

    #[test]
    fn jvm_preset_is_chosen_by_hardware_and_build() {
        let buckets = ModBuckets::default();
        let balanced = TuneMode::Balanced;
        // без аргументов и модов слабое железо получает пресет fps
        let (preset, reason) = recommended_jvm_preset("none", &[], 17, 2048, 0, &buckets, &low(), balanced);
        assert_eq!(preset, "fps");
        assert!(reason.contains("50 мс"));
        // большая сборка с запасом памяти — Aikar
        let (preset, _) = recommended_jvm_preset("none", &[], 17, 8192, 60, &buckets, &high(), balanced);
        assert_eq!(preset, "heavy_modpack");
        // средняя сборка — balanced
        let (preset, _) = recommended_jvm_preset("none", &[], 17, 4096, 20, &buckets, &mid(), balanced);
        assert_eq!(preset, "balanced");
        // Java 8 — compatibility (SerialGC вместо экспериментальных G1-флагов)
        let (preset, _) = recommended_jvm_preset("none", &[], 8, 2048, 5, &buckets, &low(), balanced);
        assert_eq!(preset, "compatibility");
        // пользовательские аргументы не перетираются
        let flags = vec!["-XX:+UseZGC".to_owned()];
        let (preset, reason) = recommended_jvm_preset("none", &flags, 21, 8192, 60, &buckets, &high(), balanced);
        assert_eq!(preset, "none");
        assert!(reason.contains("не заменяются"));
        // пустая ваниль на мощном железе остаётся без пресета
        let (preset, _) = recommended_jvm_preset("none", &[], 21, 4096, 2, &buckets, &high(), balanced);
        assert_eq!(preset, "none");
        // упор в оптимизацию просит короткие паузы G1 даже на мощном железе
        let (preset, _) = recommended_jvm_preset(
            "none",
            &[],
            21,
            4096,
            2,
            &buckets,
            &high(),
            TuneMode::Performance,
        );
        assert_eq!(preset, "fps");
    }

    #[test]
    fn graphics_mode_uses_prime_offload_and_software_fallback() {
        let mut hw = high();
        hw.gpu_vendor = "nvidia";
        let (mode, reason) = recommended_graphics_mode(&hw, "auto", 0);
        if cfg!(target_os = "linux") {
            assert_eq!(mode, "nvidia");
            assert!(reason.contains("PRIME"));
        } else {
            assert_eq!(mode, "auto");
        }
        // программный рендер без аппаратного GPU
        let software = test_hardware(Tier::Low, GpuClass::Software, 2, 60, 1280, 720);
        let (mode, _) = recommended_graphics_mode(&software, "auto", 0);
        assert_eq!(mode, "software");
        // обратный случай: software при найденной дискретной карте
        if cfg!(target_os = "linux") {
            let mut hw = high();
            hw.gpu_vendor = "amd";
            let (mode, reason) = recommended_graphics_mode(&hw, "software", 0);
            assert_eq!(mode, "auto");
            assert!(reason.contains("аппаратную"));
        }
    }

    fn plan_for(hardware_version: &str, options: Map<String, Value>, current: &Map<String, Value>, stats_value: Map<String, Value>, total_kib: i64) -> Value {
        let _ = hardware_version;
        let mut meta = Map::new();
        meta.insert("minecraft_version".to_owned(), json!("1.20.1"));
        meta.insert("loader".to_owned(), json!("fabric"));
        build_plan(&stats_value, &options, &meta, current, total_kib, None, 0, None, 0, TuneMode::Balanced)
    }

    #[test]
    fn plan_raises_heap_for_big_build_and_lists_minecraft_changes() {
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("32"));
        options.insert("maxFps".to_owned(), json!("300"));
        let mut current = Map::new();
        current.insert("memory".to_owned(), json!("1024MiB"));
        let plan = plan_for("1.20.1", options, &current, stats(60, 30_000, 0), 32 * 1024 * 1024);
        let heap = plan["heap_mib"].as_i64().unwrap();
        // на тесной машине расчёт ограничен свободной памятью, поэтому проверяем
        // не конкретное число, а согласованность причины с итоговой кучей
        assert!(heap >= 1024, "куча не может стать меньше выбранной: {heap}");
        let raised = heap > 1024;
        let changes = plan["changes"].as_array().unwrap();
        assert!(changes.iter().any(|change| change["key"] == json!("memory")));
        assert!(changes.iter().any(|change| change["key"] == json!("renderDistance")));
        // память поднимается, а не "сохраняется"
        let memory_change = changes
            .iter()
            .find(|change| change["key"] == json!("memory"))
            .unwrap();
        let reason = memory_change["reason"].as_str().unwrap();
        if raised {
            assert!(reason.contains("поднята"), "причина подъёма: {reason}");
        } else {
            assert!(reason.contains("класс GPU"), "причина без подъёма: {reason}");
        }
        // совет о подъёме кучи тоже есть
        let titles: Vec<&str> = plan["recommendations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["title"].as_str().unwrap())
            .collect();
        if raised {
            assert!(titles.contains(&"Память JVM увеличена по расчёту"));
        }
        // железо попало в план вместе с классом GPU и дисплеем
        assert!(plan["hardware"]["gpu_class"].is_string());
        assert!(plan["hardware"]["reasons"].as_array().is_some());
        assert!(plan["notes"].as_array().unwrap().iter().any(|note| note.as_str().unwrap_or_default().contains("класс GPU")));
    }

    #[test]
    fn plan_keeps_generous_user_memory_and_does_not_lower_it() {
        let mut current = Map::new();
        current.insert("memory".to_owned(), json!("16384MiB"));
        let heap = plan_for("1.20.1", Map::new(), &current, stats(2, 0, 0), 64 * 1024 * 1024)["heap_mib"]
            .as_i64()
            .unwrap();
        assert_eq!(heap, 16_384);
    }

    #[test]
    fn crash_failures_are_reported_without_inflating_heap() {
        let current = Map::new();
        // system_total_kib = 0 отключает поправку на свободную память, иначе два
        // вызова подряд видят разный MemAvailable и числа расходятся
        let base = plan_for("1.20.1", Map::new(), &current, stats(10, 0, 0), 0);
        let mut meta = Map::new();
        meta.insert("minecraft_version".to_owned(), json!("1.20.1"));
        let crashed = build_plan(&stats(10, 0, 0), &Map::new(), &meta, &current, 0, None, 3, None, 0, TuneMode::Balanced);
        assert_eq!(crashed["heap_mib"], base["heap_mib"]);
        let titles: Vec<&str> = crashed["recommendations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["title"].as_str().unwrap())
            .collect();
        assert!(titles.contains(&"Последние запуски падали"));
    }

    #[test]
    fn user_jvm_arguments_are_never_replaced() {
        let mut current = Map::new();
        current.insert("memory".to_owned(), json!("4096MiB"));
        current.insert("jvm_preset".to_owned(), json!("custom"));
        current.insert("jvm_arguments".to_owned(), json!(["-XX:+UseZGC"]));
        let plan = plan_for("1.20.1", Map::new(), &current, stats(150, 40_000, 1), 64 * 1024 * 1024);
        assert_eq!(plan["jvm_preset"], json!("custom"));
        assert_eq!(plan["jvm_arguments"], json!(["-XX:+UseZGC"]));
        let values = settings_from_plan(&plan, &current).unwrap();
        assert!(!values.contains_key("jvm_arguments"));
        assert!(!values.contains_key("jvm_preset"));
        let titles: Vec<&str> = plan["recommendations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["title"].as_str().unwrap())
            .collect();
        assert!(titles.contains(&"JVM-аргументы заданы вручную"));
    }

    #[test]
    fn lowering_max_heap_clamps_only_oversized_user_xms() {
        let plan = json!({
            "memory": "2048MiB",
            "java_path": "",
            "graphics_mode": "auto",
            "launch_wrapper": "",
            "jvm_preset": "fps",
        });
        let mut current = Map::new();
        current.insert("memory".to_owned(), json!("8192MiB"));
        current.insert("jvm_arguments".to_owned(), json!([
            "-Xms4G",
            "-XX:+UseG1GC",
            "-Dcustom=value",
            "-Xms1G",
            "-Xmsinvalid",
        ]));

        let values = settings_from_plan(&plan, &current).unwrap();
        assert_eq!(values["memory"], json!("2048MiB"));
        assert_eq!(
            values["jvm_arguments"],
            json!([
                "-Xms2G",
                "-XX:+UseG1GC",
                "-Dcustom=value",
                "-Xms1G",
                "-Xmsinvalid"
            ])
        );
        // пресет не задаётся, пока есть свои аргументы
        assert!(!values.contains_key("jvm_preset"));
    }

    #[test]
    fn settings_from_plan_applies_preset_only_for_untouched_instances() {
        let plan = json!({
            "memory": "4096MiB",
            "java_path": "",
            "graphics_mode": "auto",
            "launch_wrapper": "",
            "jvm_preset": "balanced",
        });
        let mut current = Map::new();
        current.insert("memory".to_owned(), json!("4096MiB"));
        current.insert("jvm_preset".to_owned(), json!("none"));
        let values = settings_from_plan(&plan, &current).unwrap();
        assert_eq!(values["jvm_preset"], json!("balanced"));
        // при наличии своих аргументов пресет не подменяется
        current.insert("jvm_arguments".to_owned(), json!(["-XX:+UseZGC"]));
        let values = settings_from_plan(&plan, &current).unwrap();
        assert!(!values.contains_key("jvm_preset"));
    }

    #[test]
    fn settings_from_plan_validates_memory_and_switches_java() {
        let plan = json!({
            "memory": "мусор",
            "jvm_preset": "none",
        });
        let error = settings_from_plan(&plan, &Map::new()).unwrap_err();
        assert_eq!(error, "Автонастройка рассчитал некорректный размер памяти".replace("рассчитал", "рассчитала"));

        let plan = json!({
            "memory": "4096MiB",
            "jvm_preset": "none",
            "java_path": "",
            "graphics_mode": "nvidia",
            "launch_wrapper": "",
        });
        let mut current = Map::new();
        current.insert("memory".to_owned(), json!("8192MiB"));
        current.insert("java_path".to_owned(), json!("/old/java"));
        current.insert("graphics_mode".to_owned(), json!("auto"));
        let values = settings_from_plan(&plan, &current).unwrap();
        assert_eq!(values["memory"], json!("4096MiB"));
        // пустой java_path в плане при непустом текущем = вернуть автовыбор
        assert_eq!(values["java_path"], json!(""));
        assert_eq!(values["graphics_mode"], json!("nvidia"));
        assert!(!values.contains_key("launch_wrapper"));
    }

    /// собирает заголовки советов для подменённого профиля железа
    fn recommendation_titles(
        _template: &PlanContext<'_>,
        hardware: &HardwareProfile,
        stats_value: &Map<String, Value>,
        options: &Map<String, Value>,
        meta: &Map<String, Value>,
        buckets: &ModBuckets,
    ) -> Vec<String> {
        let flags: Vec<String> = Vec::new();
        let ctx = PlanContext {
            stats: stats_value,
            options,
            meta,
            buckets,
            hardware,
            heap_mib: 4096,
            current_heap_mib: Some(2048),
            available_heap_cap_mib: None,
            crash_failures: 0,
            current_java_path: "",
            current_preset: "none",
            flags: &flags,
        };
        build_recommendations(&ctx)
            .iter()
            .map(|item| item["title"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn recommendations_explain_hardware_limits() {
        let stats_value = {
            let mut measured = stats(45, 90_000, 3);
            measured.insert("shader_programs".to_owned(), json!(17));
            measured.insert("resourcepack_count".to_owned(), json!(7));
            measured
        };
        let mut mods = BTreeMap::new();
        mods.insert("mods/sodium.jar".to_owned(), mod_entry("sodium", "Sodium"));
        mods.insert("mods/iris.jar".to_owned(), mod_entry("iris", "Iris"));
        mods.insert("mods/terralith.jar".to_owned(), mod_entry("terralith", "Terralith"));
        mods.insert("mods/create.jar".to_owned(), mod_entry("create", "Create"));
        let buckets = classify_mods(Some(&mods));
        let mut meta = Map::new();
        meta.insert("minecraft_version".to_owned(), json!("1.20.4"));
        meta.insert("loader".to_owned(), json!("fabric"));
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("24"));
        options.insert("maxFps".to_owned(), json!("260"));
        let hardware = low();
        let flags: Vec<String> = Vec::new();
        let ctx = PlanContext {
            stats: &stats_value,
            options: &options,
            meta: &meta,
            buckets: &buckets,
            hardware: &hardware,
            heap_mib: 4096,
            current_heap_mib: Some(2048),
            available_heap_cap_mib: None,
            crash_failures: 0,
            current_java_path: "",
            current_preset: "none",
            flags: &flags,
        };
        let recommendations = build_recommendations(&ctx);
        let titles: Vec<&str> = recommendations
            .iter()
            .map(|item| item["title"].as_str().unwrap())
            .collect();
        assert!(titles.contains(&"Шейдеры выше класса GPU"));
        assert!(titles.contains(&"Проверка установленных ресурспаков"));
        // куча 4096 против текущих 2048 — совет о подъёме обязан быть
        assert!(titles.contains(&"Память JVM увеличена по расчёту"));
        assert!(titles.contains(&"Дистанция рендера выше профиля железа"));
        assert!(titles.contains(&"Лимит FPS выше частоты дисплея"));
        assert!(titles.contains(&"Проверьте поддержку FerriteCore для этой версии"));
        assert!(titles.contains(&"Моды генерации мира могут замедлить исследование"));
        assert!(titles.contains(&"Профиль использует обнаруженную видеокарту"));
        let detail = recommendations
            .iter()
            .find(|item| item["title"] == json!("Шейдеры выше класса GPU"))
            .unwrap()["detail"]
            .as_str()
            .unwrap();
        assert!(detail.contains("3 шейдерпаков"));

        // сильный GPU: шейдеры считаются укладывающимися
        let strong = high();
        let titles = recommendation_titles(&ctx, &strong, &stats_value, &options, &meta, &buckets);
        assert!(titles.iter().any(|title| title == "Шейдеры укладываются в класс GPU"));

        // мало VRAM — отдельный совет
        let mut small_vram = high();
        small_vram.vram_mib = 2048;
        small_vram.gpu_class = GpuClass::Entry;
        let titles = recommendation_titles(&ctx, &small_vram, &stats_value, &options, &meta, &buckets);
        assert!(titles.iter().any(|title| title == "Шейдеры не для этой видеокарты"));
    }

    #[test]
    fn plan_lists_minecraft_changes_with_from_and_to() {
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("32"));
        let plan = plan_for("1.20.1", options, &Map::new(), stats(5, 0, 0), 32 * 1024 * 1024);
        let changes = plan["changes"].as_array().unwrap();
        let minecraft_change = changes
            .iter()
            .find(|change| change["category"] == json!("Minecraft") && change["key"] == json!("renderDistance"))
            .expect("правка options.txt в плане");
        assert_eq!(minecraft_change["from"], json!("32"));
        let to = minecraft_change["to"].as_str().unwrap().parse::<i64>().unwrap();
        assert!(to <= 32);
        assert!(minecraft_change["reason"].as_str().unwrap().contains("Чанков"));
        assert!(plan["options_changes"]["renderDistance"].is_string());
    }


    #[test]
    fn quality_mode_pushes_picture_without_exceeding_gpu_class() {
        let mut options = Map::new();
        for (key, value) in [
            ("renderDistance", "8"),
            ("simulationDistance", "6"),
            ("graphicsMode", "fast"),
            ("ao", "false"),
            ("biomeBlendRadius", "2"),
            ("entityDistanceScaling", "0.75"),
            ("entityShadows", "false"),
            ("particles", "1"),
            ("cloudStatus", "false"),
            ("maxFps", "60"),
            ("vsync", "true"),
        ] {
            options.insert(key.to_owned(), json!(value));
        }
        // мощное железо: качество поднимает картинку, но не выше потолка класса
        let quality = recommended_minecraft_options(&options, &high(), 0, 144, "1.20.1", TuneMode::Quality);
        let render: i64 = quality["renderDistance"].parse().unwrap();
        assert!(render > 8, "качество должно поднимать дистанцию: {render}");
        assert!(render <= high().render_distance_ceiling(), "выше потолка класса нельзя: {render}");
        assert_eq!(quality.get("graphicsMode").map(String::as_str), Some("fancy"));
        assert_eq!(quality.get("ao").map(String::as_str), Some("true"));
        assert_eq!(quality.get("entityShadows").map(String::as_str), Some("true"));
        assert_eq!(quality.get("particles").map(String::as_str), Some("2"));
        assert_eq!(quality.get("cloudStatus").map(String::as_str), Some("true"));
        assert_eq!(quality.get("biomeBlendRadius").map(String::as_str), Some("7"));
        // при лимите FPS вертикальная синхронизация только добавляет задержку
        assert_eq!(quality.get("vsync").map(String::as_str), Some("false"));

        // оптимизация на том же железе делает ровно наоборот
        let performance =
            recommended_minecraft_options(&options, &high(), 0, 144, "1.20.1", TuneMode::Performance);
        // "fast" уже самый дешёвый режим — оптимизации нечего менять
        assert!(performance.get("graphicsMode").is_none());
        // "1" — уже минимальная плотность частиц для мощного класса
        assert!(performance.get("particles").is_none());
        // уже был fast — оптимизация не переключает на "false"
        assert!(performance.get("cloudStatus").is_none());
        // лимит ниже частоты экрана остаётся выбором игрока
        assert!(performance.get("maxFps").is_none());
        // цель оптимизации для этого класса — 12 чанков, и план поднимает до неё
        // ступенью в 4 чанка, а не до потолка класса (18)
        assert_eq!(performance.get("renderDistance").map(String::as_str), Some("12"));
        let raised: i64 = performance["renderDistance"].parse().unwrap();
        assert!(raised < high().render_distance_ceiling());
    }

    #[test]
    fn quality_mode_still_respects_weak_hardware() {
        let mut options = Map::new();
        for (key, value) in [
            ("renderDistance", "32"),
            ("particles", "1"),
            ("cloudStatus", "fast"),
            ("entityDistanceScaling", "0.75"),
        ] {
            options.insert(key.to_owned(), json!(value));
        }
        let quality = recommended_minecraft_options(&options, &low(), 0, 60, "1.20.1", TuneMode::Quality);
        // "качество" не включает то, что железо не тянет: частицы и облака
        // остаются на дешёвых значениях
        assert!(quality.get("particles").is_none());
        assert!(quality.get("cloudStatus").is_none());
        let render: i64 = quality.get("renderDistance").map(|value| value.parse().unwrap()).unwrap_or(8);
        assert!(render <= low().render_distance_ceiling(), "потолок класса соблюдён: {render}");
    }

    #[test]
    fn performance_mode_squeezes_everything_relevant() {
        let mut options = Map::new();
        for (key, value) in [
            ("renderDistance", "20"),
            ("simulationDistance", "12"),
            ("graphicsMode", "fancy"),
            ("ao", "true"),
            ("entityShadows", "true"),
            ("particles", "2"),
            ("cloudStatus", "true"),
            ("maxFps", "0"),
            ("vsync", "false"),
            ("pauseOnLostFocus", "true"),
            ("screenEffectScale", "1.0"),
        ] {
            options.insert(key.to_owned(), json!(value));
        }
        let changes = recommended_minecraft_options(&options, &mid(), 0, 75, "1.20.1", TuneMode::Performance);
        assert!(changes["renderDistance"].parse::<i64>().unwrap() < 20);
        assert_eq!(changes.get("ao").map(String::as_str), Some("false"));
        assert_eq!(changes.get("entityShadows").map(String::as_str), Some("false"));
        assert_eq!(changes.get("particles").map(String::as_str), Some("1"));
        assert_eq!(changes.get("cloudStatus").map(String::as_str), Some("false"));
        assert_eq!(changes.get("maxFps").map(String::as_str), Some("75"));
        assert_eq!(changes.get("pauseOnLostFocus").map(String::as_str), Some("false"));
        assert_eq!(changes.get("screenEffectScale").map(String::as_str), Some("0.5"));
    }

    #[test]
    fn mode_round_trips_and_defaults_to_balanced() {
        assert_eq!(TuneMode::from_str("quality"), TuneMode::Quality);
        assert_eq!(TuneMode::from_str("QUALITY"), TuneMode::Quality);
        assert_eq!(TuneMode::from_str("performance"), TuneMode::Performance);
        assert_eq!(TuneMode::from_str("fps"), TuneMode::Performance);
        assert_eq!(TuneMode::from_str(""), TuneMode::Balanced);
        assert_eq!(TuneMode::from_str("чушь"), TuneMode::Balanced);
        assert_eq!(TuneMode::from_request(&json!({ "mode": "quality" })), TuneMode::Quality);
        assert_eq!(TuneMode::from_request(&json!({})), TuneMode::Balanced);
        assert_eq!(TuneMode::Quality.as_str(), "quality");
        assert!(!TuneMode::Performance.description().is_empty());
    }

    #[test]
    fn float_values_are_rendered_like_the_game_writes_them() {
        assert_eq!(format_option_float(1.0), "1.0");
        assert_eq!(format_option_float(0.5), "0.5");
        assert_eq!(format_option_float(0.75), "0.75");
        assert_eq!(format_option_float(1.25), "1.25");
        assert_eq!(format_option_float(2.0), "2.0");
    }

    #[test]
    fn untouched_settings_are_never_created() {
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("16"));
        let changes = recommended_minecraft_options(&options, &high(), 0, 144, "1.20.1", TuneMode::Quality);
        for key in changes.keys() {
            assert_eq!(key, "renderDistance", "создан ключ {key}, которого нет в options.txt");
        }
    }

    /// инвариант для любого железа и режима: автонастройка пишет только
    /// законные значения и никогда не выходит за пределы настроек игры
    #[test]
    fn every_mode_and_hardware_stays_within_legal_ranges() {
        let mut options = Map::new();
        for (key, value) in [
            ("renderDistance", "32"),
            ("simulationDistance", "32"),
            ("maxFps", "260"),
            ("mipmapLevels", "0"),
            ("graphicsMode", "fabulous"),
            ("ao", "true"),
            ("biomeBlendRadius", "7"),
            ("entityDistanceScaling", "5.0"),
            ("entityShadows", "true"),
            ("particles", "2"),
            ("cloudStatus", "3"),
            ("vsync", "true"),
            ("enableVsync", "true"),
            ("pauseOnLostFocus", "true"),
            ("screenEffectScale", "1.0"),
            ("chatOpacity", "1.0"),
            ("textBackgroundOpacity", "1.0"),
            ("glDebugVerbosity", "5"),
            ("guiScale", "4"),
            ("chatLineSpacing", "2"),
            ("chatHeightFocused", "2"),
            ("renderClouds", "true"),
        ] {
            options.insert(key.to_owned(), json!(value));
        }
        let mut machines = vec![
            test_hardware(Tier::Low, GpuClass::Software, 2, 30, 1024, 768),
            low(),
            test_hardware(Tier::Balanced, GpuClass::Mid, 6, 60, 1920, 1080),
            mid(),
            high(),
            test_hardware(Tier::High, GpuClass::Enthusiast, 32, 480, 3840, 2160),
        ];
        let mut igpu = test_hardware(Tier::Balanced, GpuClass::Entry, 8, 60, 1920, 1080);
        igpu.memory_mib = 4096;
        igpu.gpu_discrete = false;
        machines.push(igpu);

        for hardware in &machines {
            for mode in [TuneMode::Performance, TuneMode::Balanced, TuneMode::Quality] {
                for shaders in [0, 3] {
                    let changes = recommended_minecraft_options(
                        &options, hardware, shaders, hardware.display_hz, "1.20.1", mode,
                    );
                    for (key, value) in &changes {
                        // 1) ключ существует в файле и значение того же типа
                        assert!(
                            option_value_is_safe(&options, key, value),
                            "{key}={value} не прошло проверку ({:?}, шейдеров {shaders})",
                            mode
                        );
                        // 2) числа не выходят за пределы игры
                        if let Ok(number) = value.parse::<i64>() {
                            if key == "renderDistance" || key == "simulationDistance" {
                                assert!((2..=32).contains(&number), "{key} = {number}");
                            }
                            if key == "mipmapLevels" {
                                assert!((0..=4).contains(&number));
                            }
                            if key == "biomeBlendRadius" {
                                assert!((0..=7).contains(&number));
                            }
                            if key == "guiScale" {
                                assert!((0..=4).contains(&number));
                            }
                            if key == "chatLineSpacing" || key == "chatHeightFocused" {
                                assert!((0..=2).contains(&number));
                            }
                            if key == "maxFps" {
                                assert!((24..=10_000).contains(&number), "maxFps = {number}");
                            }
                        }
                        // 2б) дробные настройки остаются в своих узких пределах
                        if ["entityDistanceScaling", "chatOpacity", "textBackgroundOpacity", "screenEffectScale"]
                            .contains(&key.as_str())
                        {
                            let number: f64 = value.parse().expect("дробное значение");
                            assert!((0.0..=5.0).contains(&number), "{key} = {number}");
                        }
                        // 3) перечисления не выходят за известные значения
                        if ["particles", "graphicsMode", "cloudStatus", "clouds", "renderClouds"]
                            .contains(&key.as_str())
                        {
                            assert!(
                                allowed_tokens(key).contains(&value.as_str()),
                                "{key} = {value} не из словаря"
                            );
                        }
                        // 4) булевы остаются булевыми
                        if ["ao", "entityShadows", "vsync", "enableVsync", "pauseOnLostFocus"]
                            .contains(&key.as_str())
                        {
                            assert!(matches!(value.as_str(), "true" | "false"));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn guard_rejects_values_that_would_break_the_game() {
        let mut options = Map::new();
        options.insert("renderDistance".to_owned(), json!("16"));
        options.insert("maxFps".to_owned(), json!("120"));
        options.insert("ao".to_owned(), json!("true"));
        options.insert("particles".to_owned(), json!("1"));
        options.insert("cloudStatus".to_owned(), json!("fast"));
        options.insert("entityDistanceScaling".to_owned(), json!("1.0"));
        options.insert("graphicsMode".to_owned(), json!("low"));

        // законные значения проходят
        assert!(option_value_is_safe(&options, "renderDistance", "12"));
        assert!(option_value_is_safe(&options, "maxFps", "60"));
        assert!(option_value_is_safe(&options, "ao", "false"));
        assert!(option_value_is_safe(&options, "particles", "2"));
        assert!(option_value_is_safe(&options, "entityDistanceScaling", "0.75"));

        // диапазоны: за пределы, отрицательные, огромные
        assert!(!option_value_is_safe(&options, "renderDistance", "64"));
        assert!(!option_value_is_safe(&options, "renderDistance", "-1"));
        assert!(!option_value_is_safe(&options, "maxFps", "999999"));
        assert!(!option_value_is_safe(&options, "entityDistanceScaling", "9.0"));
        assert!(!option_value_is_safe(&options, "entityDistanceScaling", "-1.0"));

        // типы: булево вместо числа, текст вместо числа, число вместо строки
        assert!(!option_value_is_safe(&options, "ao", "1"));
        assert!(!option_value_is_safe(&options, "renderDistance", "много"));
        assert!(!option_value_is_safe(&options, "particles", "3"));
        assert!(!option_value_is_safe(&options, "particles", "true"));
        // значение не из словаря того формата, который уже использует игра
        assert!(!option_value_is_safe(&options, "cloudStatus", "0"));
        assert!(!option_value_is_safe(&options, "graphicsMode", "false"));
        // незнакомый ключ не пишем никогда
        assert!(!option_value_is_safe(&options, "graphicsModeTypo", "fast"));
        // отсутствующий ключ не создаём
        assert!(!option_value_is_safe(&options, "simulationDistance", "10"));
    }

    #[test]
    fn guard_survives_corrupted_options_values() {
        let mut options = Map::new();
        // так выглядит файл после сбоя записи или ручной правки
        options.insert("renderDistance".to_owned(), json!(""));
        options.insert("maxFps".to_owned(), json!("1e3"));
        options.insert("ao".to_owned(), json!("TRUE"));
        options.insert("particles".to_owned(), json!("\"2\""));
        options.insert("graphicsMode".to_owned(), json!("2"));
        for (key, value) in [
            ("renderDistance", "12"),
            ("maxFps", "60"),
            ("ao", "false"),
            ("particles", "1"),
        ] {
            assert!(
                !option_value_is_safe(&options, key, value),
                "повреждённое значение {key} не должно перезаписываться"
            );
        }
        // числовой graphicsMode: строку туда не пишем, число той же формы — можно
        assert!(!option_value_is_safe(&options, "graphicsMode", "fast"));
        assert!(option_value_is_safe(&options, "graphicsMode", "1"));
    }

    /// печатает матрицу решений для типовых машин во всех трёх режимах:
    /// cargo test demo_matrix -- --ignored --nocapture
    #[test]
    #[ignore]
    fn demo_matrix() {
        let mut options = Map::new();
        for (key, value) in [
            ("renderDistance", "32"),
            ("simulationDistance", "20"),
            ("maxFps", "260"),
            ("mipmapLevels", "0"),
            ("graphicsMode", "fast"),
            ("ao", "false"),
            ("biomeBlendRadius", "2"),
            ("entityDistanceScaling", "0.75"),
            ("entityShadows", "false"),
            ("particles", "1"),
            ("cloudStatus", "false"),
            ("vsync", "true"),
            ("pauseOnLostFocus", "true"),
            ("screenEffectScale", "1.0"),
            ("guiScale", "1"),
        ] {
            options.insert(key.to_owned(), json!(value));
        }
        let mut laptop = test_hardware(Tier::Low, GpuClass::Entry, 4, 60, 1366, 768);
        laptop.memory_mib = 8192;
        laptop.gpu_discrete = false;
        laptop.vram_mib = 0;
        let cases = [
            ("ноутбук 4 потока, UHD 620, 8 ГБ, 60 Гц", laptop),
            ("ПК 8 потоков, RX 6600, 16 ГБ, 75 Гц", test_hardware(Tier::Balanced, GpuClass::Mid, 8, 75, 1920, 1080)),
            ("ПК 12 потоков, RTX 3060, 32 ГБ, 144 Гц", test_hardware(Tier::High, GpuClass::High, 12, 144, 2560, 1440)),
            ("ПК 16 потоков, RTX 4080, 32 ГБ, 165 Гц", test_hardware(Tier::High, GpuClass::Enthusiast, 16, 165, 2560, 1440)),
        ];
        for (label, hardware) in cases {
            println!("\n############ {label} ############");
            for mode in [TuneMode::Performance, TuneMode::Balanced, TuneMode::Quality] {
                let heap = estimate_heap_mib(
                    &stats(60, 30_000, 0),
                    hardware.memory_mib * 1024,
                    None,
                    Some(&options),
                    &hardware,
                );
                let changes =
                    recommended_minecraft_options(&options, &hardware, 0, hardware.display_hz, "1.20.1", mode);
                println!("\n== {} == (куча ~{heap} МиБ)", mode.title());
                for (key, value) in &changes {
                    println!(
                        "  {key}: {} → {value}",
                        options.get(key).and_then(Value::as_str).unwrap_or("—")
                    );
                }
            }
        }
    }

    #[test]
    fn autotune_requires_named_java_client_instance() {
        let error = instance_autotune(&json!({ "operation": "instance_autotune" })).unwrap_err();
        assert_eq!(error, "Сначала выберите экземпляр");
    }
}
