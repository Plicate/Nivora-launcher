//! создаёт ярлыки экземпляров для рабочего стола и меню приложений
//! ярлык запускает только лаунчер с проверенным именем экземпляра и не заменяет чужие файлы
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::state;

pub const LAUNCH_ARGUMENT: &str = "--launch-instance";
const REGISTRY_FILE: &str = "registry.json";
const MAX_ICON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const ASSETS_DIR: &str = ".nivora-launcher-assets";
const MACOS_BUNDLE_PREFIX: &str = "com.nivora.launcher.shortcut.";

/// экземпляр, который попросили запустить ярлыком; выдаётся UI один раз
static PENDING_LAUNCH: Mutex<Option<String>> = Mutex::new(None);
/// сериализация изменений реестра ярлыков между параллельными вызовами
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Desktop,
    Menu,
}

impl Location {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).unwrap_or("desktop") {
            "" | "desktop" => Ok(Self::Desktop),
            "menu" => Ok(Self::Menu),
            other => Err(format!("Неизвестное место ярлыка: {other}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Menu => "menu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    WindowsLnk,
    MacApp,
    LinuxDesktop,
}

impl Format {
    fn current() -> Self {
        if cfg!(windows) {
            Self::WindowsLnk
        } else if cfg!(target_os = "macos") {
            Self::MacApp
        } else {
            Self::LinuxDesktop
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::WindowsLnk => "lnk",
            Self::MacApp => "app",
            Self::LinuxDesktop => "desktop",
        }
    }
}

/// параметры среды, передаваемые модулю, в том числе для тестов
#[derive(Debug, Clone)]
pub struct Environment {
    pub format: Format,
    pub desktop_dir: PathBuf,
    pub menu_dir: PathBuf,
    pub state_dir: PathBuf,
    pub launcher: PathBuf,
    pub fallback_icon: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub name: String,
    pub display_name: String,
    pub path: PathBuf,
}


pub fn instance_shortcut_status(request: &Value) -> Result<Value, String> {
    let environment = Environment::detect()?;
    let instance = resolve_instance(request)?;
    status(&environment, &instance)
}

pub fn instance_shortcut_create(request: &Value) -> Result<Value, String> {
    let location = Location::parse(request.get("location").and_then(Value::as_str))?;
    let environment = Environment::detect()?;
    let instance = resolve_instance(request)?;
    create(&environment, &instance, location)
}

pub fn instance_shortcut_remove(request: &Value) -> Result<Value, String> {
    let location = match request.get("location").and_then(Value::as_str) {
        None => None,
        Some(value) => Some(Location::parse(Some(value))?),
    };
    let name = instance_name(request)?;
    let environment = Environment::detect()?;
    let removed = remove(&environment, &name, location)?;
    Ok(json!({ "removed": removed }))
}

pub fn shortcut_take_launch_request(_request: &Value) -> Result<Value, String> {
    let name = PENDING_LAUNCH.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    Ok(json!({ "name": name }))
}

/// запоминает проверенное имя экземпляра для запуска
pub fn set_pending_launch(name: &str) -> bool {
    match validate_instance_name(name) {
        Ok(valid) => {
            *PENDING_LAUNCH.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(valid);
            true
        }
        Err(_) => false,
    }
}

/// читает имя экземпляра из аргумента `--launch-instance`
pub fn launch_argument<I: IntoIterator<Item = String>>(arguments: I) -> Option<String> {
    let mut iterator = arguments.into_iter();
    while let Some(argument) = iterator.next() {
        if argument == LAUNCH_ARGUMENT {
            return iterator.next().and_then(|value| validate_instance_name(&value).ok());
        }
        if let Some(value) = argument.strip_prefix(&format!("{LAUNCH_ARGUMENT}=")) {
            return validate_instance_name(value).ok();
        }
    }
    None
}

/// удаляет ярлыки экземпляра, если доступна среда рабочего стола
pub fn forget_instance(name: &str) {
    if let Ok(environment) = Environment::detect() {
        let _ = remove(&environment, name, None);
    }
}


fn instance_name(request: &Value) -> Result<String, String> {
    let raw = request
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Поле «name» обязательно для этой операции".to_owned())?;
    validate_instance_name(raw)
}

pub fn validate_instance_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 64 || state::safe_instance_name(trimmed) != trimmed {
        return Err(format!("Недопустимое имя экземпляра: {trimmed}"));
    }
    Ok(trimmed.to_owned())
}

fn resolve_instance(request: &Value) -> Result<Instance, String> {
    let name = instance_name(request)?;
    let roots = state::launcher_roots();
    let game_dir = state::settings_game_dir(&roots.config_home, &roots.data_home);
    let path = state::resolve_instance_path(&game_dir, &name).ok_or_else(|| format!("Экземпляр не найден: {name}"))?;
    let metadata = state::instance_metadata(&path);
    let display_name = metadata
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&name)
        .chars()
        .take(80)
        .collect();
    Ok(Instance { name, display_name, path })
}

