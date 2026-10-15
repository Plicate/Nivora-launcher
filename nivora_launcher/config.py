from __future__ import annotations

import copy
import json
import math
import os
from typing import Optional
import re
import stat
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .constants import APP_ID, APP_VERSION, DEFAULT_MSA_CLIENT_ID, FTB_MODPACK_API_URL, LEGACY_MSA_CLIENT_IDS, MODRINTH_API_URL, MSA_LOGIN_API_URL, TECHNIC_API_URL, VERSION_MANIFEST_URL
from .modpacks import DEFAULT_MODPACK_CATALOGS
from .errors import LauncherError
from .plugin_catalogs import DEFAULT_PLUGIN_CATALOGS, normalize_plugin_catalogs
from .platform_support import set_private_fd


DEFAULT_KEYBOARD_SHORTCUTS = {
    "open_settings": "Ctrl+,",
    "refresh": "Ctrl+R",
    "new_instance": "Ctrl+N",
    "focus_search": "Ctrl+K",
    "toggle_activity": "Ctrl+J",
}

DEFAULT_LAUNCHER_PROFILE = {
    "id": "default",
    "name": "По умолчанию",
    "memory": "auto",
    "java_path": "",
    "graphics_mode": "auto",
    "account_id": "auto",
    "window_width": 1280,
    "window_height": 720,
    "fullscreen": False,
    "quick_play_world": "",
    "quick_play_server": "",
    "jvm_arguments": [],
    "game_arguments": [],
    "jvm_preset": "none",
}

DEFAULT_ANIMATION_SETTINGS = {
    "page_style": "fade",
    "page_duration_ms": 140,
    "surface_duration_ms": 140,
    "surface_stagger_ms": 0,
    "page_styles": {
        "instances": "fade",
        "accounts": "fade",
        "mods": "fade",
        "modpacks": "fade",
        "plugins": "fade",
        "themes": "fade",
        "instance-settings": "fade",
        "settings": "fade",
    },
    "page_durations_ms": {
        "instances": 140,
        "accounts": 140,
        "mods": 140,
        "modpacks": 140,
        "plugins": 140,
        "themes": 140,
        "instance-settings": 140,
        "settings": 140,
    },
}

# Оверлей Nivora (Shift+Tab) и запись игры (F7). Горячие клавиши действуют,
# только пока запущен экземпляр: лаунчер регистрирует их на время сессии.
DEFAULT_OVERLAY_SETTINGS = {
    "enabled": True,
    "hotkey_overlay": "Shift+Tab",
    "hotkey_recording": "F7",
    "show_mods": True,
    "show_metrics": True,
    "recording_fps": 60,
    "recording_quality": 23,
    "recording_encoder": "auto",
    "recording_container": "mp4",
    "recording_audio": True,
    "position": "top-left",
    "scale": 100,
    "opacity": 96,
    "compact": False,
    "show_clock": True,
    "show_system_metrics": True,
    "metrics_cpu": True,
    "metrics_ram": True,
    "metrics_gpu": True,
    "metrics_disk": True,
    "metrics_temperatures": True,
    "system_metrics_display": "overlay",
    "show_chat": True,
}

DEFAULT_BACKUP_SCHEDULE = {
    "enabled": False,
    "interval_hours": 24,
    "keep": 5,
    "destination": "local",
}


def _xdg_home(name: str, fallback: Path) -> Path:
    value = os.environ.get(name)
    return Path(value).expanduser() if value else fallback


@dataclass
class Settings:
    game_dir: str = ""
    client_id: str = DEFAULT_MSA_CLIENT_ID
    java_path: str = ""
    memory: str = "auto"
    max_downloads: int = 8
    startup_view: str = "instances"
    last_view: str = "instances"
    remember_last_view: bool = True
    default_version_filter: str = "release"
    default_loader: str = "fabric"
    default_account_id: str = "auto"
    open_last_instance: bool = True
    launch_after_install: bool = False
    auto_refresh_metadata: bool = True
    metadata_refresh_hours: int = 6
    download_timeout_seconds: int = 30
    download_retries: int = 5
    confirm_destructive_actions: bool = True
    show_activity_panel: bool = True
    show_toasts: bool = True
    minimize_on_launch: bool = False
    close_to_tray: bool = False
    # Заставка при простое лаунчера: красивый фон вместо статичного окна.
    # Язык интерфейса: None = ещё не выбран (первый запуск спросит), дальше 'ru'|'en'.
    language: Optional[str] = None
    font_family: str = "system"
    keep_logs_days: int = 14
    telemetry: bool = False
    telemetry_endpoint: str = ""
    keyboard_shortcuts: dict[str, str] = field(default_factory=lambda: dict(DEFAULT_KEYBOARD_SHORTCUTS))
    graphics_mode: str = "auto"
    launcher_profiles: list[dict[str, Any]] = field(default_factory=lambda: copy.deepcopy([DEFAULT_LAUNCHER_PROFILE]))
    active_launcher_profile_id: str = "default"
    http_proxy: str = ""
    offline_debug_limit_seconds: int = 0
    launcher_version: str = APP_VERSION
    theme: str = "dark"
    active_theme_id: str = "midnight-signal"
    wallpaper_path: str = ""
    wallpaper_blur: int = 12
    wallpaper_opacity: int = 34
    accent_color: str = "#8b653f"
    sidebar_position: str = "left"
    sidebar_mode: str = "adaptive"
    sidebar_width_mode: str = "content"
    sidebar_width: int = 228
    sidebar_floating: bool = False
    sidebar_visibility: str = "always"
    animations: str = "full"
    animation_settings: dict[str, Any] = field(default_factory=lambda: copy.deepcopy(DEFAULT_ANIMATION_SETTINGS))
    overlay_settings: dict[str, Any] = field(default_factory=lambda: copy.deepcopy(DEFAULT_OVERLAY_SETTINGS))
    backup_schedule: dict[str, Any] = field(default_factory=lambda: copy.deepcopy(DEFAULT_BACKUP_SCHEDULE))
    density: str = "comfortable"
    ui_scale: int = 100
    radius_tokens: dict[str, int] = field(default_factory=lambda: dict(DEFAULT_RADIUS_TOKENS))
    page_themes: dict[str, str] = field(
        default_factory=lambda: {"instances": "inherit", "instance-settings": "inherit", "accounts": "inherit", "mods": "inherit", "modpacks": "inherit", "plugins": "inherit", "themes": "inherit", "settings": "inherit"}
    )
    page_layouts: dict[str, str] = field(
        default_factory=lambda: {"instances": "balanced", "instance-settings": "balanced", "accounts": "balanced", "mods": "wide", "modpacks": "wide", "plugins": "wide", "themes": "wide", "settings": "balanced"}
    )
    widget_visibility: dict[str, bool] = field(
        default_factory=lambda: {"instances.workspace": True, "accounts.status": True, "mods.context": True}
    )
    color_tokens: dict[str, str] = field(default_factory=lambda: dict(DEFAULT_COLOR_TOKENS))
    instance_groups: list[str] = field(default_factory=list)
    pinned_instance_groups: list[str] = field(default_factory=list)
    login_source: str = "microsoft"
    login_api_url: str = MSA_LOGIN_API_URL
    catalog_source: str = "modrinth"
    catalog_api_url: str = MODRINTH_API_URL
    data_source: str = "mojang"
    data_api_url: str = VERSION_MANIFEST_URL
    resource_catalogs: list[dict[str, str]] = field(default_factory=lambda: copy.deepcopy(DEFAULT_RESOURCE_CATALOGS))
    active_catalog_id: str = "modrinth"
    plugin_catalogs: list[dict[str, Any]] = field(default_factory=lambda: copy.deepcopy(DEFAULT_PLUGIN_CATALOGS))
    active_plugin_catalog_id: str = "modrinth-plugins"
    developer_mode: bool = False
    cloud_url: str = ""
    cloud_username: str = ""
    # Дефолт пути на WebDAV сохранён старым: у настроенных пользователей синхронизация
    # указывает в эту папку, смена дефолта «спрячет» от них уже загруженные бэкапы.
    cloud_remote_path: str = "trial-launcher"
    cloud_include_accounts: bool = False
    cloud_last_sync: float = 0.0


