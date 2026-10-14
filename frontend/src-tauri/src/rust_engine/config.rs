// Хранилище настроек: settings.json в XDG-корнях, значения по умолчанию,
// нормализация при загрузке и запись отдельных ключей с валидацией.
use regex::Regex;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

use super::constants::*;
use super::net::atomic_json_write;
use super::state::{home_dir, launcher_roots, LauncherRoots};

pub const DEFAULT_KEYBOARD_SHORTCUTS: &[(&str, &str)] = &[
    ("open_settings", "Ctrl+,"),
    ("refresh", "Ctrl+R"),
    ("new_instance", "Ctrl+N"),
    ("focus_search", "Ctrl+K"),
    ("toggle_activity", "Ctrl+J"),
];

pub const DEFAULT_COLOR_TOKENS: &[(&str, &str)] = &[
    ("accent", "#8b653f"),
    ("accent_strong", "#c5a274"),
    ("background", "#191a17"),
    ("panel", "#20221e"),
    ("panel_raised", "#282a25"),
    ("panel_soft", "#30322c"),
    ("text", "#e9e8de"),
    ("muted", "#a2a395"),
    ("quiet", "#c7c7bb"),
    ("border", "#77796b"),
    ("danger", "#d58a7d"),
    ("success", "#9bac78"),
    ("warning", "#d2ad72"),
    ("banner_shade", "#191a17"),
];

// Скругления углов основных групп элементов интерфейса, в пикселях.
pub const DEFAULT_RADIUS_TOKENS: &[(&str, i64)] = &[("card", 7), ("button", 8), ("control", 10)];

const PAGES: &[&str] = &[
    "instances",
    "instance-settings",
    "accounts",
    "mods",
    "modpacks",
    "plugins",
    "lan-rooms",
    "themes",
    "settings",
];

pub fn default_launcher_profile() -> Value {
    json!({
        "id": "default",
        "name": "По умолчанию",
        "memory": "auto",
        "java_path": "",
        "graphics_mode": "auto",
        "account_id": "auto",
        "window_width": 1280,
        "window_height": 720,
        "fullscreen": false,
        "quick_play_world": "",
        "quick_play_server": "",
        "jvm_arguments": [],
        "game_arguments": [],
        "jvm_preset": "none",
    })
}

fn default_animation_settings() -> Value {
    let pages = |style: &str, duration: i64| {
        let mut map = Map::new();
        for page in PAGES {
            map.insert(page.to_string(), json!(style));
            let _ = duration;
        }
        map
    };
    let mut durations = Map::new();
    for page in PAGES {
        durations.insert(page.to_string(), json!(140));
    }
    let mut styles = pages("fade", 140);
    styles.retain(|_, _| true);
    json!({
        "page_style": "fade",
        "page_duration_ms": 140,
        "surface_duration_ms": 140,
        "surface_stagger_ms": 0,
        "page_styles": styles,
        "page_durations_ms": durations,
    })
}

fn default_overlay_settings() -> Value {
    json!({
        "enabled": true, "hotkey_overlay": "Shift+Tab", "hotkey_recording": "F7",
        "show_mods": true, "show_metrics": true, "recording_fps": 60, "recording_quality": 23,
        "recording_encoder": "auto", "recording_container": "mp4", "recording_audio": true,
        "position": "top-left", "scale": 100, "opacity": 96, "compact": false,
        "show_clock": true, "show_system_metrics": true, "metrics_cpu": true, "metrics_ram": true,
        "metrics_gpu": true, "metrics_disk": true, "metrics_temperatures": true,
        "system_metrics_display": "overlay", "show_chat": true,
    })
}

fn default_backup_schedule() -> Value {
    json!({ "enabled": false, "interval_hours": 24, "keep": 5, "destination": "local" })
}

pub fn default_resource_catalogs() -> Value {
    json!([
        { "id": "modrinth", "name": "Modrinth", "kind": "modrinth", "api_url": MODRINTH_API_URL },
        { "id": "ftb", "name": "Feed The Beast", "kind": "ftb", "api_url": FTB_MODPACK_API_URL },
        { "id": "technic", "name": "Technic", "kind": "technic", "api_url": TECHNIC_API_URL },
    ])
}

pub fn default_settings_object() -> Value {
    let mut page_themes = Map::new();
    let mut page_layouts = Map::new();
    for page in PAGES {
        page_themes.insert(page.to_string(), json!("inherit"));
        page_layouts.insert(page.to_string(), json!("balanced"));
    }
    let shortcuts: Map<String, Value> = DEFAULT_KEYBOARD_SHORTCUTS
        .iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect();
    let colors: Map<String, Value> = DEFAULT_COLOR_TOKENS
        .iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect();
    let radius: Map<String, Value> = DEFAULT_RADIUS_TOKENS
        .iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect();
    json!({
        "game_dir": "",
        "client_id": DEFAULT_MSA_CLIENT_ID,
        "java_path": "",
        "memory": "auto",
        "max_downloads": 8,
        "startup_view": "instances",
        "last_view": "instances",
        "remember_last_view": true,
        "default_version_filter": "release",
        "default_loader": "fabric",
        "default_account_id": "auto",
        "open_last_instance": true,
        "launch_after_install": false,
        "auto_refresh_metadata": true,
        "metadata_refresh_hours": 6,
        "download_timeout_seconds": 30,
        "download_retries": 3,
        "confirm_destructive_actions": true,
        "show_activity_panel": true,
        "show_toasts": true,
        "minimize_on_launch": false,
        "close_to_tray": false,
        "language": null,
        "font_family": "system",
        "keep_logs_days": 14,
        "telemetry": false,
        "telemetry_endpoint": "",
        "keyboard_shortcuts": shortcuts,
        "graphics_mode": "auto",
        "launcher_profiles": [default_launcher_profile()],
        "active_launcher_profile_id": "default",
        "http_proxy": "",
        "offline_debug_limit_seconds": 0,
        "launcher_version": APP_VERSION,
        "theme": "dark",
        "active_theme_id": "midnight-signal",
        "wallpaper_path": "",
        "wallpaper_blur": 12,
        "wallpaper_opacity": 34,
        "accent_color": "#8b653f",
        "sidebar_position": "left",
        "sidebar_mode": "adaptive",
        "sidebar_width_mode": "content",
        "sidebar_width": 228,
        "sidebar_floating": false,
        "sidebar_visibility": "always",
        "animations": "full",
        "animation_settings": default_animation_settings(),
        "overlay_settings": default_overlay_settings(),
        "backup_schedule": default_backup_schedule(),
        "density": "comfortable",
        "ui_scale": 100,
        "radius_tokens": radius,
        "page_themes": page_themes,
        "page_layouts": page_layouts,
        "widget_visibility": { "instances.workspace": true, "accounts.status": true, "mods.context": true },
        "color_tokens": colors,
        "instance_groups": [],
        "pinned_instance_groups": [],
        "login_source": "microsoft",
        "login_api_url": MSA_LOGIN_API_URL,
        "catalog_source": "modrinth",
        "catalog_api_url": MODRINTH_API_URL,
        "data_source": "mojang",
        "data_api_url": VERSION_MANIFEST_URL,
        "resource_catalogs": default_resource_catalogs(),
        "active_catalog_id": "modrinth",
        "plugin_catalogs": super::plugin_catalogs::default_plugin_catalogs(),
        "active_plugin_catalog_id": "modrinth-plugins",
        "developer_mode": false,
        "cloud_url": "",
        "cloud_username": "",
        "cloud_remote_path": "nivora-launcher",
        "cloud_include_accounts": false,
        "cloud_last_sync": 0.0,
    })
}

pub fn system_memory_kib() -> i64 {
    #[cfg(unix)]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    if let Some(value) = rest.split_whitespace().next().and_then(|part| part.parse::<i64>().ok()) {
                        return value;
                    }
                }
            }
        }
    }
    0
}

pub fn recommended_memory_kib() -> i64 {
    let total = system_memory_kib();
    if total <= 0 {
        return 2 * 1024 * 1024;
    }
    (total / 2 / 1024).max(1024 * 1024) * 1024
}

pub fn parse_memory_kib(value: &str) -> Result<i64, String> {
    parse_memory_kib_or(value, None)
}