pub fn status(environment: &Environment, instance: &Instance) -> Result<Value, String> {
    let registry = read_registry(&environment.state_dir);
    let entry = |location: Location| {
        let path = registry
            .get(&instance.name)
            .and_then(|items| items.get(location.as_str()))
            .map(PathBuf::from)
            .unwrap_or_else(|| shortcut_path(environment, instance, location, &registry));
        let exists = path.exists() && is_ours(environment.format, &path, &instance.name);
        json!({ "exists": exists, "path": path.to_string_lossy() })
    };
    Ok(json!({
        "supported": true,
        "platform": match environment.format {
            Format::WindowsLnk => "windows",
            Format::MacApp => "macos",
            Format::LinuxDesktop => "linux",
        },
        "desktop": entry(Location::Desktop),
        "menu": entry(Location::Menu),
    }))
}

pub fn create(environment: &Environment, instance: &Instance, location: Location) -> Result<Value, String> {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut registry = read_registry(&environment.state_dir);
    let target = shortcut_path(environment, instance, location, &registry);
    let directory = target.parent().ok_or("Некорректный путь ярлыка")?.to_path_buf();
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Не удалось создать каталог для ярлыка {}: {error}", directory.display()))?;
    if (target.exists() || is_symlink(&target)) && !is_ours(environment.format, &target, &instance.name) {
        return Err(format!(
            "В «{}» уже есть «{}», созданный не Nivora Launcher. Переименуйте его или удалите вручную.",
            directory.display(),
            file_name(&target)
        ));
    }
    // при переименовании экземпляра удаляем его старый ярлык
    if let Some(previous) = registry.get(&instance.name).and_then(|items| items.get(location.as_str())).map(PathBuf::from) {
        if previous != target && previous.exists() && is_ours(environment.format, &previous, &instance.name) {
            remove_path(&previous)?;
        }
    }
    let icon = prepare_icon(environment, instance)?;
    let icon_kind = if icon.is_some() { "instance" } else { "fallback" };
    let icon_path = icon.or_else(|| environment.fallback_icon.clone());
    match environment.format {
        Format::LinuxDesktop => write_desktop_entry(environment, instance, &target, icon_path.as_deref())?,
        Format::MacApp => write_macos_bundle(environment, instance, &target, icon_path.as_deref())?,
        Format::WindowsLnk => write_windows_link(environment, instance, &target, icon_path.as_deref())?,
    }
    registry
        .entry(instance.name.clone())
        .or_default()
        .insert(location.as_str().to_owned(), target.to_string_lossy().into_owned());
    write_registry(&environment.state_dir, &registry)?;
    Ok(json!({ "path": target.to_string_lossy(), "location": location.as_str(), "icon": icon_kind }))
}

pub fn remove(environment: &Environment, name: &str, location: Option<Location>) -> Result<Vec<String>, String> {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut registry = read_registry(&environment.state_dir);
    let mut removed = Vec::new();
    let Some(items) = registry.get_mut(name) else {
        return Ok(removed);
    };
    let keys: Vec<String> = items
        .keys()
        .filter(|key| location.map(|value| value.as_str() == key.as_str()).unwrap_or(true))
        .cloned()
        .collect();
    for key in keys {
        if let Some(path) = items.remove(&key) {
            let path = PathBuf::from(path);
            if path.exists() && is_ours(environment.format, &path, name) {
                remove_path(&path)?;
                removed.push(path.to_string_lossy().into_owned());
            }
        }
    }
    if items.is_empty() {
        registry.remove(name);
    }
    let _ = fs::remove_file(icon_base(environment, name).with_extension("ico"));
    let _ = fs::remove_file(icon_base(environment, name).with_extension("png"));
    write_registry(&environment.state_dir, &registry)?;
    Ok(removed)
}

fn shortcut_path(
    environment: &Environment,
    instance: &Instance,
    location: Location,
    registry: &BTreeMap<String, BTreeMap<String, String>>,
) -> PathBuf {
    let directory = match location {
        Location::Desktop => environment.desktop_dir.clone(),
        Location::Menu => environment.menu_dir.clone(),
    };
    let extension = environment.format.extension();
    let base = sanitize_file_stem(&instance.display_name, &instance.name);
    let candidate = directory.join(format!("{base}.{extension}"));
    // при совпадении имён добавляем имя каталога экземпляра
    let taken_by_other = registry.iter().any(|(owner, items)| {
        owner != &instance.name && items.values().any(|path| Path::new(path) == candidate)
    });
    if taken_by_other {
        directory.join(format!("{base} ({}).{extension}", instance.name))
    } else {
        candidate
    }
}

/// очищает отображаемое имя для пути на Windows, macOS и Linux
pub fn sanitize_file_stem(display_name: &str, fallback: &str) -> String {
    let cleaned: String = display_name
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.trim_matches(|c: char| c == '.' || c == ' ').chars().take(80).collect();
    let stem = if trimmed.is_empty() { fallback.to_owned() } else { trimmed };
    let upper = stem.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str())
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        format!("{stem}_")
    } else {
        stem
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.file_type().is_symlink()).unwrap_or(false)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

fn remove_path(path: &Path) -> Result<(), String> {
    let result = if path.is_dir() && !is_symlink(path) {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|error| format!("Не удалось удалить ярлык {}: {error}", path.display()))
}