DEFAULT_COLOR_TOKENS = {
    "accent": "#8b653f",
    "accent_strong": "#c5a274",
    "background": "#191a17",
    "panel": "#20221e",
    "panel_raised": "#282a25",
    "panel_soft": "#30322c",
    "text": "#e9e8de",
    "muted": "#a2a395",
    "quiet": "#c7c7bb",
    "border": "#77796b",
    "danger": "#d58a7d",
    "success": "#9bac78",
    "warning": "#d2ad72",
    "banner_shade": "#191a17",
}

# Скругления углов основных групп элементов интерфейса, в пикселях.
DEFAULT_RADIUS_TOKENS = {
    "card": 7,
    "button": 8,
    "control": 10,
}


_MEMORY_RE = re.compile(r"^\s*(\d+(?:\.\d+)?)\s*([kmgt]i?b?|b)?\s*$", re.IGNORECASE)
_MEMORY_FACTORS = {
    "b": 1 / 1024,
    "k": 1,
    "kb": 1,
    "kib": 1,
    "m": 1024,
    "mb": 1024,
    "mib": 1024,
    "g": 1024 * 1024,
    "gb": 1024 * 1024,
    "gib": 1024 * 1024,
    "t": 1024 * 1024 * 1024,
    "tb": 1024 * 1024 * 1024,
    "tib": 1024 * 1024 * 1024,
}


DEFAULT_RESOURCE_CATALOGS = [dict(item) for item in DEFAULT_MODPACK_CATALOGS]


def system_memory_kib() -> int:
    if os.name == "nt":
        try:
            import psutil

            return int(psutil.virtual_memory().total) // 1024
        except (ImportError, OSError, AttributeError, ValueError):
            return 0
    try:
        pages = int(os.sysconf("SC_PHYS_PAGES"))
        page_size = int(os.sysconf("SC_PAGE_SIZE"))
        if pages > 0 and page_size > 0:
            return pages * page_size // 1024
    except (AttributeError, OSError, ValueError):
        pass
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        pass
    return 0