pub fn parse_memory_kib_or(value: &str, default_kib: Option<i64>) -> Result<i64, String> {
    let raw = value.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("auto") || raw.eq_ignore_ascii_case("automatic") {
        return Ok(default_kib.unwrap_or_else(recommended_memory_kib));
    }
    let regex = Regex::new(r"(?i)^(\d+(?:\.\d+)?)\s*([kmgt]i?b?|b)?\z").unwrap();
    let Some(captures) = regex.captures(raw) else {
        return Err("Память JVM должна быть числом с единицей KiB, MiB или GiB".to_owned());
    };
    let number: f64 = captures
        .get(1)
        .and_then(|value| value.as_str().parse().ok())
        .ok_or("Память JVM должна быть числом с единицей KiB, MiB или GiB")?;
    if !number.is_finite() || number <= 0.0 {
        return Err("Память JVM должна быть больше нуля".to_owned());
    }
    let unit = captures.get(2).map(|value| value.as_str().to_lowercase()).unwrap_or_else(|| "m".to_owned());
    let factor: f64 = match unit.as_str() {
        "b" => 1.0 / 1024.0,
        "k" | "kb" | "kib" => 1.0,
        "m" | "mb" | "mib" => 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    let kib = (number * factor) as i64;
    if kib < 1 {
        return Err("Значение памяти JVM слишком маленькое".to_owned());
    }
    Ok(kib)
}

pub fn format_memory_kib(value: i64) -> String {
    let kib = value.max(1);
    if kib % (1024 * 1024) == 0 {
        return format!("{}GiB", kib / (1024 * 1024));
    }
    if kib % 1024 == 0 {
        return format!("{}MiB", kib / 1024);
    }
    format!("{kib}KiB")
}

pub fn java_memory_value(value: &str) -> Result<String, String> {
    let kib = parse_memory_kib(value)?;
    if kib % (1024 * 1024) == 0 {
        return Ok(format!("{}G", kib / (1024 * 1024)));
    }
    if kib % 1024 == 0 {
        return Ok(format!("{}M", kib / 1024));
    }
    Ok(format!("{kib}K"))
}

pub fn coerce_bool(value: &Value, fallback: bool) -> bool {
    match value {
        Value::Bool(inner) => *inner,
        Value::String(text) => match text.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => fallback,
        },
        _ => fallback,
    }
}

pub fn bounded_int(value: &Value, minimum: i64, maximum: i64, fallback: i64) -> i64 {
    let number = match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    number.map(|value| value.clamp(minimum, maximum)).unwrap_or(fallback)
}

pub struct Paths {
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub cache_home: PathBuf,
    pub settings_file: PathBuf,
    pub accounts_file: PathBuf,
    pub instance_security_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub instances_dir: PathBuf,
    pub runtimes_dir: PathBuf,
    pub download_cache: PathBuf,
}

impl Paths {
    pub fn from_roots(roots: &LauncherRoots) -> Self {
        Self {
            config_home: roots.config_home.clone(),
            data_home: roots.data_home.clone(),
            cache_home: roots.cache_home.clone(),
            settings_file: roots.config_home.join("settings.json"),
            accounts_file: roots.config_home.join("accounts.json"),
            instance_security_dir: roots.config_home.join("instance-security"),
            logs_dir: roots.data_home.join("logs"),
            instances_dir: roots.data_home.join("instances"),
            runtimes_dir: roots.data_home.join("runtimes"),
            download_cache: roots.cache_home.join("downloads"),
        }
    }

    pub fn discover() -> Self {
        // Корни одни на весь движок — state::launcher_roots: Windows берёт
        // %APPDATA%/%LOCALAPPDATA% как Python Paths, Unix — XDG. Раньше здесь
        // была своя копия логики, корректная лишь случайно (join с абсолютным
        // путём заменял базу «/» из несуществующего на Windows HOME).
        Self::from_roots(&launcher_roots())
    }

    pub fn ensure(&self) -> Result<(), String> {
        for path in [
            &self.config_home,
            &self.instance_security_dir,
            &self.data_home,
            &self.cache_home,
            &self.logs_dir,
            &self.instances_dir,
            &self.runtimes_dir,
            &self.download_cache,
        ] {
            std::fs::create_dir_all(path).map_err(|error| format!("Не удалось создать каталог {}: {error}", path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
            }
        }
        Ok(())
    }
}

pub struct ConfigStore {
    pub paths: Paths,
    settings: Value,
}

impl ConfigStore {
    pub fn load() -> Result<Self, String> {
        let paths = Paths::discover();
        paths.ensure()?;
        let mut store = Self { settings: default_settings_object(), paths };
        store.settings = store.load_and_normalize()?;
        Ok(store)
    }

    fn load_and_normalize(&self) -> Result<Value, String> {
        let defaults = default_settings_object();
        if !self.paths.settings_file.exists() {
            let mut settings = defaults.clone();
            settings["game_dir"] = json!(self.paths.data_home.join("game").to_string_lossy());
            settings["memory"] = json!(format_memory_kib(recommended_memory_kib()));
            settings["client_id"] = json!(configured_client_id());
            self.save(&settings)?;
            return Ok(settings);
        }
        let meta = std::fs::symlink_metadata(&self.paths.settings_file)
            .map_err(|error| format!("Не удалось прочитать настройки: {error}"))?;
        if meta.is_symlink() || meta.len() > 1024 * 1024 {
            return Err("Файл настроек слишком большой или является ссылкой".to_owned());
        }
        let raw_text = std::fs::read_to_string(&self.paths.settings_file)
            .map_err(|error| format!("Не удалось прочитать настройки: {error}"))?;
        let parsed: Value = serde_json::from_str(&raw_text).map_err(|error| format!("Не удалось прочитать настройки: {error}"))?;
        let Value::Object(map) = parsed else {
            return Err("Файл настроек имеет неожиданный формат".to_owned());
        };
        // Берём известные поля из файла поверх умолчаний.
        let mut settings = defaults;
        for (key, value) in map {
            if settings.get(&key).is_some() {
                settings[key.clone()] = value;
            }
        }
        if settings["game_dir"].as_str().unwrap_or("").is_empty() {
            settings["game_dir"] = json!(self.paths.data_home.join("game").to_string_lossy());
        }
        let mut changed = false;
        let game_dir = settings["game_dir"].as_str().unwrap_or("").to_owned();
        let normalized_game_dir = self.validated_game_dir(&game_dir)?.to_string_lossy().to_string();
        if game_dir != normalized_game_dir {
            settings["game_dir"] = json!(normalized_game_dir);
            changed = true;
        }
        let raw_cloud_url = settings["cloud_url"].as_str().unwrap_or("").trim().trim_end_matches('/').to_owned();
        let valid_cloud_url = raw_cloud_url.is_empty()
            || url::Url::parse(&raw_cloud_url)
                .map(|url| url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty() && url.password().is_none())
                .unwrap_or(false);
        let normalized_cloud_url = if valid_cloud_url { raw_cloud_url } else { String::new() };
        if settings["cloud_url"].as_str() != Some(normalized_cloud_url.as_str()) {
            settings["cloud_url"] = json!(normalized_cloud_url);
            changed = true;
        }
        let cloud_username = settings["cloud_username"].as_str().unwrap_or("").trim().chars().take(320).collect::<String>();
        if settings["cloud_username"].as_str() != Some(cloud_username.as_str()) {
            settings["cloud_username"] = json!(cloud_username);
            changed = true;
        }
        let active_theme = settings["active_theme_id"].as_str().unwrap_or("").trim().chars().take(64).collect::<String>();
        if settings["active_theme_id"].as_str() != Some(active_theme.as_str()) {
            settings["active_theme_id"] = json!(active_theme);
            changed = true;
        }
        let remote_path_parts = normalize_cloud_path(settings["cloud_remote_path"].as_str().unwrap_or(""));
        if settings["cloud_remote_path"].as_str() != Some(remote_path_parts.as_str()) {
            settings["cloud_remote_path"] = json!(remote_path_parts);
            changed = true;
        }
        if settings["cloud_include_accounts"].as_bool() != Some(false) {
            settings["cloud_include_accounts"] = json!(false);
            changed = true;
        }
        let cloud_sync = settings["cloud_last_sync"].as_f64().unwrap_or(0.0).max(0.0);
        if settings["cloud_last_sync"].as_f64() != Some(cloud_sync) {
            settings["cloud_last_sync"] = json!(cloud_sync);
            changed = true;
        }
        if !settings["http_proxy"].as_str().unwrap_or("").trim().is_empty() {
            let proxy = settings["http_proxy"].as_str().unwrap_or("").trim().to_owned();
            let valid = url::Url::parse(&proxy)
                .map(|url| {
                    matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") && url.host_str().is_some()
                })
                .unwrap_or(false);
            if !valid {
                settings["http_proxy"] = json!("");
                changed = true;
            }
        }
        let environment_client_id = configured_client_id();
        let stored_client_id = settings["client_id"].as_str().unwrap_or("").to_owned();
        if !stored_client_id.is_empty()
            && (LEGACY_MSA_CLIENT_IDS.contains(&stored_client_id.to_lowercase().as_str())
                || !is_valid_client_id(&stored_client_id))
        {
            settings["client_id"] = json!(environment_client_id);
            changed = true;
        } else if stored_client_id.is_empty() && !environment_client_id.is_empty() {
            settings["client_id"] = json!(environment_client_id);
            changed = true;
        }
        match parse_memory_kib(settings["memory"].as_str().unwrap_or("auto")) {
            Ok(mut memory_kib) => {
                let total_kib = system_memory_kib();
                if total_kib > 0 && memory_kib > total_kib {
                    memory_kib = total_kib;
                    changed = true;
                }
                let normalized = format_memory_kib(memory_kib);
                if settings["memory"].as_str() != Some(normalized.as_str()) {
                    settings["memory"] = json!(normalized);
                    changed = true;
                }
            }
            Err(_) => {
                settings["memory"] = json!(format_memory_kib(recommended_memory_kib()));
                changed = true;
            }
        }
        if settings["offline_debug_limit_seconds"].as_i64() != Some(0) {
            settings["offline_debug_limit_seconds"] = json!(0);
            changed = true;
        }
        let developer_mode = coerce_bool(&settings["developer_mode"], false);
        if settings["developer_mode"].as_bool() != Some(developer_mode) {
            settings["developer_mode"] = json!(developer_mode);
            changed = true;
        }
        let downloads = bounded_int(&settings["max_downloads"], 1, 32, 8);
        if settings["max_downloads"].as_i64() != Some(downloads) {
            settings["max_downloads"] = json!(downloads);
            changed = true;
        }
        let normalized_behavior = normalize_behavior(&settings);
        if Value::Object(normalized_behavior.clone()) != Value::Object(behavior_values(&settings)) {
            apply(&mut settings, &normalized_behavior);
            changed = true;
        }
        let normalized_appearance = normalize_appearance(&settings);
        if Value::Object(normalized_appearance.clone()) != Value::Object(appearance_values(&settings)) {
            apply(&mut settings, &normalized_appearance);
            changed = true;
        }
        let normalized_sources = normalize_sources(&settings);
        if Value::Object(normalized_sources.clone()) != Value::Object(source_values(&settings)) {
            apply(&mut settings, &normalized_sources);
            changed = true;
        }
        let normalized_catalogs = normalize_resource_catalogs(
            &settings["resource_catalogs"],
            settings["catalog_source"].as_str().unwrap_or("modrinth"),
            settings["catalog_api_url"].as_str().unwrap_or(MODRINTH_API_URL),
            false,
        );
        let normalized_active = normalize_active_catalog_id(&settings["active_catalog_id"], &normalized_catalogs);
        if normalized_catalogs != settings["resource_catalogs"] || normalized_active != settings["active_catalog_id"] {
            settings["resource_catalogs"] = normalized_catalogs.clone();
            settings["active_catalog_id"] = json!(normalized_active);
            changed = true;
        }
        if let Some(active) = normalized_catalogs.as_array().and_then(|items| {
            items
                .iter()
                .find(|item| item["id"].as_str() == normalized_active.as_str())
                .cloned()
        }) {
            if settings["catalog_source"] != active["kind"] || settings["catalog_api_url"] != active["api_url"] {
                settings["catalog_source"] = active["kind"].clone();
                settings["catalog_api_url"] = active["api_url"].clone();
                changed = true;
            }
        }
        let groups = normalize_groups(&settings["instance_groups"]);
        if groups != settings["instance_groups"] {
            settings["instance_groups"] = groups.clone();
            changed = true;
        }
        let pinned = normalize_pinned_groups(&settings["pinned_instance_groups"], Some(&groups));
        if pinned != settings["pinned_instance_groups"] {
            settings["pinned_instance_groups"] = pinned;
            changed = true;
        }
        let plugin_catalogs = super::plugin_catalogs::normalize_plugin_catalogs(&settings["plugin_catalogs"]);
        let plugin_active = normalize_active_catalog_id(&settings["active_plugin_catalog_id"], &plugin_catalogs);
        if plugin_catalogs != settings["plugin_catalogs"] || plugin_active != settings["active_plugin_catalog_id"] {
            settings["plugin_catalogs"] = plugin_catalogs;
            settings["active_plugin_catalog_id"] = plugin_active;
            changed = true;
        }
        let profiles = normalize_launcher_profiles(&settings["launcher_profiles"]);
        let profile_active = normalize_active_profile_id(&settings["active_launcher_profile_id"], &profiles);
        if profiles != settings["launcher_profiles"] || profile_active != settings["active_launcher_profile_id"] {
            settings["launcher_profiles"] = profiles;
            settings["active_launcher_profile_id"] = profile_active;
            changed = true;
        }
        if changed {
            self.save(&settings)?;
        }
        Ok(settings)
    }