/// проверяет, что ярлык принадлежит лаунчеру и этому экземпляру
fn is_ours(format: Format, path: &Path, name: &str) -> bool {
    if is_symlink(path) {
        return false;
    }
    match format {
        Format::LinuxDesktop => fs::read_to_string(path)
            .map(|content| content.lines().any(|line| line.trim() == format!("X-Nivora-Instance={name}")))
            .unwrap_or(false),
        Format::MacApp => fs::read_to_string(path.join("Contents/Info.plist"))
            .map(|content| content.contains(&format!("<string>{MACOS_BUNDLE_PREFIX}{name}</string>")))
            .unwrap_or(false),
        Format::WindowsLnk => fs::read(path)
            .map(|bytes| {
                let marker: Vec<u8> = format!("{LAUNCH_ARGUMENT} \"{name}\"")
                    .encode_utf16()
                    .flat_map(|unit| unit.to_le_bytes())
                    .collect();
                bytes.windows(marker.len()).any(|window| window == marker.as_slice())
            })
            .unwrap_or(false),
    }
}


fn read_registry(state_dir: &Path) -> BTreeMap<String, BTreeMap<String, String>> {
    state::read_bounded_json(&state_dir.join(REGISTRY_FILE), MAX_REGISTRY_BYTES)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn write_registry(state_dir: &Path, registry: &BTreeMap<String, BTreeMap<String, String>>) -> Result<(), String> {
    fs::create_dir_all(state_dir).map_err(|error| format!("Не удалось создать {}: {error}", state_dir.display()))?;
    let content = serde_json::to_vec_pretty(registry).map_err(|error| error.to_string())?;
    let temporary = state_dir.join(format!("{REGISTRY_FILE}.tmp"));
    fs::write(&temporary, content).map_err(|error| format!("Не удалось записать реестр ярлыков: {error}"))?;
    fs::rename(&temporary, state_dir.join(REGISTRY_FILE)).map_err(|error| format!("Не удалось записать реестр ярлыков: {error}"))
}


fn icon_base(environment: &Environment, name: &str) -> PathBuf {
    environment.state_dir.join("icons").join(name)
}

/// возвращает иконку экземпляра или `None`, если её нет или она повреждена
fn prepare_icon(environment: &Environment, instance: &Instance) -> Result<Option<PathBuf>, String> {
    let Some(source) = instance_icon_png(&instance.path) else {
        return Ok(None);
    };
    let Ok(image) = decode_png_rgba(&source) else {
        return Ok(None);
    };
    let directory = environment.state_dir.join("icons");
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать каталог иконок: {error}"))?;
    let base = icon_base(environment, &instance.name);
    let output = match environment.format {
        Format::WindowsLnk => {
            let path = base.with_extension("ico");
            fs::write(&path, build_ico(&image)?).map_err(|error| format!("Не удалось записать иконку: {error}"))?;
            path
        }
        Format::LinuxDesktop => {
            let path = base.with_extension("png");
            fs::write(&path, encode_png(&image.resize(256))?).map_err(|error| format!("Не удалось записать иконку: {error}"))?;
            path
        }
        Format::MacApp => {
            let path = base.with_extension("icns");
            fs::write(&path, build_icns(&image)?).map_err(|error| format!("Не удалось записать иконку: {error}"))?;
            path
        }
    };
    Ok(Some(output))
}

fn instance_icon_png(instance_path: &Path) -> Option<Vec<u8>> {
    let metadata = state::instance_metadata(instance_path);
    let file = metadata.get("icon_file").and_then(Value::as_str)?.trim().to_owned();
    if file.is_empty() || file.contains('/') || file.contains('\\') || !file.to_ascii_lowercase().ends_with(".png") {
        return None;
    }
    let path = instance_path.join(ASSETS_DIR).join(file);
    let meta = fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_ICON_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    bytes.starts_with(b"\x89PNG\r\n\x1a\n").then_some(bytes)
}

#[derive(Debug, Clone)]
pub struct Rgba {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Rgba {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_pixels(width: u32, height: u32, pixels: Vec<u8>) -> Rgba {
        Rgba { width, height, pixels }
    }
    fn resize(&self, size: u32) -> Rgba {
        let scale = (size as f32 / self.width as f32).min(size as f32 / self.height as f32);
        let width = ((self.width as f32 * scale).round() as u32).clamp(1, size);
        let height = ((self.height as f32 * scale).round() as u32).clamp(1, size);
        let offset_x = (size - width) / 2;
        let offset_y = (size - height) / 2;
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let source_x = ((x as f32 + 0.5) / scale - 0.5).clamp(0.0, (self.width - 1) as f32);
                let source_y = ((y as f32 + 0.5) / scale - 0.5).clamp(0.0, (self.height - 1) as f32);
                let (x0, y0) = (source_x.floor() as u32, source_y.floor() as u32);
                let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
                let (fx, fy) = (source_x - x0 as f32, source_y - y0 as f32);
                let target = (((y + offset_y) * size + (x + offset_x)) * 4) as usize;
                for channel in 0..4 {
                    let sample = |px: u32, py: u32| self.pixels[((py * self.width + px) * 4) as usize + channel] as f32;
                    let top = sample(x0, y0) * (1.0 - fx) + sample(x1, y0) * fx;
                    let bottom = sample(x0, y1) * (1.0 - fx) + sample(x1, y1) * fx;
                    pixels[target + channel] = (top * (1.0 - fy) + bottom * fy).round() as u8;
                }
            }
        }
        Rgba { width: size, height: size, pixels }
    }
}