def recommended_memory_kib(total_kib: int | None = None) -> int:
    total = total_kib or system_memory_kib()
    if total <= 0:
        return 2 * 1024 * 1024
    return max(1024 * 1024, (total // 2 // 1024) * 1024)


def parse_memory_kib(value: str, default_kib: int | None = None) -> int:
    raw = str(value).strip()
    if raw.lower() in {"", "auto", "automatic"}:
        return default_kib or recommended_memory_kib()
    match = _MEMORY_RE.fullmatch(raw)
    if not match:
        raise LauncherError("Память JVM должна быть числом с единицей KiB, MiB или GiB")
    number = float(match.group(1))
    if not math.isfinite(number) or number <= 0:
        raise LauncherError("Память JVM должна быть больше нуля")
    unit = (match.group(2) or "m").lower()
    kib = int(number * _MEMORY_FACTORS[unit])
    if kib < 1:
        raise LauncherError("Значение памяти JVM слишком маленькое")
    return kib


def format_memory_kib(value: int) -> str:
    kib = max(1, int(value))
    if kib % (1024 * 1024) == 0:
        return f"{kib // (1024 * 1024)}GiB"
    if kib % 1024 == 0:
        return f"{kib // 1024}MiB"
    return f"{kib}KiB"


def java_memory_value(value: str) -> str:
    kib = parse_memory_kib(value)
    if kib % (1024 * 1024) == 0:
        return f"{kib // (1024 * 1024)}G"
    if kib % 1024 == 0:
        return f"{kib // 1024}M"
    return f"{kib}K"


class Paths:
    def __init__(self) -> None:
        if os.name == "nt":
            roaming = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
            local = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local"))
            self.config_home = _xdg_home("XDG_CONFIG_HOME", roaming) / APP_ID
            self.data_home = _xdg_home("XDG_DATA_HOME", local) / APP_ID
            self.cache_home = _xdg_home("XDG_CACHE_HOME", local / "Cache") / APP_ID
        else:
            self.config_home = _xdg_home("XDG_CONFIG_HOME", Path.home() / ".config") / APP_ID
            self.data_home = _xdg_home("XDG_DATA_HOME", Path.home() / ".local" / "share") / APP_ID
            self.cache_home = _xdg_home("XDG_CACHE_HOME", Path.home() / ".cache") / APP_ID
        self.settings_file = self.config_home / "settings.json"
        self.accounts_file = self.config_home / "accounts.json"
        self.instance_security_dir = self.config_home / "instance-security"
        self.logs_dir = self.data_home / "logs"
        self.instances_dir = self.data_home / "instances"
        self.runtimes_dir = self.data_home / "runtimes"
        self.download_cache = self.cache_home / "downloads"

    def ensure(self) -> None:
        for path in (
            self.config_home,
            self.instance_security_dir,
            self.data_home,
            self.cache_home,
            self.logs_dir,
            self.instances_dir,
            self.runtimes_dir,
            self.download_cache,
        ):
            path.mkdir(mode=0o700, parents=True, exist_ok=True)
            try:
                path.chmod(0o700)
            except OSError:
                pass


class ConfigStore:
    def __init__(self, paths: Paths | None = None) -> None:
        self.paths = paths or Paths()
        self.paths.ensure()
        self.settings = self._load_settings()

    def _load_settings(self) -> Settings:
        if not self.paths.settings_file.exists():
            settings = Settings()
            settings.game_dir = str(self.data_home_game_dir())
            settings.memory = format_memory_kib(recommended_memory_kib())
            settings.client_id = _configured_client_id()
            self.save_settings(settings)
            return settings
        try:
            if self.paths.settings_file.is_symlink() or self.paths.settings_file.stat().st_size > 1 * 1024 * 1024:
                raise LauncherError("Файл настроек слишком большой или является ссылкой")
            raw = json.loads(self.paths.settings_file.read_text(encoding="utf-8"))
            if not isinstance(raw, dict):
                raise LauncherError("Файл настроек имеет неожиданный формат")
            fields = {key: raw[key] for key in asdict(Settings()) if key in raw}
            settings = Settings(**fields)
        except (OSError, ValueError, TypeError) as exc:
            raise LauncherError(f"Не удалось прочитать настройки: {exc}") from exc
        if not settings.game_dir:
            settings.game_dir = str(self.data_home_game_dir())
        changed = False
        normalized_game_dir = str(self._validated_game_dir(settings.game_dir))
        if settings.game_dir != normalized_game_dir:
            settings.game_dir = normalized_game_dir
            changed = True
        raw_cloud_url = str(settings.cloud_url or "").strip().rstrip("/")
        try:
            cloud_url = urlsplit(raw_cloud_url)
            valid_cloud_url = not raw_cloud_url or (
                cloud_url.scheme == "https"
                and bool(cloud_url.hostname)
                and not cloud_url.username
                and not cloud_url.password
            )
        except ValueError:
            valid_cloud_url = False
        normalized_cloud_url = raw_cloud_url if valid_cloud_url else ""
        if settings.cloud_url != normalized_cloud_url:
            settings.cloud_url = normalized_cloud_url
            changed = True
        normalized_cloud_username = str(settings.cloud_username or "").strip()[:320]
        if settings.cloud_username != normalized_cloud_username:
            settings.cloud_username = normalized_cloud_username
            changed = True
        normalized_active_theme = str(settings.active_theme_id or "").strip()[:64]
        if settings.active_theme_id != normalized_active_theme:
            settings.active_theme_id = normalized_active_theme
            changed = True
        cloud_parts = [part for part in str(settings.cloud_remote_path or "trial-launcher").strip().replace("\\", "/").split("/") if part and part != "."]
        if any(part == ".." or "\x00" in part for part in cloud_parts):
            cloud_parts = []
        normalized_cloud_path = "/".join(cloud_parts)[:512] or "trial-launcher"
        if settings.cloud_remote_path != normalized_cloud_path:
            settings.cloud_remote_path = normalized_cloud_path
            changed = True
        normalized_cloud_accounts = False
        if settings.cloud_include_accounts != normalized_cloud_accounts:
            settings.cloud_include_accounts = normalized_cloud_accounts
            changed = True
        try:
            normalized_cloud_sync = max(0.0, float(settings.cloud_last_sync))
        except (TypeError, ValueError):
            normalized_cloud_sync = 0.0
        if settings.cloud_last_sync != normalized_cloud_sync:
            settings.cloud_last_sync = normalized_cloud_sync
            changed = True
        if settings.http_proxy:
            proxy = urlsplit(str(settings.http_proxy).strip())
            if proxy.scheme not in {"http", "https", "socks5", "socks5h"} or not proxy.hostname:
                settings.http_proxy = ""
                changed = True
        environment_client_id = _configured_client_id()
        if settings.client_id in LEGACY_MSA_CLIENT_IDS or (
            settings.client_id
            and not re.fullmatch(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", str(settings.client_id))
        ):
            settings.client_id = environment_client_id
            changed = True
        elif not settings.client_id and environment_client_id:
            settings.client_id = environment_client_id
            changed = True
        try:
            memory_kib = parse_memory_kib(settings.memory)
            total_kib = system_memory_kib()
            if total_kib and memory_kib > total_kib:
                memory_kib = total_kib
                changed = True
            normalized_memory = format_memory_kib(memory_kib)
            if settings.memory != normalized_memory:
                settings.memory = normalized_memory
                changed = True
        except LauncherError:
            settings.memory = format_memory_kib(recommended_memory_kib())
            changed = True
        if settings.offline_debug_limit_seconds != 0:
            settings.offline_debug_limit_seconds = 0
            changed = True
        normalized_developer_mode = _coerce_bool(settings.developer_mode, False)
        if settings.developer_mode != normalized_developer_mode:
            settings.developer_mode = normalized_developer_mode
            changed = True
        try:
            normalized_downloads = min(32, max(1, int(settings.max_downloads)))
        except (TypeError, ValueError):
            normalized_downloads = 8
        if settings.max_downloads != normalized_downloads:
            settings.max_downloads = normalized_downloads
            changed = True
        normalized_behavior = _normalize_behavior(settings)
        if normalized_behavior != _behavior_values(settings):
            _apply_behavior(settings, normalized_behavior)
            changed = True
        normalized_appearance = _normalize_appearance(settings)
        if normalized_appearance != _appearance_values(settings):
            _apply_appearance(settings, normalized_appearance)
            changed = True
        normalized_sources = _normalize_sources(settings)
        if normalized_sources != _source_values(settings):
            for key, value in normalized_sources.items():
                setattr(settings, key, value)
            changed = True
        normalized_catalogs = _normalize_resource_catalogs(
            settings.resource_catalogs,
            settings.catalog_source,
            settings.catalog_api_url,
            use_legacy="resource_catalogs" not in raw,
        )
        normalized_active = _normalize_active_catalog_id(settings.active_catalog_id, normalized_catalogs)
        if normalized_catalogs != settings.resource_catalogs or normalized_active != settings.active_catalog_id:
            settings.resource_catalogs = normalized_catalogs
            settings.active_catalog_id = normalized_active
            changed = True
        active_catalog = next(item for item in normalized_catalogs if item["id"] == normalized_active)
        if settings.catalog_source != active_catalog["kind"] or settings.catalog_api_url != active_catalog["api_url"]:
            settings.catalog_source = active_catalog["kind"]
            settings.catalog_api_url = active_catalog["api_url"]
            changed = True
        normalized_groups = _normalize_groups(settings.instance_groups)
        if normalized_groups != settings.instance_groups:
            settings.instance_groups = normalized_groups
            changed = True
        normalized_pinned_groups = _normalize_pinned_groups(settings.pinned_instance_groups, normalized_groups)
        if normalized_pinned_groups != settings.pinned_instance_groups:
            settings.pinned_instance_groups = normalized_pinned_groups
            changed = True
        normalized_plugin_catalogs = normalize_plugin_catalogs(settings.plugin_catalogs)
        normalized_plugin_active = _normalize_active_catalog_id(settings.active_plugin_catalog_id, normalized_plugin_catalogs)
        if normalized_plugin_catalogs != settings.plugin_catalogs or normalized_plugin_active != settings.active_plugin_catalog_id:
            settings.plugin_catalogs = normalized_plugin_catalogs
            settings.active_plugin_catalog_id = normalized_plugin_active
            changed = True
        normalized_profiles = _normalize_launcher_profiles(settings.launcher_profiles)
        normalized_profile_active = _normalize_active_profile_id(settings.active_launcher_profile_id, normalized_profiles)
        if normalized_profiles != settings.launcher_profiles or normalized_profile_active != settings.active_launcher_profile_id:
            settings.launcher_profiles = normalized_profiles
            settings.active_launcher_profile_id = normalized_profile_active
            changed = True
        if changed:
            self.save_settings(settings)
        return settings

    def data_home_game_dir(self) -> Path:
        return self.paths.data_home / "game"

    def save_settings(self, settings: Settings | None = None) -> None:
        value = settings or self.settings
        _atomic_json_write(self.paths.settings_file, asdict(value), private=True)

    @property
    def game_dir(self) -> Path:
        path = self._validated_game_dir(self.settings.game_dir)
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        try:
            path.chmod(0o700)
        except OSError:
            pass
        return path

    def set_value(self, key: str, value: Any) -> None:
        self._set_value(key, value)
        self.save_settings()

    def set_values(self, values: dict[str, Any]) -> None:
        if not isinstance(values, dict):
            raise LauncherError("Настройки имеют неожиданный формат")
        original = self.settings
        self.settings = copy.deepcopy(original)
        try:
            for key, value in values.items():
                self._set_value(key, value, normalize_group_pins=False)
            self.settings.pinned_instance_groups = _normalize_pinned_groups(
                self.settings.pinned_instance_groups,
                self.settings.instance_groups,
            )
            self.save_settings()
        except Exception:
            self.settings = original
            raise

    def _set_value(self, key: str, value: Any, *, normalize_group_pins: bool = True) -> None:
        if not hasattr(self.settings, key):
            raise LauncherError(f"Неизвестная настройка: {key}")
        current = getattr(self.settings, key)
        if key == "client_id":
            client_id = str(value).strip()
            if client_id and (
                client_id.lower() in {item.lower() for item in LEGACY_MSA_CLIENT_IDS}
                or not re.fullmatch(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", client_id)
            ):
                raise LauncherError("Укажите client ID собственного Azure public-client приложения")
            value = client_id
        if key == "memory":
            memory_kib = parse_memory_kib(value)
            total_kib = system_memory_kib()
            if total_kib and memory_kib > total_kib:
                raise LauncherError(f"Память JVM не может превышать доступные {format_memory_kib(total_kib)}")
            value = format_memory_kib(memory_kib)
        if key == "game_dir":
            value = str(self._validated_game_dir(value))
        if key == "max_downloads":
            try:
                value = min(32, max(1, int(value)))
            except (TypeError, ValueError) as exc:
                raise LauncherError("max_downloads должно быть числом от 1 до 32") from exc
        if key == "http_proxy" and value.strip():
            proxy = urlsplit(value.strip())
            if proxy.scheme not in {"http", "https", "socks5", "socks5h"} or not proxy.hostname:
                raise LauncherError("Прокси должен иметь вид http://host:port, https://host:port или socks5://host:port")
        if key == "cloud_url":
            raw_url = str(value).strip().rstrip("/")
            if raw_url:
                cloud_url = urlsplit(raw_url)
                if cloud_url.scheme != "https" or not cloud_url.hostname or cloud_url.username or cloud_url.password:
                    raise LauncherError("WebDAV endpoint должен иметь вид https://host/remote.php/dav/files/user")
                value = raw_url
            else:
                value = ""
        if key == "active_theme_id":
            value = str(value).strip()[:64]
        if key == "cloud_username":
            value = str(value).strip()[:320]
        if key == "cloud_remote_path":
            parts = [part for part in str(value or "trial-launcher").strip().replace("\\", "/").split("/") if part and part != "."]
            if any(part == ".." or "\x00" in part for part in parts):
                raise LauncherError("Путь облачной папки содержит недопустимые сегменты")
            value = "/".join(parts)[:512] or "trial-launcher"
        if key == "cloud_include_accounts":
            value = False
        if key == "accent_color":
            value = str(value).strip().lower()
            if not re.fullmatch(r"#[0-9a-f]{6}", value):
                raise LauncherError("Акцентный цвет должен быть в формате #rrggbb")
        if key in {"font_family", "theme", "sidebar_position", "sidebar_mode", "sidebar_width_mode", "sidebar_visibility", "animations", "density", "login_source", "catalog_source", "data_source", "startup_view", "last_view", "default_version_filter", "default_loader", "default_account_id"}:
            value = str(value).strip().lower()
        if key == "graphics_mode":
            value = str(value).strip().lower()
            if value not in {"auto", "nvidia", "mesa", "software"}:
                raise LauncherError(f"Неизвестный graphics mode: {value}")
        if key == "launcher_profiles":
            value = _normalize_launcher_profiles(value)
        if key == "active_launcher_profile_id":
            value = str(value).strip().lower()
        if key == "animation_settings":
            value = _normalize_animation_settings(value)
        if key == "overlay_settings":
            value = _normalize_overlay_settings(value)
        if key == "backup_schedule":
            value = _normalize_backup_schedule(value)
        if key == "wallpaper_path":
            value = str(value).strip()[:2048]
        if key == "ui_scale":
            try:
                value = min(125, max(80, int(value)))
            except (TypeError, ValueError) as exc:
                raise LauncherError("Масштаб интерфейса должен быть числом от 80 до 125") from exc
        if key in {"login_api_url", "catalog_api_url", "data_api_url"}:
            fallback = MSA_LOGIN_API_URL if key == "login_api_url" else MODRINTH_API_URL if key == "catalog_api_url" else VERSION_MANIFEST_URL
            restricted_hosts = {"login.microsoftonline.com"} if key == "login_api_url" else {"piston-meta.mojang.com"} if key == "data_api_url" else None
            value = _normalize_api_url(str(value), fallback, allowed_hosts=restricted_hosts, developer_mode=self.settings.developer_mode)
        if key == "instance_groups":
            value = _normalize_groups(value)
        if key == "pinned_instance_groups":
            value = _normalize_pinned_groups(value)
        if key == "resource_catalogs":
            value = _normalize_resource_catalogs(value, self.settings.catalog_source, self.settings.catalog_api_url)
        if key == "plugin_catalogs":
            value = normalize_plugin_catalogs(value)
        if key == "active_catalog_id":
            value = str(value).strip().lower()
        if key == "active_plugin_catalog_id":
            value = str(value).strip().lower()
        if key == "color_tokens":
            value = _normalize_color_tokens({**self.settings.color_tokens, **value} if isinstance(value, dict) else self.settings.color_tokens)
        if key == "radius_tokens":
            value = _normalize_radius_tokens({**self.settings.radius_tokens, **(value if isinstance(value, dict) else {})})
        if key in {"page_themes", "page_layouts", "widget_visibility"}:
            if not isinstance(value, dict):
                raise LauncherError(f"{key} должен быть объектом")
            if key == "widget_visibility":
                value = {str(item_key): _coerce_bool(item_value, True) for item_key, item_value in value.items()}
            else:
                value = {str(item_key): str(item_value) for item_key, item_value in value.items()}
        if key == "keyboard_shortcuts":
            value = _normalize_shortcuts(value)
        # Язык интерфейса: null (не выбран), 'ru' или 'en'.
        if key == "language":
            if value is None:
                pass
            elif str(value).strip().lower() not in ("ru", "en"):
                raise LauncherError("Неизвестный язык интерфейса")
            else:
                value = str(value).strip().lower()
        if isinstance(current, bool):
            value = _coerce_bool(value, current)
        elif isinstance(current, int):
            try:
                value = int(value)
            except ValueError as exc:
                raise LauncherError(f"Настройка {key} должна быть числом") from exc
        elif isinstance(current, float):
            try:
                value = float(value)
            except (TypeError, ValueError) as exc:
                raise LauncherError(f"Настройка {key} должна быть числом") from exc
        setattr(self.settings, key, value)
        if normalize_group_pins and key in {"instance_groups", "pinned_instance_groups"}:
            self.settings.pinned_instance_groups = _normalize_pinned_groups(
                self.settings.pinned_instance_groups,
                self.settings.instance_groups,
            )
        if key in {
            "startup_view",
            "last_view",
            "remember_last_view",
            "default_version_filter",
            "default_loader",
            "default_account_id",
            "open_last_instance",
            "launch_after_install",
            "auto_refresh_metadata",
            "metadata_refresh_hours",
            "download_timeout_seconds",
            "download_retries",
            "confirm_destructive_actions",
            "show_activity_panel",
            "show_toasts",
            "minimize_on_launch",
            "close_to_tray",
            "font_family",
            "keep_logs_days",
            "telemetry",
            "telemetry_endpoint",
            "keyboard_shortcuts",
        }:
            _apply_behavior(self.settings, _normalize_behavior(self.settings))
        if key == "accent_color":
            self.settings.color_tokens = {**_normalize_color_tokens(self.settings.color_tokens), "accent": value}
        elif key == "color_tokens":
            self.settings.accent_color = value["accent"]
        if key in {
            "theme",
            "wallpaper_path",
            "wallpaper_blur",
            "wallpaper_opacity",
            "accent_color",
            "sidebar_position",
            "sidebar_mode",
            "sidebar_width_mode",
            "sidebar_width",
            "sidebar_floating",
            "sidebar_visibility",
            "animations",
            "animation_settings",
            "density",
            "ui_scale",
            "radius_tokens",
            "page_themes",
            "page_layouts",
            "widget_visibility",
            "color_tokens",
        }:
            _apply_appearance(self.settings, _normalize_appearance(self.settings))
        if key in {"login_source", "login_api_url", "catalog_source", "catalog_api_url", "data_source", "data_api_url", "instance_groups", "developer_mode"}:
            normalized = _normalize_sources(self.settings)
            self.settings.login_source = normalized["login_source"]
            self.settings.login_api_url = normalized["login_api_url"]
            self.settings.catalog_source = normalized["catalog_source"]
            self.settings.catalog_api_url = normalized["catalog_api_url"]
            self.settings.data_source = normalized["data_source"]
            self.settings.data_api_url = normalized["data_api_url"]
            self.settings.instance_groups = _normalize_groups(self.settings.instance_groups)
        if key in {"resource_catalogs", "active_catalog_id"}:
            catalogs = _normalize_resource_catalogs(self.settings.resource_catalogs, self.settings.catalog_source, self.settings.catalog_api_url)
            active_id = _normalize_active_catalog_id(self.settings.active_catalog_id, catalogs)
            self.settings.resource_catalogs = catalogs
            self.settings.active_catalog_id = active_id
            active_catalog = next(item for item in catalogs if item["id"] == active_id)
            self.settings.catalog_source = active_catalog["kind"]
            self.settings.catalog_api_url = active_catalog["api_url"]
        if key in {"plugin_catalogs", "active_plugin_catalog_id"}:
            catalogs = normalize_plugin_catalogs(self.settings.plugin_catalogs)
            active_id = _normalize_active_catalog_id(self.settings.active_plugin_catalog_id, catalogs)
            self.settings.plugin_catalogs = catalogs
            self.settings.active_plugin_catalog_id = active_id
        if key in {"launcher_profiles", "active_launcher_profile_id"}:
            profiles = _normalize_launcher_profiles(self.settings.launcher_profiles)
            active_id = _normalize_active_profile_id(self.settings.active_launcher_profile_id, profiles)
            self.settings.launcher_profiles = profiles
            self.settings.active_launcher_profile_id = active_id

    def _validated_game_dir(self, value: str | Path) -> Path:
        raw = Path(value).expanduser()
        try:
            path = raw.resolve()
        except OSError as exc:
            raise LauncherError(f"Некорректная папка игры: {value}") from exc
        forbidden = {
            Path("/").resolve(),
            Path.home().resolve(),
            self.paths.config_home.resolve(),
            self.paths.data_home.resolve(),
        }
        if path in forbidden:
            raise LauncherError("Нельзя использовать корень системы или домашний каталог как папку игры")
        return path


def _appearance_values(settings: Settings) -> dict[str, Any]:
    return {
        "theme": settings.theme,
        "active_theme_id": settings.active_theme_id,
        "wallpaper_path": settings.wallpaper_path,
        "wallpaper_blur": settings.wallpaper_blur,
        "wallpaper_opacity": settings.wallpaper_opacity,
        "accent_color": settings.accent_color,
        "sidebar_position": settings.sidebar_position,
        "sidebar_mode": settings.sidebar_mode,
        "sidebar_width_mode": settings.sidebar_width_mode,
        "sidebar_width": settings.sidebar_width,
        "sidebar_floating": settings.sidebar_floating,
        "sidebar_visibility": settings.sidebar_visibility,
        "animations": settings.animations,
        "animation_settings": settings.animation_settings,
        "density": settings.density,
        "ui_scale": settings.ui_scale,
        "radius_tokens": settings.radius_tokens,
        "page_themes": settings.page_themes,
        "page_layouts": settings.page_layouts,
        "widget_visibility": settings.widget_visibility,
        "color_tokens": settings.color_tokens,
    }


def _behavior_values(settings: Settings) -> dict[str, Any]:
    return {
        "startup_view": settings.startup_view,
        "last_view": settings.last_view,
        "remember_last_view": settings.remember_last_view,
        "default_version_filter": settings.default_version_filter,
        "default_loader": settings.default_loader,
        "default_account_id": settings.default_account_id,
        "open_last_instance": settings.open_last_instance,
        "launch_after_install": settings.launch_after_install,
        "auto_refresh_metadata": settings.auto_refresh_metadata,
        "metadata_refresh_hours": settings.metadata_refresh_hours,
        "download_timeout_seconds": settings.download_timeout_seconds,
        "download_retries": settings.download_retries,
        "confirm_destructive_actions": settings.confirm_destructive_actions,
        "show_activity_panel": settings.show_activity_panel,
        "show_toasts": settings.show_toasts,
        "minimize_on_launch": settings.minimize_on_launch,
        "close_to_tray": settings.close_to_tray,
        "font_family": settings.font_family,
        "keep_logs_days": settings.keep_logs_days,
        "telemetry": settings.telemetry,
        "telemetry_endpoint": settings.telemetry_endpoint,
        "keyboard_shortcuts": settings.keyboard_shortcuts,
    }


def _apply_behavior(settings: Settings, values: dict[str, Any]) -> None:
    for key, value in values.items():
        setattr(settings, key, value)


def _normalize_shortcuts(value: Any) -> dict[str, str]:
    raw = value if isinstance(value, dict) else {}
    result: dict[str, str] = {}
    for key, default in DEFAULT_KEYBOARD_SHORTCUTS.items():
        candidate = str(raw.get(key, default)).strip()[:40]
        result[key] = candidate
    return result


def _normalize_behavior(settings: Settings) -> dict[str, Any]:
    views = {"instances", "accounts", "mods", "modpacks", "plugins", "themes", "settings"}
    filters = {"all", "release", "snapshot", "old_beta", "old_alpha"}
    loaders = {"vanilla", "fabric", "forge", "neoforge", "quilt"}
    startup_view = str(settings.startup_view).strip().lower()
    if startup_view not in views:
        startup_view = "instances"
    last_view = str(settings.last_view).strip().lower()
    if last_view not in views:
        last_view = startup_view
    version_filter = str(settings.default_version_filter).strip().lower()
    if version_filter not in filters:
        version_filter = "release"
    loader = str(settings.default_loader).strip().lower()
    if loader not in loaders:
        loader = "fabric"
    try:
        refresh_hours = min(168, max(1, int(settings.metadata_refresh_hours)))
    except (TypeError, ValueError):
        refresh_hours = 6
    try:
        timeout = min(120, max(5, int(settings.download_timeout_seconds)))
    except (TypeError, ValueError):
        timeout = 30
    try:
        retries = min(6, max(1, int(settings.download_retries)))
    except (TypeError, ValueError):
        retries = 5
    try:
        keep_logs = min(365, max(0, int(settings.keep_logs_days)))
    except (TypeError, ValueError):
        keep_logs = 14
    font_family = str(settings.font_family).strip().lower()
    if font_family not in {"system", "inter", "manrope", "mono"}:
        font_family = "system"
    telemetry_endpoint = str(settings.telemetry_endpoint).strip()[:400]
    if telemetry_endpoint:
        parsed = urlsplit(telemetry_endpoint)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            telemetry_endpoint = ""
    return {
        "startup_view": startup_view,
        "last_view": last_view,
        "remember_last_view": _coerce_bool(settings.remember_last_view, True),
        "default_version_filter": version_filter,
        "default_loader": loader,
        "default_account_id": str(settings.default_account_id).strip()[:200] or "auto",
        "open_last_instance": _coerce_bool(settings.open_last_instance, True),
        "launch_after_install": _coerce_bool(settings.launch_after_install, False),
        "auto_refresh_metadata": _coerce_bool(settings.auto_refresh_metadata, True),
        "metadata_refresh_hours": refresh_hours,
        "download_timeout_seconds": timeout,
        "download_retries": retries,
        "confirm_destructive_actions": _coerce_bool(settings.confirm_destructive_actions, True),
        "show_activity_panel": _coerce_bool(settings.show_activity_panel, True),
        "show_toasts": _coerce_bool(settings.show_toasts, True),
        "minimize_on_launch": _coerce_bool(settings.minimize_on_launch, False),
        "close_to_tray": _coerce_bool(settings.close_to_tray, False),
        "font_family": font_family,
        "keep_logs_days": keep_logs,
        "telemetry": _coerce_bool(settings.telemetry, False),
        "telemetry_endpoint": telemetry_endpoint,
        "keyboard_shortcuts": _normalize_shortcuts(settings.keyboard_shortcuts),
    }


def _apply_appearance(settings: Settings, values: dict[str, Any]) -> None:
    for key, value in values.items():
        setattr(settings, key, value)


def _normalize_appearance(settings: Settings) -> dict[str, Any]:
    pages = {"instances", "instance-settings", "accounts", "mods", "modpacks", "plugins", "themes", "settings"}
    valid_page_themes = {"inherit", "light", "dark", "glass"}
    valid_page_layouts = {"compact", "balanced", "wide"}
    raw_page_themes = settings.page_themes if isinstance(settings.page_themes, dict) else {}
    raw_page_layouts = settings.page_layouts if isinstance(settings.page_layouts, dict) else {}
    raw_widget_visibility = settings.widget_visibility if isinstance(settings.widget_visibility, dict) else {}
    page_themes = {
        key: str(raw_page_themes.get(key, "inherit")) if str(raw_page_themes.get(key, "inherit")) in valid_page_themes else "inherit"
        for key in pages
    }
    page_layouts = {
        key: str(raw_page_layouts.get(key, "balanced")) if str(raw_page_layouts.get(key, "balanced")) in valid_page_layouts else "balanced"
        for key in pages
    }
    widget_visibility = {
        "instances.workspace": _coerce_bool(raw_widget_visibility.get("instances.workspace", True), True),
        "accounts.status": _coerce_bool(raw_widget_visibility.get("accounts.status", True), True),
        "mods.context": _coerce_bool(raw_widget_visibility.get("mods.context", True), True),
    }
    color_tokens = _normalize_color_tokens(settings.color_tokens)
    active_theme_id = str(settings.active_theme_id or "midnight-signal").strip().lower()[:64] or "midnight-signal"
    theme_value = str(settings.theme).lower()
    theme = theme_value if theme_value in {"system", "light", "dark", "glass"} else "system"
    position_value = str(settings.sidebar_position).lower()
    sidebar_position = position_value if position_value in {"left", "right", "top", "bottom"} else "left"
    mode_value = str(settings.sidebar_mode).lower()
    sidebar_mode = mode_value if mode_value in {"expanded", "adaptive", "compact"} else "adaptive"
    width_mode_value = str(settings.sidebar_width_mode).lower()
    sidebar_width_mode = width_mode_value if width_mode_value in {"full", "content", "manual"} else "content"
    try:
        sidebar_width = min(360, max(168, int(settings.sidebar_width)))
    except (TypeError, ValueError):
        sidebar_width = 228
    sidebar_floating = _coerce_bool(settings.sidebar_floating, False)
    visibility_value = str(settings.sidebar_visibility).lower()
    sidebar_visibility = visibility_value if visibility_value in {"always", "hover"} else "always"
    animation_value = str(settings.animations).lower()
    animations = animation_value if animation_value in {"full", "reduced", "off"} else "full"
    animation_settings = _normalize_animation_settings(settings.animation_settings)
    density_value = str(settings.density).lower()
    density = density_value if density_value in {"compact", "comfortable", "spacious"} else "comfortable"
    try:
        ui_scale = min(125, max(80, int(settings.ui_scale)))
    except (TypeError, ValueError):
        ui_scale = 100
    accent = str(settings.accent_color).lower()
    if not re.fullmatch(r"#[0-9a-f]{6}", accent):
        accent = DEFAULT_COLOR_TOKENS["accent"]
    color_tokens["accent"] = accent
    try:
        blur = min(32, max(0, int(settings.wallpaper_blur)))
    except (TypeError, ValueError):
        blur = 12
    try:
        opacity = min(80, max(0, int(settings.wallpaper_opacity)))
    except (TypeError, ValueError):
        opacity = 34
    return {
        "theme": theme,
        "active_theme_id": active_theme_id,
        "wallpaper_path": str(settings.wallpaper_path or "")[:2048],
        "wallpaper_blur": blur,
        "wallpaper_opacity": opacity,
        "accent_color": accent,
        "sidebar_position": sidebar_position,
        "sidebar_mode": sidebar_mode,
        "sidebar_width_mode": sidebar_width_mode,
        "sidebar_width": sidebar_width,
        "sidebar_floating": sidebar_floating,
        "sidebar_visibility": sidebar_visibility,
        "animations": animations,
        "animation_settings": animation_settings,
        "density": density,
        "ui_scale": ui_scale,
        "radius_tokens": _normalize_radius_tokens(settings.radius_tokens),
        "page_themes": page_themes,
        "page_layouts": page_layouts,
        "widget_visibility": widget_visibility,
        "color_tokens": color_tokens,
    }


def _safe_preset_id(value: str) -> str:
    """Sanitise a jvm_preset value."""
    allowed = {"none", "fps", "balanced", "heavy_modpack", "compatibility", "streaming"}
    cleaned = str(value).strip().lower()
    return cleaned if cleaned in allowed else "none"


def _normalize_launcher_profiles(value: Any) -> list[dict[str, Any]]:
    raw_values = value if isinstance(value, list) else []
    result: list[dict[str, Any]] = []
    used: set[str] = set()
    for index, item in enumerate(raw_values[:24]):
        if not isinstance(item, dict):
            continue
        raw_id = re.sub(r"[^a-z0-9_-]+", "-", str(item.get("id", "")).strip().lower()).strip("-_")
        profile_id = (raw_id or ("default" if index == 0 else f"profile-{index + 1}"))[:48]
        base_id = profile_id
        suffix = 2
        while profile_id in used:
            profile_id = f"{base_id[:42]}-{suffix}"
            suffix += 1
        used.add(profile_id)
        memory = str(item.get("memory", "auto")).strip() or "auto"
        if memory.lower() not in {"auto", "automatic"}:
            try:
                memory = format_memory_kib(parse_memory_kib(memory))
            except LauncherError:
                memory = "auto"
        graphics_mode = str(item.get("graphics_mode", "auto")).strip().lower()
        if graphics_mode not in {"auto", "nvidia", "mesa", "software"}:
            graphics_mode = "auto"
        result.append(
            {
                "id": profile_id,
                "name": str(item.get("name", "Профиль запуска")).strip()[:80] or f"Профиль {index + 1}",
                "memory": memory,
                "java_path": str(item.get("java_path", "")).strip()[:2048],
                "graphics_mode": graphics_mode,
                "account_id": str(item.get("account_id", "auto")).strip()[:200] or "auto",
                "window_width": _bounded_int(item.get("window_width", 1280), 320, 8192, 1280),
                "window_height": _bounded_int(item.get("window_height", 720), 240, 8192, 720),
                "fullscreen": _coerce_bool(item.get("fullscreen", False), False),
                "quick_play_world": _safe_profile_text(item.get("quick_play_world")),
                "quick_play_server": _safe_profile_text(item.get("quick_play_server")),
                "jvm_arguments": _profile_arguments(item.get("jvm_arguments")),
                "game_arguments": _profile_arguments(item.get("game_arguments")),
                "jvm_preset": _safe_preset_id(str(item.get("jvm_preset", "none"))),
            }
        )
    if not result:
        result = [copy.deepcopy(DEFAULT_LAUNCHER_PROFILE)]
    return result


def _normalize_active_profile_id(value: Any, profiles: list[dict[str, Any]]) -> str:
    candidate = str(value or "").strip().lower()
    return candidate if any(item["id"] == candidate for item in profiles) else profiles[0]["id"]


def _normalize_animation_settings(value: Any) -> dict[str, Any]:
    raw = value if isinstance(value, dict) else {}
    styles = {"fade", "slide", "scale", "none"}
    pages = {"instances", "instance-settings", "accounts", "mods", "modpacks", "plugins", "themes", "settings"}
    page_styles_raw = raw.get("page_styles") if isinstance(raw.get("page_styles"), dict) else {}
    page_durations_raw = raw.get("page_durations_ms") if isinstance(raw.get("page_durations_ms"), dict) else {}
    fallback_style = str(raw.get("page_style", DEFAULT_ANIMATION_SETTINGS["page_style"])).strip().lower()
    if fallback_style not in styles:
        fallback_style = "slide"
    return {
        "page_style": fallback_style,
        "page_duration_ms": _bounded_int(raw.get("page_duration_ms", 220), 0, 1000, 220),
        "surface_duration_ms": _bounded_int(raw.get("surface_duration_ms", 300), 0, 1200, 300),
        "surface_stagger_ms": _bounded_int(raw.get("surface_stagger_ms", 35), 0, 200, 35),
        "page_styles": {
            page: str(page_styles_raw.get(page, fallback_style)).strip().lower()
            if str(page_styles_raw.get(page, fallback_style)).strip().lower() in styles else fallback_style
            for page in pages
        },
        "page_durations_ms": {
            page: _bounded_int(page_durations_raw.get(page, raw.get("page_duration_ms", 220)), 0, 1000, 220)
            for page in pages
        },
    }


def _normalize_overlay_settings(value: Any) -> dict[str, Any]:
    raw = value if isinstance(value, dict) else {}
    encoders = {"auto", "nvenc", "vaapi", "x264"}
    containers = {"mp4", "mkv"}
    positions = {"top-left", "top-right", "bottom-left", "bottom-right"}

    def hotkey(key: str, fallback: str) -> str:
        text = str(raw.get(key, fallback)).strip()[:32]
        allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+*")
        return text if text and set(text) <= allowed else fallback

    encoder = str(raw.get("recording_encoder", "auto")).strip().lower()
    container = str(raw.get("recording_container", "mp4")).strip().lower()
    position = str(raw.get("position", "top-left")).strip().lower()
    metrics_display = str(raw.get("system_metrics_display", "overlay")).strip().lower()
    return {
        "enabled": _coerce_bool(raw.get("enabled", True), True),
        "hotkey_overlay": hotkey("hotkey_overlay", "Shift+Tab"),
        "hotkey_recording": hotkey("hotkey_recording", "F7"),
        "show_mods": _coerce_bool(raw.get("show_mods", True), True),
        "show_metrics": _coerce_bool(raw.get("show_metrics", True), True),
        "recording_fps": _bounded_int(raw.get("recording_fps", 60), 15, 120, 60),
        "recording_quality": _bounded_int(raw.get("recording_quality", 23), 15, 35, 23),
        "recording_encoder": encoder if encoder in encoders else "auto",
        "recording_container": container if container in containers else "mp4",
        "recording_audio": _coerce_bool(raw.get("recording_audio", True), True),
        "position": position if position in positions else "top-left",
        "scale": _bounded_int(raw.get("scale", 100), 80, 130, 100),
        "opacity": _bounded_int(raw.get("opacity", 96), 60, 100, 96),
        "compact": _coerce_bool(raw.get("compact", False), False),
        "show_clock": _coerce_bool(raw.get("show_clock", True), True),
        "show_system_metrics": _coerce_bool(raw.get("show_system_metrics", True), True),
        "metrics_cpu": _coerce_bool(raw.get("metrics_cpu", True), True),
        "metrics_ram": _coerce_bool(raw.get("metrics_ram", True), True),
        "metrics_gpu": _coerce_bool(raw.get("metrics_gpu", True), True),
        "metrics_disk": _coerce_bool(raw.get("metrics_disk", True), True),
        "metrics_temperatures": _coerce_bool(raw.get("metrics_temperatures", True), True),
        "system_metrics_display": metrics_display if metrics_display in {"overlay", "always"} else "overlay",
        "show_chat": _coerce_bool(raw.get("show_chat", True), True),
    }


def _normalize_backup_schedule(value: Any) -> dict[str, Any]:
    raw = value if isinstance(value, dict) else {}
    destination = str(raw.get("destination", "local")).strip().lower()
    try:
        interval_hours = min(24 * 30, max(1, int(raw.get("interval_hours", 24))))
    except (TypeError, ValueError):
        interval_hours = 24
    try:
        keep = min(50, max(1, int(raw.get("keep", 5))))
    except (TypeError, ValueError):
        keep = 5
    return {
        "enabled": _coerce_bool(raw.get("enabled", False), False),
        "interval_hours": interval_hours,
        "keep": keep,
        "destination": destination if destination in {"local", "cloud"} else "local",
    }


def _bounded_int(value: Any, minimum: int, maximum: int, fallback: int) -> int:
    try:
        return min(maximum, max(minimum, int(value)))
    except (TypeError, ValueError):
        return fallback


def _safe_profile_text(value: Any) -> str:
    text = str(value or "").strip()
    if "\x00" in text or "\n" in text or "\r" in text:
        return ""
    return text[:512]


def _profile_arguments(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    result: list[str] = []
    for item in value[:32]:
        text = _safe_profile_text(item)
        if text:
            result.append(text)
    return result


def _appearance_source_values(settings: Settings) -> dict[str, Any]:
    return {
        "login_source": settings.login_source,
        "login_api_url": settings.login_api_url,
        "catalog_source": settings.catalog_source,
        "catalog_api_url": settings.catalog_api_url,
        "data_source": settings.data_source,
        "data_api_url": settings.data_api_url,
    }


def _source_values(settings: Settings) -> dict[str, Any]:
    return _appearance_source_values(settings)


def _normalize_sources(settings: Settings) -> dict[str, str]:
    login_source = str(settings.login_source).strip().lower()
    catalog_source = str(settings.catalog_source).strip().lower()
    data_source = str(settings.data_source).strip().lower()
    return {
        "login_source": login_source if login_source in {"microsoft", "disabled"} else "microsoft",
        "login_api_url": _normalize_api_url(
            settings.login_api_url,
            MSA_LOGIN_API_URL,
            allowed_hosts={"login.microsoftonline.com"},
            developer_mode=bool(settings.developer_mode),
        ),
        "catalog_source": catalog_source if catalog_source in {"modrinth", "custom"} else "modrinth",
        "catalog_api_url": _normalize_api_url(settings.catalog_api_url, MODRINTH_API_URL),
        "data_source": data_source if data_source in {"mojang", "custom"} else "mojang",
        "data_api_url": _normalize_api_url(
            settings.data_api_url,
            VERSION_MANIFEST_URL,
            allowed_hosts={"piston-meta.mojang.com"},
            developer_mode=bool(settings.developer_mode),
        ),
    }


def _normalize_color_tokens(value: Any) -> dict[str, str]:
    raw = value if isinstance(value, dict) else {}
    return {
        key: str(raw.get(key, fallback)).strip().lower()
        if re.fullmatch(r"#[0-9a-fA-F]{6}", str(raw.get(key, fallback)).strip())
        else fallback
        for key, fallback in DEFAULT_COLOR_TOKENS.items()
    }


def _normalize_radius_tokens(value: Any) -> dict[str, int]:
    raw = value if isinstance(value, dict) else {}
    result: dict[str, int] = {}
    for key, fallback in DEFAULT_RADIUS_TOKENS.items():
        try:
            number = int(raw.get(key, fallback))
        except (TypeError, ValueError):
            number = fallback
        result[key] = min(24, max(0, number))
    return result


def _normalize_groups(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    result: list[str] = []
    for item in value:
        name = str(item).strip()[:40]
        if name and name not in result:
            result.append(name)
    return sorted(result, key=str.casefold)[:100]


def _normalize_pinned_groups(value: Any, groups: list[str] | None = None) -> list[str]:
    if not isinstance(value, list):
        return []
    allowed = set(groups) if groups is not None else None
    result: list[str] = []
    for item in value:
        name = str(item).strip()[:40]
        if name and name not in result and (allowed is None or name in allowed):
            result.append(name)
        if len(result) == 3:
            break
    return result


def _normalize_api_url(
    value: Any,
    fallback: str,
    *,
    allowed_hosts: set[str] | None = None,
    developer_mode: bool = False,
) -> str:
    raw = str(value or "").strip()
    try:
        parsed = urlsplit(raw)
        port = parsed.port
    except ValueError:
        parsed = None
        port = None
    if (
        not parsed
        or parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or (port not in {None, 443})
        or (allowed_hosts and parsed.hostname.lower().rstrip(".") not in allowed_hosts and not developer_mode)
    ):
        return fallback
    return raw.rstrip("/")


def _normalize_resource_catalogs(
    value: Any,
    legacy_source: str = "modrinth",
    legacy_url: str = MODRINTH_API_URL,
    *,
    use_legacy: bool = False,
) -> list[dict[str, str]]:
    raw_values = value if isinstance(value, list) else []
    if use_legacy and (str(legacy_source).strip().lower() == "custom" or str(legacy_url).rstrip("/") != MODRINTH_API_URL):
        raw_values = [
            {
                "id": "custom",
                "name": "Свой каталог",
                "kind": "custom",
                "api_url": legacy_url,
            }
        ]
    result: list[dict[str, str]] = []
    used: set[str] = set()
    for index, item in enumerate(raw_values):
        if not isinstance(item, dict):
            continue
        name = str(item.get("name", "")).strip()[:60]
        kind = str(item.get("kind", "custom")).strip().lower()
        if kind not in {"modrinth", "ftb", "technic", "custom"}:
            kind = "custom"
        fallback = {
            "modrinth": MODRINTH_API_URL,
            "ftb": FTB_MODPACK_API_URL,
            "technic": TECHNIC_API_URL,
            "custom": MODRINTH_API_URL,
        }[kind]
        api_url = _normalize_api_url(item.get("api_url", fallback), fallback)
        raw_id = re.sub(r"[^a-z0-9_-]+", "-", str(item.get("id", "")).strip().lower()).strip("-_")
        catalog_id = raw_id[:32] or ("modrinth" if index == 0 and kind == "modrinth" else f"catalog-{index + 1}")
        base_id = catalog_id
        suffix = 2
        while catalog_id in used:
            catalog_id = f"{base_id[:28]}-{suffix}"
            suffix += 1
        used.add(catalog_id)
        result.append(
            {
                "id": catalog_id,
                "name": name or {"modrinth": "Modrinth", "ftb": "Feed The Beast", "technic": "Technic"}.get(kind, f"Каталог {index + 1}"),
                "kind": kind,
                "api_url": api_url,
            }
        )
    if not result:
        result = [dict(DEFAULT_RESOURCE_CATALOGS[0])]
    return result[:20]


def _normalize_active_catalog_id(value: Any, catalogs: list[dict[str, str]]) -> str:
    candidate = str(value or "").strip().lower()
    return candidate if any(item["id"] == candidate for item in catalogs) else catalogs[0]["id"]


def _coerce_bool(value: Any, fallback: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        normalized = value.strip().lower()
        if normalized in {"true", "1", "yes", "on"}:
            return True
        if normalized in {"false", "0", "no", "off"}:
            return False
    return fallback

def _configured_client_id() -> str:
    # NIVORA_LAUNCHER_CLIENT_ID — новое имя; старое оставлено как совместимый алиас.
    value = (
        os.environ.get("NIVORA_LAUNCHER_CLIENT_ID")
        or os.environ.get("TRIAL_LAUNCHER_CLIENT_ID")
        or DEFAULT_MSA_CLIENT_ID
    ).strip()
    if value.lower() in {item.lower() for item in LEGACY_MSA_CLIENT_IDS}:
        return ""
    return value if re.fullmatch(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", value) else ""


def _atomic_json_write(path: Path, value: Any, private: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp_path = Path(temp_name)
    try:
        if private:
            set_private_fd(fd, stat.S_IRUSR | stat.S_IWUSR)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        temp_path.replace(path)
    finally:
        try:
            temp_path.unlink()
        except FileNotFoundError:
            pass