    pub fn save(&self, settings: &Value) -> Result<(), String> {
        atomic_json_write(&self.paths.settings_file, settings, true)
    }

    pub fn settings(&self) -> &Value {
        &self.settings
    }

    pub fn str_setting(&self, key: &str) -> String {
        self.settings.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
    }

    pub fn int_setting(&self, key: &str) -> i64 {
        self.settings.get(key).and_then(Value::as_i64).unwrap_or(0)
    }

    pub fn bool_setting(&self, key: &str) -> bool {
        self.settings.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn object_setting(&self, key: &str) -> Value {
        self.settings
            .get(key)
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}))
    }

    pub fn data_home_game_dir(&self) -> PathBuf {
        self.paths.data_home.join("game")
    }

    /// Папка игры из настроек; создаётся с правами 0700.
    pub fn game_dir(&self) -> Result<PathBuf, String> {
        let path = self.validated_game_dir(&self.str_setting("game_dir"))?;
        std::fs::create_dir_all(&path).map_err(|error| format!("Не удалось создать папку игры: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
        }
        Ok(path)
    }

    pub fn set_values(&mut self, values: &Value) -> Result<(), String> {
        let Value::Object(map) = values else {
            return Err("Настройки имеют неожиданный формат".to_owned());
        };
        let original = self.settings.clone();
        let mut next = original.clone();
        for (key, value) in map {
            if let Err(error) = set_value_on(&mut next, key, value) {
                self.settings = original;
                return Err(error);
            }
        }
        let pinned = normalize_pinned_groups(&next["pinned_instance_groups"], Some(&normalize_groups(&next["instance_groups"])));
        next["pinned_instance_groups"] = pinned;
        self.save(&next)?;
        self.settings = next;
        Ok(())
    }

    fn validated_game_dir(&self, value: &str) -> Result<PathBuf, String> {
        let expanded = if let Some(rest) = value.strip_prefix('~') {
            home_dir().join(rest.trim_start_matches(['/']))
        } else {
            PathBuf::from(value)
        };
        // canonicalize строг: требует существования пути. Питонья пара этого
        // валидатора (resolve(strict=False)) пропускает несуществующие пути —
        // каталог создаётся позже, при первом обращении к game_dir(). Без
        // нестрогого резолва свежая установка не давала войти в Microsoft:
        // загрузка настроек валидировала ещё не созданную папку.
        let path = expanded.canonicalize().unwrap_or_else(|_| {
            if expanded.is_absolute() {
                expanded.clone()
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(&expanded)
            }
        });
        let forbidden = [
            PathBuf::from("/"),
            home_dir(),
            self.paths.config_home.clone(),
            self.paths.data_home.clone(),
        ];
        if forbidden.iter().any(|forbidden| path == *forbidden) {
            return Err("Нельзя использовать корень системы или домашний каталог как папку игры".to_owned());
        }
        Ok(path)
    }
}

fn configured_client_id() -> String {
    let value = std::env::var("NIVORA_LAUNCHER_CLIENT_ID")
        .unwrap_or_else(|_| DEFAULT_MSA_CLIENT_ID.to_owned())
        .trim()
        .to_owned();
    if LEGACY_MSA_CLIENT_IDS.contains(&value.to_lowercase().as_str()) {
        return String::new();
    }
    if is_valid_client_id(&value) {
        value
    } else {
        String::new()
    }
}

pub fn is_valid_client_id(value: &str) -> bool {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z")
        .unwrap()
        .is_match(value)
}

pub fn normalize_cloud_path(value: &str) -> String {
    let normalized = value.trim().replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.iter().any(|part| *part == ".." || part.contains('\0')) {
        return "nivora-launcher".to_owned();
    }
    let joined = parts.join("/");
    let truncated: String = joined.chars().take(512).collect();
    if truncated.is_empty() {
        "nivora-launcher".to_owned()
    } else {
        truncated
    }
}

fn behavior_values(settings: &Value) -> Map<String, Value> {
    let keys = [
        "startup_view", "last_view", "remember_last_view", "default_version_filter", "default_loader",
        "default_account_id", "open_last_instance", "launch_after_install", "auto_refresh_metadata",
        "metadata_refresh_hours", "download_timeout_seconds", "download_retries",
        "confirm_destructive_actions", "show_activity_panel", "show_toasts", "minimize_on_launch",
        "close_to_tray", "font_family", "keep_logs_days", "telemetry", "telemetry_endpoint",
        "keyboard_shortcuts",
    ];
    keys.iter()
        .map(|key| (key.to_string(), settings[*key].clone()))
        .collect()
}