pub fn decode_png_rgba(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("Иконка слишком большая".to_owned());
    }
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).map_err(|error| error.to_string())?;
    let data = &buffer[..frame.buffer_size()];
    let pixels: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("Неподдерживаемый формат PNG".to_owned()),
    };
    Ok(Rgba { width, height, pixels })
}

pub fn encode_png(image: &Rgba) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer.write_image_data(&image.pixels).map_err(|error| error.to_string())?;
    }
    Ok(output)
}

pub fn build_ico(image: &Rgba) -> Result<Vec<u8>, String> {
    let sizes = [256u32, 48, 32, 16];
    let frames: Vec<Vec<u8>> = sizes.iter().map(|&size| encode_png(&image.resize(size))).collect::<Result<_, _>>()?;
    let mut output = Vec::new();
    output.extend_from_slice(&[0, 0, 1, 0]);
    output.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, frame) in sizes.iter().zip(&frames) {
        let dimension = if *size >= 256 { 0 } else { *size as u8 };
        output.extend_from_slice(&[dimension, dimension, 0, 0]);
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&32u16.to_le_bytes());
        output.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        output.extend_from_slice(&offset.to_le_bytes());
        offset += frame.len() as u32;
    }
    for frame in frames {
        output.extend_from_slice(&frame);
    }
    Ok(output)
}

pub fn build_icns(image: &Rgba) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for (kind, size) in [(b"ic07", 128u32), (b"ic08", 256), (b"ic09", 512)] {
        let frame = encode_png(&image.resize(size))?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&((frame.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(&frame);
    }
    let mut output = Vec::with_capacity(body.len() + 8);
    output.extend_from_slice(b"icns");
    output.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    output.extend_from_slice(&body);
    Ok(output)
}


/// экранирует аргумент `Exec` по правилам Desktop Entry
pub fn desktop_exec_argument(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

fn desktop_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', " ").replace('\r', " ").replace('\t', " ")
}

pub fn desktop_entry_content(environment: &Environment, instance: &Instance, icon: Option<&Path>) -> String {
    let exec = [
        desktop_exec_argument(&environment.launcher.to_string_lossy()),
        LAUNCH_ARGUMENT.to_owned(),
        desktop_exec_argument(&instance.name),
    ]
    .join(" ");
    let icon_line = icon
        .map(|path| desktop_value(&path.to_string_lossy()))
        .unwrap_or_else(|| "nivora-launcher".to_owned());
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nComment={}\nExec={exec}\nIcon={icon_line}\nTerminal=false\nCategories=Game;\nStartupNotify=true\nX-Nivora-Instance={}\n",
        desktop_value(&instance.display_name),
        desktop_value(&format!("Запуск экземпляра «{}» в Nivora Launcher", instance.display_name)),
        instance.name,
    )
}

fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("nivora-tmp");
    fs::write(&temporary, content).map_err(|error| format!("Не удалось записать ярлык {}: {error}", path.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("Не удалось записать ярлык {}: {error}", path.display()))
}

fn write_desktop_entry(environment: &Environment, instance: &Instance, target: &Path, icon: Option<&Path>) -> Result<(), String> {
    write_atomic(target, desktop_entry_content(environment, instance, icon).as_bytes())?;
    super::platform::set_mode(target, 0o755);
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn macos_info_plist(instance: &Instance) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>CFBundleExecutable</key><string>launch</string>\n  <key>CFBundleIdentifier</key><string>{MACOS_BUNDLE_PREFIX}{}</string>\n  <key>CFBundleName</key><string>{}</string>\n  <key>CFBundleDisplayName</key><string>{}</string>\n  <key>CFBundleIconFile</key><string>icon.icns</string>\n  <key>CFBundlePackageType</key><string>APPL</string>\n  <key>CFBundleShortVersionString</key><string>1.0</string>\n  <key>LSMinimumSystemVersion</key><string>10.15</string>\n  <key>NSHighResolutionCapable</key><true/>\n</dict>\n</plist>\n",
        instance.name,
        xml_escape(&instance.display_name),
        xml_escape(&instance.display_name),
    )
}

pub fn macos_launch_script(environment: &Environment, instance: &Instance) -> String {
    format!(
        "#!/bin/sh\n# Ярлык Nivora Launcher: запускает экземпляр «{}».\nexec {} {LAUNCH_ARGUMENT} {}\n",
        instance.name,
        shell_single_quote(&environment.launcher.to_string_lossy()),
        shell_single_quote(&instance.name),
    )
}

fn write_macos_bundle(environment: &Environment, instance: &Instance, target: &Path, icon: Option<&Path>) -> Result<(), String> {
    // собираем пакет рядом и подменяем атомарно, чтобы Finder не увидел его частично
    let staging = target.with_extension("app-nivora-tmp");
    let _ = fs::remove_dir_all(&staging);
    let contents = staging.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    for directory in [&macos, &resources] {
        fs::create_dir_all(directory).map_err(|error| format!("Не удалось создать {}: {error}", directory.display()))?;
    }
    fs::write(contents.join("Info.plist"), macos_info_plist(instance)).map_err(|error| error.to_string())?;
    fs::write(contents.join("PkgInfo"), b"APPL????").map_err(|error| error.to_string())?;
    let script = macos.join("launch");
    fs::write(&script, macos_launch_script(environment, instance)).map_err(|error| error.to_string())?;
    super::platform::set_mode(&script, 0o755);
    if let Some(icon) = icon {
        fs::copy(icon, resources.join("icon.icns")).map_err(|error| format!("Не удалось скопировать иконку: {error}"))?;
    }
    if target.exists() {
        fs::remove_dir_all(target).map_err(|error| format!("Не удалось обновить ярлык {}: {error}", target.display()))?;
    }
    fs::rename(&staging, target).map_err(|error| format!("Не удалось записать ярлык {}: {error}", target.display()))
}

/// экранирует аргумент для `CommandLineToArgvW`
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_arguments(name: &str) -> String {
    format!("{LAUNCH_ARGUMENT} \"{name}\"")
}

#[cfg(not(windows))]
fn write_windows_link(_environment: &Environment, _instance: &Instance, _target: &Path, _icon: Option<&Path>) -> Result<(), String> {
    Err("Ярлыки .lnk создаются только в Windows".to_owned())
}

#[cfg(windows)]
fn write_windows_link(environment: &Environment, instance: &Instance, target: &Path, icon: Option<&Path>) -> Result<(), String> {
    let working_dir = environment.launcher.parent().map(Path::to_path_buf).unwrap_or_default();
    let (icon_path, icon_index) = match icon {
        Some(path) => (path.to_path_buf(), 0),
        None => (environment.launcher.clone(), 0),
    };
    windows_link::write(&windows_link::Link {
        target: environment.launcher.clone(),
        arguments: windows_arguments(&instance.name),
        working_dir,
        icon: icon_path,
        icon_index,
        description: format!("Запуск экземпляра «{}» в Nivora Launcher", instance.display_name),
        output: target.to_path_buf(),
    })
}

/// создаёт `.lnk` через `IShellLinkW` в потоке с инициализированным COM
#[cfg(windows)]
pub mod windows_link {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };

    pub struct Link {
        pub target: PathBuf,
        pub arguments: String,
        pub working_dir: PathBuf,
        pub icon: PathBuf,
        pub icon_index: i32,
        pub description: String,
        pub output: PathBuf,
    }

    const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    const RPC_E_CHANGED_MODE: HRESULT = 0x8001_0106_u32 as i32;

    type Raw = *mut c_void;

    #[repr(C)]
    struct IUnknownVtbl {
        query_interface: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> HRESULT,
        add_ref: unsafe extern "system" fn(Raw) -> u32,
        release: unsafe extern "system" fn(Raw) -> u32,
    }

    #[repr(C)]
    struct IShellLinkWVtbl {
        unknown: IUnknownVtbl,
        get_path: usize,
        get_id_list: usize,
        set_id_list: usize,
        get_description: usize,
        set_description: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
        get_working_directory: usize,
        set_working_directory: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
        get_arguments: usize,
        set_arguments: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
        get_hotkey: usize,
        set_hotkey: usize,
        get_show_cmd: usize,
        set_show_cmd: usize,
        get_icon_location: usize,
        set_icon_location: unsafe extern "system" fn(Raw, *const u16, i32) -> HRESULT,
        set_relative_path: usize,
        resolve: usize,
        set_path: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
    }

    #[repr(C)]
    struct IPersistFileVtbl {
        unknown: IUnknownVtbl,
        get_class_id: usize,
        is_dirty: usize,
        load: usize,
        save: unsafe extern "system" fn(Raw, *const u16, i32) -> HRESULT,
        save_completed: usize,
        get_cur_file: usize,
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide_str(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn check(result: HRESULT, step: &str) -> Result<(), String> {
        if result < 0 {
            Err(format!("Не удалось создать ярлык Windows ({step}: 0x{:08X})", result as u32))
        } else {
            Ok(())
        }
    }

    struct ComGuard(bool);

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    unsafe fn release(object: Raw) {
        if !object.is_null() {
            let vtbl = *(object as *const *const IUnknownVtbl);
            ((*vtbl).release)(object);
        }
    }

    pub fn write(link: &Link) -> Result<(), String> {
        let initialized = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if initialized < 0 && initialized != RPC_E_CHANGED_MODE {
            return check(initialized, "CoInitializeEx");
        }
        let _guard = ComGuard(initialized >= 0);
        let mut shell_link: Raw = std::ptr::null_mut();
        check(
            unsafe { CoCreateInstance(&CLSID_SHELL_LINK, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELL_LINK_W, &mut shell_link) },
            "CoCreateInstance",
        )?;
        let result = (|| unsafe {
            let vtbl = &**(shell_link as *const *const IShellLinkWVtbl);
            check((vtbl.set_path)(shell_link, wide(link.target.as_os_str()).as_ptr()), "SetPath")?;
            check((vtbl.set_arguments)(shell_link, wide_str(&link.arguments).as_ptr()), "SetArguments")?;
            check((vtbl.set_working_directory)(shell_link, wide(link.working_dir.as_os_str()).as_ptr()), "SetWorkingDirectory")?;
            check((vtbl.set_description)(shell_link, wide_str(&link.description).as_ptr()), "SetDescription")?;
            check((vtbl.set_icon_location)(shell_link, wide(link.icon.as_os_str()).as_ptr(), link.icon_index), "SetIconLocation")?;
            let mut persist: Raw = std::ptr::null_mut();
            check((vtbl.unknown.query_interface)(shell_link, &IID_IPERSIST_FILE, &mut persist), "QueryInterface")?;
            let persist_vtbl = &**(persist as *const *const IPersistFileVtbl);
            let saved = (persist_vtbl.save)(persist, wide(link.output.as_os_str()).as_ptr(), 1);
            release(persist);
            check(saved, "IPersistFile::Save")
        })();
        unsafe { release(shell_link) };
        result
    }

    #[allow(dead_code)]
    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }
}