fn appearance_values(settings: &Value) -> Map<String, Value> {
    let keys = [
        "theme", "active_theme_id", "wallpaper_path", "wallpaper_blur", "wallpaper_opacity", "accent_color",
        "sidebar_position", "sidebar_mode", "sidebar_width_mode", "sidebar_width", "sidebar_floating",
        "sidebar_visibility", "animations", "animation_settings", "density", "ui_scale", "radius_tokens",
        "page_themes", "page_layouts", "widget_visibility", "color_tokens",
    ];
    keys.iter()
        .map(|key| (key.to_string(), settings[*key].clone()))
        .collect()
}

fn source_values(settings: &Value) -> Map<String, Value> {
    let keys = [
        "login_source", "login_api_url", "catalog_source", "catalog_api_url", "data_source", "data_api_url",
    ];
    keys.iter()
        .map(|key| (key.to_string(), settings[*key].clone()))
        .collect()
}

fn apply(settings: &mut Value, values: &Map<String, Value>) {
    if let Some(target) = settings.as_object_mut() {
        for (key, value) in values {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn normalize_shortcuts(value: &Value) -> Value {
    let raw = value.as_object().cloned().unwrap_or_default();
    let mut result = Map::new();
    for (key, fallback) in DEFAULT_KEYBOARD_SHORTCUTS {
        let candidate = raw
            .get(*key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .trim()
            .chars()
            .take(40)
            .collect::<String>();
        result.insert(key.to_string(), json!(candidate));
    }
    Value::Object(result)
}

fn normalize_behavior(settings: &Value) -> Map<String, Value> {
    let views = ["instances", "accounts", "mods", "modpacks", "plugins", "lan-rooms", "themes", "settings"];
    let filters = ["all", "release", "snapshot", "old_beta", "old_alpha"];
    let loaders = ["vanilla", "fabric", "forge", "neoforge", "quilt"];
    let clamp_member = |value: &str, allowed: &[&str], fallback: &str| -> String {
        let cleaned = value.trim().to_lowercase();
        if allowed.contains(&cleaned.as_str()) {
            cleaned
        } else {
            fallback.to_owned()
        }
    };
    let startup_view = clamp_member(settings["startup_view"].as_str().unwrap_or(""), &views, "instances");
    let last_view_raw = clamp_member(settings["last_view"].as_str().unwrap_or(""), &views, "nope");
    let last_view = if last_view_raw == "nope" { startup_view.clone() } else { last_view_raw };
    let font_family = clamp_member(settings["font_family"].as_str().unwrap_or(""), &["system", "inter", "manrope", "mono"], "system");
    let mut telemetry_endpoint = settings["telemetry_endpoint"].as_str().unwrap_or("").trim().chars().take(400).collect::<String>();
    if !telemetry_endpoint.is_empty() {
        let parsed = url::Url::parse(&telemetry_endpoint);
        if !parsed.as_ref().map(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).unwrap_or(false) {
            telemetry_endpoint.clear();
        }
    }
    let default_account = settings["default_account_id"].as_str().unwrap_or("").trim().chars().take(200).collect::<String>();
    let mut result = Map::new();
    result.insert("startup_view".into(), json!(startup_view));
    result.insert("last_view".into(), json!(last_view));
    result.insert("remember_last_view".into(), json!(coerce_bool(&settings["remember_last_view"], true)));
    result.insert("default_version_filter".into(), json!(clamp_member(settings["default_version_filter"].as_str().unwrap_or(""), &filters, "release")));
    result.insert("default_loader".into(), json!(clamp_member(settings["default_loader"].as_str().unwrap_or(""), &loaders, "fabric")));
    result.insert("default_account_id".into(), json!(if default_account.is_empty() { "auto".to_owned() } else { default_account }));
    result.insert("open_last_instance".into(), json!(coerce_bool(&settings["open_last_instance"], true)));
    result.insert("launch_after_install".into(), json!(coerce_bool(&settings["launch_after_install"], false)));
    result.insert("auto_refresh_metadata".into(), json!(coerce_bool(&settings["auto_refresh_metadata"], true)));
    result.insert("metadata_refresh_hours".into(), json!(bounded_int(&settings["metadata_refresh_hours"], 1, 168, 6)));
    result.insert("download_timeout_seconds".into(), json!(bounded_int(&settings["download_timeout_seconds"], 5, 120, 30)));
    result.insert("download_retries".into(), json!(bounded_int(&settings["download_retries"], 1, 6, 3)));
    result.insert("confirm_destructive_actions".into(), json!(coerce_bool(&settings["confirm_destructive_actions"], true)));
    result.insert("show_activity_panel".into(), json!(coerce_bool(&settings["show_activity_panel"], true)));
    result.insert("show_toasts".into(), json!(coerce_bool(&settings["show_toasts"], true)));
    result.insert("minimize_on_launch".into(), json!(coerce_bool(&settings["minimize_on_launch"], false)));
    result.insert("close_to_tray".into(), json!(coerce_bool(&settings["close_to_tray"], false)));
    result.insert("font_family".into(), json!(font_family));
    result.insert("keep_logs_days".into(), json!(bounded_int(&settings["keep_logs_days"], 0, 365, 14)));
    result.insert("telemetry".into(), json!(coerce_bool(&settings["telemetry"], false)));
    result.insert("telemetry_endpoint".into(), json!(telemetry_endpoint));
    result.insert("keyboard_shortcuts".into(), normalize_shortcuts(&settings["keyboard_shortcuts"]));
    result
}

fn normalize_appearance(settings: &Value) -> Map<String, Value> {
    let valid_page_themes = ["inherit", "light", "dark", "glass"];
    let valid_page_layouts = ["compact", "balanced", "wide"];
    let page_themes = |allowed: &[&str], fallback: &str| {
        let raw = settings["page_themes"].as_object().cloned().unwrap_or_default();
        let mut map = Map::new();
        for page in PAGES {
            let value = raw.get(*page).and_then(Value::as_str).unwrap_or(fallback);
            map.insert(page.to_string(), json!(if allowed.contains(&value) { value } else { fallback }));
        }
        Value::Object(map)
    };
    let member = |value: &str, allowed: &[&str], fallback: &str| -> String {
        let cleaned = value.trim().to_lowercase();
        if allowed.contains(&cleaned.as_str()) {
            cleaned
        } else {
            fallback.to_owned()
        }
    };
    let mut color_tokens = normalize_color_tokens(&settings["color_tokens"]);
    let active_theme_id = settings["active_theme_id"]
        .as_str()
        .unwrap_or("midnight-signal")
        .trim()
        .to_lowercase()
        .chars()
        .take(64)
        .collect::<String>();
    let active_theme_id = if active_theme_id.is_empty() { "midnight-signal".to_owned() } else { active_theme_id };
    let accent = settings["accent_color"].as_str().unwrap_or("").to_lowercase();
    let accent = if Regex::new(r"^#[0-9a-f]{6}\z").unwrap().is_match(&accent) {
        accent
    } else {
        "#8b653f".to_owned()
    };
    color_tokens["accent"] = json!(accent.clone());
    let mut result = Map::new();
    result.insert("theme".into(), json!(member(settings["theme"].as_str().unwrap_or(""), &["system", "light", "dark", "glass"], "system")));
    result.insert("active_theme_id".into(), json!(active_theme_id));
    result.insert("wallpaper_path".into(), json!(settings["wallpaper_path"].as_str().unwrap_or("").chars().take(2048).collect::<String>()));
    result.insert("wallpaper_blur".into(), json!(bounded_int(&settings["wallpaper_blur"], 0, 32, 12)));
    result.insert("wallpaper_opacity".into(), json!(bounded_int(&settings["wallpaper_opacity"], 0, 80, 34)));
    result.insert("accent_color".into(), json!(accent));
    result.insert("sidebar_position".into(), json!(member(settings["sidebar_position"].as_str().unwrap_or(""), &["left", "right", "top", "bottom"], "left")));
    result.insert("sidebar_mode".into(), json!(member(settings["sidebar_mode"].as_str().unwrap_or(""), &["expanded", "adaptive", "compact"], "adaptive")));
    result.insert("sidebar_width_mode".into(), json!(member(settings["sidebar_width_mode"].as_str().unwrap_or(""), &["full", "content", "manual"], "content")));
    result.insert("sidebar_width".into(), json!(bounded_int(&settings["sidebar_width"], 168, 360, 228)));
    result.insert("sidebar_floating".into(), json!(coerce_bool(&settings["sidebar_floating"], false)));
    result.insert("sidebar_visibility".into(), json!(member(settings["sidebar_visibility"].as_str().unwrap_or(""), &["always", "hover"], "always")));
    result.insert("animations".into(), json!(member(settings["animations"].as_str().unwrap_or(""), &["full", "reduced", "off"], "full")));
    result.insert("animation_settings".into(), normalize_animation_settings(&settings["animation_settings"]));
    result.insert("density".into(), json!(member(settings["density"].as_str().unwrap_or(""), &["compact", "comfortable", "spacious"], "comfortable")));
    result.insert("ui_scale".into(), json!(bounded_int(&settings["ui_scale"], 80, 125, 100)));
    result.insert("radius_tokens".into(), normalize_radius_tokens(&settings["radius_tokens"]));
    result.insert("page_themes".into(), page_themes(&valid_page_themes, "inherit"));
    result.insert("page_layouts".into(), page_themes(&valid_page_layouts, "balanced"));
    let raw_visibility = settings["widget_visibility"].as_object().cloned().unwrap_or_default();
    let mut visibility = Map::new();
    for key in ["instances.workspace", "accounts.status", "mods.context"] {
        visibility.insert(key.to_string(), json!(coerce_bool(raw_visibility.get(key).unwrap_or(&json!(true)), true)));
    }
    result.insert("widget_visibility".into(), Value::Object(visibility));
    result.insert("color_tokens".into(), Value::Object(color_tokens));
    result
}

fn normalize_animation_settings(value: &Value) -> Value {
    let styles = ["fade", "slide", "scale", "none"];
    let raw = value.as_object().cloned().unwrap_or_default();
    let fallback_style = raw
        .get("page_style")
        .and_then(Value::as_str)
        .unwrap_or("fade")
        .trim()
        .to_lowercase();
    let fallback_style = if styles.contains(&fallback_style.as_str()) { fallback_style } else { "slide".to_owned() };
    let page_duration = bounded_int(raw.get("page_duration_ms").unwrap_or(&json!(220)), 0, 1000, 220);
    let raw_styles = raw.get("page_styles").and_then(Value::as_object).cloned().unwrap_or_default();
    let raw_durations = raw.get("page_durations_ms").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut page_styles = Map::new();
    let mut page_durations = Map::new();
    for page in PAGES {
        let style = raw_styles.get(*page).and_then(Value::as_str).unwrap_or(&fallback_style).trim().to_lowercase();
        page_styles.insert(page.to_string(), json!(if styles.contains(&style.as_str()) { style } else { fallback_style.clone() }));
        let fallback_duration = raw.get("page_duration_ms").cloned().unwrap_or(json!(220));
        let duration = raw_durations.get(*page).or(Some(&fallback_duration));
        page_durations.insert(page.to_string(), json!(bounded_int(duration.unwrap_or(&fallback_duration), 0, 1000, page_duration)));
    }
    json!({
        "page_style": fallback_style,
        "page_duration_ms": page_duration,
        "surface_duration_ms": bounded_int(raw.get("surface_duration_ms").unwrap_or(&json!(300)), 0, 1200, 300),
        "surface_stagger_ms": bounded_int(raw.get("surface_stagger_ms").unwrap_or(&json!(35)), 0, 200, 35),
        "page_styles": page_styles,
        "page_durations_ms": page_durations,
    })
}

pub fn normalize_overlay_settings(value: &Value) -> Value {
    let raw = value.as_object().cloned().unwrap_or_default();
    let raw_str = |key: &str, fallback: &str| raw.get(key).and_then(Value::as_str).unwrap_or(fallback).to_owned();
    let hotkey = |key: &str, fallback: &str| -> String {
        let text = raw.get(key).and_then(Value::as_str).unwrap_or(fallback).trim().chars().take(32).collect::<String>();
        let allowed = |character: char| character.is_ascii_alphanumeric() || character == '+' || character == '*';
        if !text.is_empty() && text.chars().all(allowed) {
            text
        } else {
            fallback.to_owned()
        }
    };
    let member = |value: &str, allowed: &[&str], fallback: &str| -> String {
        let cleaned = value.trim().to_lowercase();
        if allowed.contains(&cleaned.as_str()) { cleaned } else { fallback.to_owned() }
    };
    json!({
        "enabled": coerce_bool(raw.get("enabled").unwrap_or(&json!(true)), true),
        "hotkey_overlay": hotkey("hotkey_overlay", "Shift+Tab"),
        "hotkey_recording": hotkey("hotkey_recording", "F7"),
        "show_mods": coerce_bool(raw.get("show_mods").unwrap_or(&json!(true)), true),
        "show_metrics": coerce_bool(raw.get("show_metrics").unwrap_or(&json!(true)), true),
        "recording_fps": bounded_int(raw.get("recording_fps").unwrap_or(&json!(60)), 15, 120, 60),
        "recording_quality": bounded_int(raw.get("recording_quality").unwrap_or(&json!(23)), 15, 35, 23),
        "recording_encoder": member(&raw_str("recording_encoder", "auto"), &["auto", "nvenc", "vaapi", "x264"], "auto"),
        "recording_container": member(&raw_str("recording_container", "mp4"), &["mp4", "mkv"], "mp4"),
        "recording_audio": coerce_bool(raw.get("recording_audio").unwrap_or(&json!(true)), true),
        "position": member(&raw_str("position", "top-left"), &["top-left", "top-right", "bottom-left", "bottom-right"], "top-left"),
        "scale": bounded_int(raw.get("scale").unwrap_or(&json!(100)), 80, 130, 100),
        "opacity": bounded_int(raw.get("opacity").unwrap_or(&json!(96)), 60, 100, 96),
        "compact": coerce_bool(raw.get("compact").unwrap_or(&json!(false)), false),
        "show_clock": coerce_bool(raw.get("show_clock").unwrap_or(&json!(true)), true),
        "show_system_metrics": coerce_bool(raw.get("show_system_metrics").unwrap_or(&json!(true)), true),
        "metrics_cpu": coerce_bool(raw.get("metrics_cpu").unwrap_or(&json!(true)), true),
        "metrics_ram": coerce_bool(raw.get("metrics_ram").unwrap_or(&json!(true)), true),
        "metrics_gpu": coerce_bool(raw.get("metrics_gpu").unwrap_or(&json!(true)), true),
        "metrics_disk": coerce_bool(raw.get("metrics_disk").unwrap_or(&json!(true)), true),
        "metrics_temperatures": coerce_bool(raw.get("metrics_temperatures").unwrap_or(&json!(true)), true),
        "system_metrics_display": member(&raw_str("system_metrics_display", "overlay"), &["overlay", "always"], "overlay"),
        "show_chat": coerce_bool(raw.get("show_chat").unwrap_or(&json!(true)), true),
    })
}

pub fn normalize_backup_schedule(value: &Value) -> Value {
    let raw = value.as_object().cloned().unwrap_or_default();
    let destination = raw.get("destination").and_then(Value::as_str).unwrap_or("local").trim().to_lowercase();
    json!({
        "enabled": coerce_bool(raw.get("enabled").unwrap_or(&json!(false)), false),
        "interval_hours": bounded_int(raw.get("interval_hours").unwrap_or(&json!(24)), 1, 24 * 30, 24),
        "keep": bounded_int(raw.get("keep").unwrap_or(&json!(5)), 1, 50, 5),
        "destination": if destination == "cloud" { "cloud" } else { "local" },
    })
}

pub fn normalize_launcher_profiles(value: &Value) -> Value {
    let raw_values = value.as_array().cloned().unwrap_or_default();
    let mut result: Vec<Value> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for (index, item) in raw_values.iter().take(24).enumerate() {
        let Some(fields) = item.as_object() else { continue };
        let get_str = |key: &str, fallback: &str| fields.get(key).and_then(Value::as_str).unwrap_or(fallback).to_owned();
        let raw_id = Regex::new(r"[^a-z0-9_-]+")
            .unwrap()
            .replace_all(get_str("id", "").trim().to_lowercase().as_str(), "-")
            .trim_matches(['-', '_'])
            .to_owned();
        let mut profile_id: String = if raw_id.is_empty() {
            if index == 0 { "default".to_owned() } else { format!("profile-{}", index + 1) }
        } else {
            raw_id.chars().take(48).collect()
        };
        let base_id = profile_id.clone();
        let mut suffix = 2;
        while used.contains(&profile_id) {
            profile_id = format!("{}-{suffix}", base_id.chars().take(42).collect::<String>());
            suffix += 1;
        }
        used.push(profile_id.clone());
        let mut memory = get_str("memory", "auto").trim().to_owned();
        if memory.is_empty() {
            memory = "auto".to_owned();
        }
        if !memory.eq_ignore_ascii_case("auto") && !memory.eq_ignore_ascii_case("automatic") {
            memory = match parse_memory_kib(&memory) {
                Ok(kib) => format_memory_kib(kib),
                Err(_) => "auto".to_owned(),
            };
        }
        let graphics_mode = get_str("graphics_mode", "auto").trim().to_lowercase();
        let graphics_mode = if ["auto", "nvidia", "mesa", "software"].contains(&graphics_mode.as_str()) {
            graphics_mode
        } else {
            "auto".to_owned()
        };
        let preset = get_str("jvm_preset", "none").trim().to_lowercase();
        let jvm_preset = if ["none", "fps", "balanced", "heavy_modpack", "compatibility", "streaming"].contains(&preset.as_str()) {
            preset
        } else {
            "none".to_owned()
        };
        let name = get_str("name", "Профиль запуска").trim().chars().take(80).collect::<String>();
        let name = if name.is_empty() { format!("Профиль {}", index + 1) } else { name };
        let account_id = get_str("account_id", "auto").trim().chars().take(200).collect::<String>();
        result.push(json!({
            "id": profile_id,
            "name": name,
            "memory": memory,
            "java_path": get_str("java_path", "").trim().chars().take(2048).collect::<String>(),
            "graphics_mode": graphics_mode,
            "account_id": if account_id.is_empty() { "auto".to_owned() } else { account_id },
            "window_width": bounded_int(fields.get("window_width").unwrap_or(&json!(1280)), 320, 8192, 1280),
            "window_height": bounded_int(fields.get("window_height").unwrap_or(&json!(720)), 240, 8192, 720),
            "fullscreen": coerce_bool(fields.get("fullscreen").unwrap_or(&json!(false)), false),
            "quick_play_world": safe_profile_text(fields.get("quick_play_world")),
            "quick_play_server": safe_profile_text(fields.get("quick_play_server")),
            "jvm_arguments": profile_arguments(fields.get("jvm_arguments")),
            "game_arguments": profile_arguments(fields.get("game_arguments")),
            "jvm_preset": jvm_preset,
        }));
    }
    if result.is_empty() {
        result.push(default_launcher_profile());
    }
    Value::Array(result)
}

fn safe_profile_text(value: Option<&Value>) -> String {
    let text = value.and_then(Value::as_str).unwrap_or("").trim();
    if text.contains('\0') || text.contains('\n') || text.contains('\r') {
        return String::new();
    }
    text.chars().take(512).collect()
}

fn profile_arguments(value: Option<&Value>) -> Value {
    let Some(items) = value.and_then(Value::as_array) else {
        return json!([]);
    };
    let mut result: Vec<String> = Vec::new();
    for item in items.iter().take(32) {
        let text = safe_profile_text(Some(item));
        if !text.is_empty() {
            result.push(text);
        }
    }
    Value::Array(result.into_iter().map(Value::String).collect())
}

fn normalize_active_profile_id(value: &Value, profiles: &Value) -> Value {
    let candidate = value.as_str().unwrap_or("").trim().to_lowercase();
    let found = profiles
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"].as_str() == Some(candidate.as_str())))
        .map(|item| item["id"].clone());
    match found {
        Some(id) => id,
        None => profiles.as_array().and_then(|items| items.first().map(|item| item["id"].clone())).unwrap_or(json!("default")),
    }
}

fn normalize_sources(settings: &Value) -> Map<String, Value> {
    let developer_mode = coerce_bool(&settings["developer_mode"], false);
    let member = |value: &str, allowed: &[&str], fallback: &str| -> String {
        let cleaned = value.trim().to_lowercase();
        if allowed.contains(&cleaned.as_str()) { cleaned } else { fallback.to_owned() }
    };
    let mut result = Map::new();
    result.insert("login_source".into(), json!(member(settings["login_source"].as_str().unwrap_or(""), &["microsoft", "disabled"], "microsoft")));
    result.insert(
        "login_api_url".into(),
        json!(normalize_api_url(
            settings["login_api_url"].as_str().unwrap_or(""),
            MSA_LOGIN_API_URL,
            Some(&["login.microsoftonline.com"]),
            developer_mode,
        )),
    );
    result.insert("catalog_source".into(), json!(member(settings["catalog_source"].as_str().unwrap_or(""), &["modrinth", "custom"], "modrinth")));
    result.insert("catalog_api_url".into(), json!(normalize_api_url(settings["catalog_api_url"].as_str().unwrap_or(""), MODRINTH_API_URL, None, false)));
    result.insert("data_source".into(), json!(member(settings["data_source"].as_str().unwrap_or(""), &["mojang", "custom"], "mojang")));
    result.insert(
        "data_api_url".into(),
        json!(normalize_api_url(settings["data_api_url"].as_str().unwrap_or(""), VERSION_MANIFEST_URL, Some(&["piston-meta.mojang.com"]), developer_mode)),
    );
    result
}

pub fn normalize_api_url(value: &str, fallback: &str, allowed_hosts: Option<&[&str]>, developer_mode: bool) -> String {
    let raw = value.trim();
    let Ok(parsed) = url::Url::parse(raw) else {
        return fallback.to_owned();
    };
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    let host = host.trim_end_matches('.');
    let port_ok = parsed.port().is_none() || parsed.port() == Some(443);
    let host_ok = match allowed_hosts {
        Some(hosts) if !developer_mode => hosts.contains(&host),
        _ => true,
    };
    if parsed.scheme() != "https" || host.is_empty() || !parsed.username().is_empty() || parsed.password().is_some() || !port_ok || !host_ok {
        return fallback.to_owned();
    }
    raw.trim_end_matches('/').to_owned()
}

pub fn normalize_color_tokens(value: &Value) -> Map<String, Value> {
    let raw = value.as_object().cloned().unwrap_or_default();
    let hex = Regex::new(r"^#[0-9a-fA-F]{6}\z").unwrap();
    let mut result = Map::new();
    for (key, fallback) in DEFAULT_COLOR_TOKENS {
        let candidate = raw.get(*key).and_then(Value::as_str).unwrap_or(fallback).trim().to_lowercase();
        result.insert(key.to_string(), json!(if hex.is_match(&candidate) { candidate } else { (*fallback).to_owned() }));
    }
    result
}

pub fn normalize_radius_tokens(value: &Value) -> Value {
    let raw = value.as_object().cloned().unwrap_or_default();
    let mut result = Map::new();
    for (key, fallback) in DEFAULT_RADIUS_TOKENS {
        let number = match raw.get(*key) {
            Some(Value::Number(number)) => number.as_i64(),
            Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
            _ => None,
        };
        result.insert(key.to_string(), json!(number.map(|value| value.clamp(0, 24)).unwrap_or(*fallback)));
    }
    Value::Object(result)
}

pub fn normalize_groups(value: &Value) -> Value {
    let Some(items) = value.as_array() else {
        return json!([]);
    };
    let mut result: Vec<String> = Vec::new();
    for item in items {
        let name: String = match item {
            Value::String(text) => text.trim().chars().take(40).collect(),
            _ => item.to_string().chars().take(40).collect(),
        };
        if !name.is_empty() && !result.contains(&name) {
            result.push(name);
        }
    }
    result.sort_by_key(|name| name.to_lowercase());
    result.truncate(100);
    Value::Array(result.into_iter().map(Value::String).collect())
}

pub fn normalize_pinned_groups(value: &Value, groups: Option<&Value>) -> Value {
    let Some(items) = value.as_array() else {
        return json!([]);
    };
    let allowed: Option<Vec<String>> = groups.map(|groups| {
        groups
            .as_array()
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    });
    let mut result: Vec<String> = Vec::new();
    for item in items {
        let name: String = match item {
            Value::String(text) => text.trim().chars().take(40).collect(),
            _ => item.to_string().chars().take(40).collect(),
        };
        let permitted = allowed
            .as_ref()
            .map(|allowed| allowed.contains(&name))
            .unwrap_or(true);
        if !name.is_empty() && !result.contains(&name) && permitted {
            result.push(name);
        }
        if result.len() == 3 {
            break;
        }
    }
    Value::Array(result.into_iter().map(Value::String).collect())
}

pub fn normalize_resource_catalogs(value: &Value, legacy_source: &str, legacy_url: &str, use_legacy: bool) -> Value {
    let mut raw_values: Vec<Value> = value.as_array().cloned().unwrap_or_default();
    if use_legacy
        && (legacy_source.trim().to_lowercase() == "custom" || legacy_url.trim_end_matches('/') != MODRINTH_API_URL)
    {
        raw_values = vec![json!({ "id": "custom", "name": "Свой каталог", "kind": "custom", "api_url": legacy_url })];
    }
    let mut result: Vec<Value> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for (index, item) in raw_values.iter().take(20).enumerate() {
        let Some(fields) = item.as_object() else { continue };
        let name = fields.get("name").and_then(Value::as_str).unwrap_or("").trim().chars().take(60).collect::<String>();
        let kind_raw = fields.get("kind").and_then(Value::as_str).unwrap_or("custom").trim().to_lowercase();
        let kind = if ["modrinth", "ftb", "technic", "custom"].contains(&kind_raw.as_str()) {
            kind_raw.clone()
        } else {
            "custom".to_owned()
        };
        let fallback = match kind.as_str() {
            "modrinth" => MODRINTH_API_URL,
            "ftb" => FTB_MODPACK_API_URL,
            "technic" => TECHNIC_API_URL,
            _ => MODRINTH_API_URL,
        };
        let api_url = normalize_api_url(
            fields.get("api_url").and_then(Value::as_str).unwrap_or(fallback),
            fallback,
            None,
            false,
        );
        let raw_id = Regex::new(r"[^a-z0-9_-]+")
            .unwrap()
            .replace_all(fields.get("id").and_then(Value::as_str).unwrap_or("").trim().to_lowercase().as_str(), "-")
            .trim_matches(['-', '_'])
            .to_owned();
        let mut catalog_id: String = if raw_id.is_empty() {
            if index == 0 && kind == "modrinth" { "modrinth".to_owned() } else { format!("catalog-{}", index + 1) }
        } else {
            raw_id.chars().take(32).collect()
        };
        let base_id = catalog_id.clone();
        let mut suffix = 2;
        while used.contains(&catalog_id) {
            catalog_id = format!("{}-{suffix}", base_id.chars().take(28).collect::<String>());
            suffix += 1;
        }
        used.push(catalog_id.clone());
        let default_name = match kind.as_str() {
            "modrinth" => "Modrinth".to_owned(),
            "ftb" => "Feed The Beast".to_owned(),
            "technic" => "Technic".to_owned(),
            _ => format!("Каталог {}", index + 1),
        };
        result.push(json!({
            "id": catalog_id,
            "name": if name.is_empty() { default_name } else { name },
            "kind": kind,
            "api_url": api_url,
        }));
    }
    if result.is_empty() {
        let defaults = default_resource_catalogs();
        if let Some(first) = defaults.as_array().and_then(|items| items.first()) {
            result.push(first.clone());
        }
    }
    Value::Array(result)
}

pub fn normalize_active_catalog_id(value: &Value, catalogs: &Value) -> Value {
    let candidate = value.as_str().unwrap_or("").trim().to_lowercase();
    let found = catalogs
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"].as_str() == Some(candidate.as_str())))
        .map(|item| item["id"].clone());
    match found {
        Some(id) => id,
        None => catalogs
            .as_array()
            .and_then(|items| items.first().map(|item| item["id"].clone()))
            .unwrap_or(json!("")),
    }
}

/// Запись одного ключа настроек с валидацией — общий путь для save_settings
/// и внутренних обновлений.
pub fn set_value_on(settings: &mut Value, key: &str, value: &Value) -> Result<(), String> {
    if settings.get(key).is_none() {
        return Err(format!("Неизвестная настройка: {key}"));
    }
    let current = settings[key].clone();
    let mut value = value.clone();
    match key {
        "client_id" => {
            let client_id = value.as_str().unwrap_or("").trim().to_owned();
            if !client_id.is_empty()
                && (LEGACY_MSA_CLIENT_IDS.contains(&client_id.to_lowercase().as_str()) || !is_valid_client_id(&client_id))
            {
                return Err("Укажите client ID собственного Azure public-client приложения".to_owned());
            }
            value = json!(client_id);
        }
        "memory" => {
            let memory_kib = parse_memory_kib(value.as_str().unwrap_or("auto"))?;
            let total_kib = system_memory_kib();
            if total_kib > 0 && memory_kib > total_kib {
                return Err(format!("Память JVM не может превышать доступные {}", format_memory_kib(total_kib)));
            }
            value = json!(format_memory_kib(memory_kib));
        }
        "game_dir" => {
            value = json!(value.as_str().unwrap_or("").trim());
        }
        "max_downloads" => {
            value = json!(bounded_int(&value, 1, 32, 8));
        }
        "http_proxy" => {
            let proxy = value.as_str().unwrap_or("").trim().to_owned();
            if !proxy.is_empty() {
                let valid = url::Url::parse(&proxy)
                    .map(|url| {
                        matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") && url.host_str().is_some()
                    })
                    .unwrap_or(false);
                if !valid {
                    return Err("Прокси должен иметь вид http://host:port, https://host:port или socks5://host:port".to_owned());
                }
            }
            value = json!(proxy);
        }
        "cloud_url" => {
            let raw_url = value.as_str().unwrap_or("").trim().trim_end_matches('/').to_owned();
            if !raw_url.is_empty() {
                let valid = url::Url::parse(&raw_url)
                    .map(|url| {
                        url.scheme() == "https"
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                    })
                    .unwrap_or(false);
                if !valid {
                    return Err("WebDAV endpoint должен иметь вид https://host/remote.php/dav/files/user".to_owned());
                }
                value = json!(raw_url);
            } else {
                value = json!("");
            }
        }
        "active_theme_id" => value = json!(value.as_str().unwrap_or("").trim().chars().take(64).collect::<String>()),
        "cloud_username" => value = json!(value.as_str().unwrap_or("").trim().chars().take(320).collect::<String>()),
        "cloud_remote_path" => {
            let raw = value.as_str().unwrap_or("");
            let has_bad_segment = raw
                .trim()
                .replace('\\', "/")
                .split('/')
                .any(|part| part == ".." || part.contains('\0'));
            if has_bad_segment {
                return Err("Путь облачной папки содержит недопустимые сегменты".to_owned());
            }
            value = json!(normalize_cloud_path(raw));
        }
        "cloud_include_accounts" => value = json!(false),
        "accent_color" => {
            let accent = value.as_str().unwrap_or("").trim().to_lowercase();
            if !Regex::new(r"^#[0-9a-f]{6}\z").unwrap().is_match(&accent) {
                return Err("Акцентный цвет должен быть в формате #rrggbb".to_owned());
            }
            value = json!(accent);
        }
        "graphics_mode" => {
            let mode = value.as_str().unwrap_or("").trim().to_lowercase();
            if !["auto", "nvidia", "mesa", "software"].contains(&mode.as_str()) {
                return Err(format!("Неизвестный graphics mode: {mode}"));
            }
            value = json!(mode);
        }
        "launcher_profiles" => value = normalize_launcher_profiles(&value),
        "active_launcher_profile_id" => value = json!(value.as_str().unwrap_or("").trim().to_lowercase()),
        "animation_settings" => value = normalize_animation_settings(&value),
        "overlay_settings" => value = normalize_overlay_settings(&value),
        "backup_schedule" => value = normalize_backup_schedule(&value),
        "wallpaper_path" => value = json!(value.as_str().unwrap_or("").trim().chars().take(2048).collect::<String>()),
        "ui_scale" => value = json!(bounded_int(&value, 80, 125, 100)),
        "login_api_url" | "catalog_api_url" | "data_api_url" => {
            let developer_mode = coerce_bool(&settings["developer_mode"], false);
            let (fallback, restricted): (&str, Option<&[&str]>) = match key {
                "login_api_url" => (MSA_LOGIN_API_URL, Some(&["login.microsoftonline.com"])),
                "catalog_api_url" => (MODRINTH_API_URL, None),
                _ => (VERSION_MANIFEST_URL, Some(&["piston-meta.mojang.com"])),
            };
            value = json!(normalize_api_url(value.as_str().unwrap_or(""), fallback, restricted, developer_mode));
        }
        "instance_groups" => value = normalize_groups(&value),
        "pinned_instance_groups" => value = normalize_pinned_groups(&value, None),
        "resource_catalogs" => {
            value = normalize_resource_catalogs(
                &value,
                settings["catalog_source"].as_str().unwrap_or("modrinth"),
                settings["catalog_api_url"].as_str().unwrap_or(MODRINTH_API_URL),
                false,
            )
        }
        "plugin_catalogs" => value = super::plugin_catalogs::normalize_plugin_catalogs(&value),
        "active_catalog_id" | "active_plugin_catalog_id" => value = json!(value.as_str().unwrap_or("").trim().to_lowercase()),
        "color_tokens" => {
            let mut merged = settings["color_tokens"].clone();
            if let (Some(target), Some(source)) = (merged.as_object_mut(), value.as_object()) {
                for (color_key, color_value) in source {
                    target.insert(color_key.clone(), color_value.clone());
                }
            }
            value = Value::Object(normalize_color_tokens(&merged));
        }
        "radius_tokens" => {
            let mut merged = settings["radius_tokens"].clone();
            if let (Some(target), Some(source)) = (merged.as_object_mut(), value.as_object()) {
                for (radius_key, radius_value) in source {
                    target.insert(radius_key.clone(), radius_value.clone());
                }
            }
            value = normalize_radius_tokens(&merged);
        }
        "page_themes" | "page_layouts" | "widget_visibility" => {
            let Some(map) = value.as_object() else {
                return Err(format!("{key} должен быть объектом"));
            };
            let converted: Map<String, Value> = map
                .iter()
                .map(|(map_key, map_value)| {
                    if key == "widget_visibility" {
                        (map_key.clone(), json!(coerce_bool(map_value, true)))
                    } else {
                        (map_key.clone(), json!(map_value.as_str().unwrap_or("")))
                    }
                })
                .collect();
            value = Value::Object(converted);
        }
        "keyboard_shortcuts" => value = normalize_shortcuts(&value),
        "language" => {
            // null = «язык не выбран» (первый запуск спросит), иначе 'ru'|'en'.
            value = match &value {
                Value::Null => Value::Null,
                other => match other.as_str().map(|text| text.trim().to_lowercase()) {
                    Some(code) if code == "ru" || code == "en" => json!(code),
                    _ => return Err("Неизвестный язык интерфейса".to_owned()),
                },
            };
        }
        _ => {}
    }
    // Приведение типов к типу текущего значения.
    value = match &current {
        Value::Bool(_) => json!(coerce_bool(&value, current.as_bool().unwrap_or(false))),
        Value::Number(_) => match value.as_str().and_then(|text| text.trim().parse::<i64>().ok()) {
            Some(number) => json!(number),
            None => value,
        },
        _ => value,
    };
    settings[key] = value.clone();
    // Связанные ключи пересчитываются после записи.
    let behavior_keys = [
        "startup_view", "last_view", "remember_last_view", "default_version_filter", "default_loader",
        "default_account_id", "open_last_instance", "launch_after_install", "auto_refresh_metadata",
        "metadata_refresh_hours", "download_timeout_seconds", "download_retries",
        "confirm_destructive_actions", "show_activity_panel", "show_toasts", "minimize_on_launch",
        "close_to_tray", "font_family", "keep_logs_days", "telemetry", "telemetry_endpoint",
        "keyboard_shortcuts",
    ];
    if behavior_keys.contains(&key) {
        let normalized = normalize_behavior(settings);
        apply(settings, &normalized);
    }
    if key == "accent_color" {
        let mut tokens = normalize_color_tokens(&settings["color_tokens"]);
        tokens.insert("accent".into(), value);
        settings["color_tokens"] = Value::Object(tokens);
    } else if key == "color_tokens" {
        settings["accent_color"] = settings["color_tokens"]["accent"].clone();
    }
    let appearance_keys = [
        "theme", "wallpaper_path", "wallpaper_blur", "wallpaper_opacity", "accent_color",
        "sidebar_position", "sidebar_mode", "sidebar_width_mode", "sidebar_width", "sidebar_floating",
        "sidebar_visibility", "animations", "animation_settings", "density", "ui_scale",
        "radius_tokens", "page_themes", "page_layouts", "widget_visibility", "color_tokens",
    ];
    if appearance_keys.contains(&key) {
        let normalized = normalize_appearance(settings);
        apply(settings, &normalized);
    }
    if ["login_source", "login_api_url", "catalog_source", "catalog_api_url", "data_source", "data_api_url", "instance_groups", "developer_mode"]
        .contains(&key)
    {
        let normalized = normalize_sources(settings);
        apply(settings, &normalized);
        settings["instance_groups"] = normalize_groups(&settings["instance_groups"]);
    }
    if ["resource_catalogs", "active_catalog_id"].contains(&key) {
        let catalogs = normalize_resource_catalogs(
            &settings["resource_catalogs"],
            settings["catalog_source"].as_str().unwrap_or("modrinth"),
            settings["catalog_api_url"].as_str().unwrap_or(MODRINTH_API_URL),
            false,
        );
        let active_id = normalize_active_catalog_id(&settings["active_catalog_id"], &catalogs);
        settings["resource_catalogs"] = catalogs.clone();
        settings["active_catalog_id"] = active_id.clone();
        if let Some(active) = catalogs.as_array().and_then(|items| {
            items.iter().find(|item| item["id"] == active_id).cloned()
        }) {
            settings["catalog_source"] = active["kind"].clone();
            settings["catalog_api_url"] = active["api_url"].clone();
        }
    }
    if ["plugin_catalogs", "active_plugin_catalog_id"].contains(&key) {
        let catalogs = super::plugin_catalogs::normalize_plugin_catalogs(&settings["plugin_catalogs"]);
        let active_id = normalize_active_catalog_id(&settings["active_plugin_catalog_id"], &catalogs);
        settings["plugin_catalogs"] = catalogs;
        settings["active_plugin_catalog_id"] = active_id;
    }
    if ["launcher_profiles", "active_launcher_profile_id"].contains(&key) {
        let profiles = normalize_launcher_profiles(&settings["launcher_profiles"]);
        let active_id = normalize_active_profile_id(&settings["active_launcher_profile_id"], &profiles);
        settings["launcher_profiles"] = profiles;
        settings["active_launcher_profile_id"] = active_id;
    }
    if ["instance_groups", "pinned_instance_groups"].contains(&key) {
        let pinned = normalize_pinned_groups(&settings["pinned_instance_groups"], Some(&settings["instance_groups"]));
        settings["pinned_instance_groups"] = pinned;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_env(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("nivora-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (root.join("config"), root.join("data"), root.join("cache"))
    }

    fn store_with_env(tag: &str) -> ConfigStore {
        let (config, data, cache) = temp_env(tag);
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_CACHE_HOME", &cache);
        ConfigStore::load().unwrap()
    }

    #[test]
    fn defaults_are_written_on_first_load() {
        let store = store_with_env("defaults");
        assert_eq!(store.str_setting("cloud_remote_path"), "nivora-launcher");
        assert!(store.str_setting("client_id").starts_with("ce777973"));
        assert!(store.paths.settings_file.is_file());
    }

    #[test]
    fn memory_helpers_round_trip() {
        assert_eq!(parse_memory_kib("8GiB").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_memory_kib("512MiB").unwrap(), 512 * 1024);
        assert_eq!(parse_memory_kib("1024").unwrap(), 1024 * 1024);
        assert_eq!(parse_memory_kib_or("auto", Some(4096)).unwrap(), 4096);
        assert!(parse_memory_kib("abc").is_err());
        assert_eq!(format_memory_kib(8 * 1024 * 1024), "8GiB");
        assert_eq!(format_memory_kib(512 * 1024), "512MiB");
        assert_eq!(java_memory_value("4GiB").unwrap(), "4G");
    }

    #[test]
    fn set_values_validates_and_normalizes() {
        let mut store = store_with_env("set-values");
        store
            .set_values(&json!({ "memory": "4096", "accent_color": "#ABCDEF", "ui_scale": 999, "cloud_remote_path": "a/../b" }))
            .unwrap_err();
        store
            .set_values(&json!({ "memory": "4096", "accent_color": "#abcdef", "ui_scale": 999 }))
            .unwrap();
        assert_eq!(store.str_setting("memory"), "4GiB");
        assert_eq!(store.str_setting("accent_color"), "#abcdef");
        assert_eq!(store.int_setting("ui_scale"), 125);
        assert_eq!(
            store.settings()["color_tokens"]["accent"],
            json!("#abcdef")
        );
    }

    #[test]
    fn proxy_and_cloud_validation() {
        let mut settings = default_settings_object();
        set_value_on(&mut settings, "http_proxy", &json!("socks5://127.0.0.1:9050")).unwrap();
        assert_eq!(settings["http_proxy"], json!("socks5://127.0.0.1:9050"));
        assert!(set_value_on(&mut settings, "http_proxy", &json!("ftp://host")).is_err());
        assert!(set_value_on(&mut settings, "cloud_url", &json!("http://dav.example.com")).is_err());
        set_value_on(&mut settings, "cloud_url", &json!("https://dav.example.com/dav/")).unwrap();
        assert_eq!(settings["cloud_url"], json!("https://dav.example.com/dav"));
        assert!(set_value_on(&mut settings, "client_id", &json!("00000000402b5328")).is_err());
        set_value_on(&mut settings, "client_id", &json!("CE777973-8920-4993-8B21-4A06DAB23BA6")).unwrap();
    }

    #[test]
    fn groups_are_deduplicated_and_capped() {
        let value = json!(["Бета", " бета ", "Альфа", ""]);
        let groups = normalize_groups(&value);
        // Дедупликация чувствительна к регистру, сортировка — нет.
        assert_eq!(groups, json!(["Альфа", "Бета", "бета"]));
        let pinned = normalize_pinned_groups(&json!(["Альфа", "Бета", "Альфа", "Чужая"]), Some(&groups));
        assert_eq!(pinned, json!(["Альфа", "Бета"]));
    }
}