impl Environment {
    pub fn detect() -> Result<Self, String> {
        let roots = state::launcher_roots();
        let format = Format::current();
        let launcher = launcher_executable()?;
        let desktop_dir = env_dir("NIVORA_SHORTCUT_DESKTOP_DIR").unwrap_or_else(|| desktop_directory(format));
        let menu_dir = env_dir("NIVORA_SHORTCUT_MENU_DIR").unwrap_or_else(|| menu_directory(format, &roots.data_home));
        Ok(Self {
            format,
            desktop_dir,
            menu_dir,
            state_dir: roots.data_home.join("shortcuts"),
            fallback_icon: fallback_icon(format, &launcher),
            launcher,
        })
    }
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// возвращает файл запуска; для AppImage это сам образ, а не точка монтирования
fn launcher_executable() -> Result<PathBuf, String> {
    if let Some(appimage) = env_dir("APPIMAGE") {
        return Ok(appimage);
    }
    std::env::current_exe().map_err(|error| format!("Не удалось определить путь к лаунчеру: {error}"))
}

fn fallback_icon(format: Format, launcher: &Path) -> Option<PathBuf> {
    match format {
        Format::WindowsLnk => None,
        Format::MacApp => {
            let resources = launcher.parent()?.parent()?.join("Resources");
            ["icon.icns", "Nivora Launcher.icns"]
                .iter()
                .map(|name| resources.join(name))
                .find(|path| path.is_file())
        }
        Format::LinuxDesktop => None,
    }
}

fn desktop_directory(format: Format) -> PathBuf {
    #[cfg(windows)]
    if let Some(path) = windows_known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop) {
        return path;
    }
    if format == Format::LinuxDesktop {
        if let Some(path) = xdg_user_dir("XDG_DESKTOP_DIR") {
            return path;
        }
    }
    home().join("Desktop")
}

fn menu_directory(format: Format, data_home: &Path) -> PathBuf {
    match format {
        Format::WindowsLnk => {
            #[cfg(windows)]
            if let Some(path) = windows_known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Programs) {
                return path.join("Nivora Launcher");
            }
            home().join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Nivora Launcher")
        }
        Format::MacApp => home().join("Applications").join("Nivora Launcher"),
        Format::LinuxDesktop => std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| data_home.parent().map(Path::to_path_buf).unwrap_or_else(|| home().join(".local/share")))
            .join("applications"),
    }
}

/// читает локализованный каталог рабочего стола из XDG-настроек
fn xdg_user_dir(key: &str) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    let content = fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    parse_user_dirs(&content, key, &home())
}

pub fn parse_user_dirs(content: &str, key: &str, home: &Path) -> Option<PathBuf> {
    for line in content.lines() {
        let line = line.trim();
        let Some(value) = line.strip_prefix(key).and_then(|rest| rest.trim_start().strip_prefix('=')) else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let path = if let Some(rest) = value.strip_prefix("$HOME") {
            home.join(rest.trim_start_matches('/'))
        } else {
            PathBuf::from(value)
        };
        if path.is_absolute() && path != home {
            return Some(path);
        }
    }
    None
}

#[cfg(windows)]
fn windows_known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    let mut raw: *mut u16 = std::ptr::null_mut();
    let result = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
    if result < 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw as *const _) };
        }
        return None;
    }
    let length = (0..).take_while(|&index| unsafe { *raw.add(index) } != 0).count();
    let value = unsafe { std::slice::from_raw_parts(raw, length) };
    let path = PathBuf::from(std::ffi::OsString::from_wide(value));
    unsafe { CoTaskMemFree(raw as *const _) };
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("nivora-shortcuts-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn environment(root: &Path, format: Format) -> Environment {
        Environment {
            format,
            desktop_dir: root.join("Рабочий стол"),
            menu_dir: root.join("menu dir"),
            state_dir: root.join("state"),
            launcher: root.join("Program Files/Nivora Launcher/nivora-launcher"),
            fallback_icon: None,
        }
    }

    fn instance(root: &Path, name: &str, display: &str, icon: Option<&[u8]>) -> Instance {
        let path = root.join("game/instances").join(name);
        fs::create_dir_all(path.join(ASSETS_DIR)).unwrap();
        let mut metadata = json!({ "display_name": display });
        if let Some(bytes) = icon {
            fs::write(path.join(ASSETS_DIR).join("icon.png"), bytes).unwrap();
            metadata["icon_file"] = json!("icon.png");
        }
        fs::write(path.join(".nivora-launcher-instance.json"), metadata.to_string()).unwrap();
        Instance { name: name.into(), display_name: display.into(), path }
    }

    fn sample_png(width: u32, height: u32) -> Vec<u8> {
        let pixels: Vec<u8> = (0..width * height).flat_map(|index| [(index % 255) as u8, 80, 160, 255]).collect();
        encode_png(&Rgba { width, height, pixels }).unwrap()
    }

    #[test]
    fn linux_desktop_entry_handles_spaces_and_non_ascii_and_is_idempotent() {
        let sandbox = Sandbox::new("linux");
        let environment = environment(&sandbox.0, Format::LinuxDesktop);
        let item = instance(&sandbox.0, "survival-1", "Выживание: «мир» 1.21", Some(&sample_png(64, 32)));
        let first = create(&environment, &item, Location::Desktop).unwrap();
        let path = PathBuf::from(first["path"].as_str().unwrap());
        assert_eq!(file_name(&path), "Выживание «мир» 1.21.desktop");
        assert_eq!(first["icon"], "instance");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Exec=\"") && content.contains("Program Files/Nivora Launcher/nivora-launcher\" --launch-instance \"survival-1\""));
        assert!(content.contains("X-Nivora-Instance=survival-1"));
        let icon_line = content.lines().find(|line| line.starts_with("Icon=")).unwrap();
        let icon = PathBuf::from(&icon_line[5..]);
        assert_eq!(decode_png_rgba(&fs::read(&icon).unwrap()).unwrap().width, 256);
        let second = create(&environment, &item, Location::Desktop).unwrap();
        assert_eq!(second["path"], first["path"]);
        assert_eq!(fs::read_dir(&environment.desktop_dir).unwrap().count(), 1);
        let state = status(&environment, &item).unwrap();
        assert_eq!(state["desktop"]["exists"], true);
        assert_eq!(state["menu"]["exists"], false);
        assert_eq!(remove(&environment, "survival-1", None).unwrap().len(), 1);
        assert!(!path.exists());
        assert_eq!(status(&environment, &item).unwrap()["desktop"]["exists"], false);
    }

    #[test]
    fn rename_replaces_the_old_shortcut_and_foreign_files_are_protected() {
        let sandbox = Sandbox::new("rename");
        let environment = environment(&sandbox.0, Format::LinuxDesktop);
        let mut item = instance(&sandbox.0, "pack", "Old Name", None);
        let old = PathBuf::from(create(&environment, &item, Location::Menu).unwrap()["path"].as_str().unwrap().to_owned());
        item.display_name = "New Name".into();
        let new = PathBuf::from(create(&environment, &item, Location::Menu).unwrap()["path"].as_str().unwrap().to_owned());
        assert!(!old.exists() && new.exists());
        let foreign = environment.desktop_dir.join("New Name.desktop");
        fs::create_dir_all(&environment.desktop_dir).unwrap();
        fs::write(&foreign, "[Desktop Entry]\nName=Other\n").unwrap();
        let error = create(&environment, &item, Location::Desktop).unwrap_err();
        assert!(error.contains("создан"), "{error}");
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "[Desktop Entry]\nName=Other\n");
        remove(&environment, "pack", None).unwrap();
        assert!(foreign.exists());
    }

    #[test]
    fn same_display_name_for_two_instances_gets_distinct_files() {
        let sandbox = Sandbox::new("collide");
        let environment = environment(&sandbox.0, Format::LinuxDesktop);
        let first = instance(&sandbox.0, "a", "Мир", None);
        let second = instance(&sandbox.0, "b", "Мир", None);
        let one = create(&environment, &first, Location::Desktop).unwrap();
        let two = create(&environment, &second, Location::Desktop).unwrap();
        assert_ne!(one["path"], two["path"]);
        assert!(two["path"].as_str().unwrap().ends_with("Мир (b).desktop"));
        assert_eq!(one["icon"], "fallback");
    }

    #[test]
    fn macos_bundle_has_plist_script_and_icns() {
        let sandbox = Sandbox::new("macos");
        let environment = environment(&sandbox.0, Format::MacApp);
        let item = instance(&sandbox.0, "creative", "Креатив & 'тест'", Some(&sample_png(32, 32)));
        let path = PathBuf::from(create(&environment, &item, Location::Desktop).unwrap()["path"].as_str().unwrap().to_owned());
        assert!(path.ends_with("Креатив & 'тест'.app"));
        let plist = fs::read_to_string(path.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>com.nivora.launcher.shortcut.creative</string>"));
        assert!(plist.contains("Креатив &amp; &apos;тест&apos;"));
        let script = fs::read_to_string(path.join("Contents/MacOS/launch")).unwrap();
        assert!(script.contains("exec '") && script.ends_with("--launch-instance 'creative'\n"));
        let icns = fs::read(path.join("Contents/Resources/icon.icns")).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        create(&environment, &item, Location::Desktop).unwrap();
        assert_eq!(remove(&environment, "creative", Some(Location::Desktop)).unwrap().len(), 1);
        assert!(!path.exists());
    }

    #[test]
    fn ico_layout_points_at_valid_png_frames() {
        let image = decode_png_rgba(&sample_png(100, 60)).unwrap();
        let ico = build_ico(&image).unwrap();
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        assert_eq!(count, 4);
        for index in 0..count {
            let entry = &ico[6 + index * 16..6 + (index + 1) * 16];
            let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            let frame = decode_png_rgba(&ico[offset..offset + size]).unwrap();
            let expected = if entry[0] == 0 { 256 } else { entry[0] as u32 };
            assert_eq!((frame.width, frame.height), (expected, expected));
        }
    }

    #[test]
    fn launch_argument_accepts_only_valid_instance_names() {
        let args = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        assert_eq!(launch_argument(args(&["app", "--launch-instance", "my-pack_1.2"])), Some("my-pack_1.2".into()));
        assert_eq!(launch_argument(args(&["app", "--launch-instance=pack"])), Some("pack".into()));
        assert_eq!(launch_argument(args(&["app", "--launch-instance", "../etc"])), None);
        assert_eq!(launch_argument(args(&["app", "--launch-instance", "a b"])), None);
        assert_eq!(launch_argument(args(&["app", "--launch-instance", "x; rm -rf /"])), None);
        assert_eq!(launch_argument(args(&["app"])), None);
        assert!(set_pending_launch("pack"));
        assert_eq!(shortcut_take_launch_request(&json!({})).unwrap()["name"], "pack");
        assert_eq!(shortcut_take_launch_request(&json!({})).unwrap()["name"], Value::Null);
        assert!(!set_pending_launch("bad name"));
    }

    #[test]
    fn exec_and_file_name_escaping() {
        assert_eq!(desktop_exec_argument("/opt/My App/bin"), "\"/opt/My App/bin\"");
        assert_eq!(desktop_exec_argument("a\"b$c`d"), "\"a\\\\\"b\\\\$c\\\\`d\"");
        assert_eq!(desktop_exec_argument("100%"), "\"100%%\"");
        assert_eq!(sanitize_file_stem("  CON ", "x"), "CON_");
        assert_eq!(sanitize_file_stem("a/b\\c:d*e?", "x"), "a b c d e");
        assert_eq!(sanitize_file_stem("...", "fallback"), "fallback");
        assert_eq!(windows_arguments("pack"), "--launch-instance \"pack\"");
        let home = Path::new("/home/игрок");
        assert_eq!(
            parse_user_dirs("XDG_DESKTOP_DIR=\"$HOME/Рабочий стол\"\n", "XDG_DESKTOP_DIR", home),
            Some(PathBuf::from("/home/игрок/Рабочий стол"))
        );
        assert_eq!(parse_user_dirs("XDG_DESKTOP_DIR=\"$HOME/\"\n", "XDG_DESKTOP_DIR", home), None);
    }

    #[test]
    fn parallel_create_and_remove_are_consistent() {
        let sandbox = Sandbox::new("parallel");
        let environment = environment(&sandbox.0, Format::LinuxDesktop);
        let items: Vec<Instance> = (0..8).map(|index| instance(&sandbox.0, &format!("i{index}"), &format!("Экземпляр {index}"), None)).collect();
        std::thread::scope(|scope| {
            for item in &items {
                let environment = &environment;
                scope.spawn(move || {
                    for _ in 0..20 {
                        create(environment, item, Location::Desktop).unwrap();
                        create(environment, item, Location::Menu).unwrap();
                    }
                });
            }
        });
        assert_eq!(fs::read_dir(&environment.desktop_dir).unwrap().count(), 8);
        assert_eq!(read_registry(&environment.state_dir).len(), 8);
        for item in &items {
            assert_eq!(remove(&environment, &item.name, None).unwrap().len(), 2);
        }
        assert_eq!(fs::read_dir(&environment.desktop_dir).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn windows_link_is_written_by_the_shell() {
        let sandbox = Sandbox::new("windows");
        let environment = Environment {
            launcher: std::env::current_exe().unwrap(),
            ..environment(&sandbox.0, Format::WindowsLnk)
        };
        let item = instance(&sandbox.0, "win-pack", "Мир с пробелами", Some(&sample_png(48, 48)));
        let result = create(&environment, &item, Location::Desktop).unwrap();
        let path = PathBuf::from(result["path"].as_str().unwrap());
        assert!(path.ends_with("Мир с пробелами.lnk"));
        assert!(is_ours(Format::WindowsLnk, &path, "win-pack"));
        create(&environment, &item, Location::Desktop).unwrap();
        assert_eq!(remove(&environment, "win-pack", None).unwrap().len(), 1);
        assert!(!path.exists());
    }
}
