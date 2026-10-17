from __future__ import annotations

import base64
import ipaddress
import json
import os
import platform
import re
import shutil
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import uuid
from collections import deque
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlsplit

from .accounts import AccountStore, MicrosoftAuthenticator, OfflineDebugAccount, microsoft_login_status
from .bedrock import BedrockProfileStore
from .cloud_storage import (
    WebDavStorage,
    cloud_settings_public,
    delete_cloud_file,
    restore_cloud,
    save_cloud_password,
    sync_cloud,
)
from .config import ConfigStore, format_memory_kib, parse_memory_kib, recommended_memory_kib, system_memory_kib
from .constants import APP_VERSION, BUILD_STAMP, MAX_INSTANCE_LIVE_STATE_JSON_BYTES
from .errors import LauncherError
from .github_plugins import GitHubPluginService, SecretToolTokenProvider
from .installer import MinecraftInstaller
from .instance_files import InstanceFiles, MAX_INSTANCE_UPLOAD_BYTES
from .instance_workspace import InstanceWorkspace
from .instances import InstanceStore
from .launcher import GameLauncher
from .loaders import LoaderRepository
from .metadata import VersionRepository, safe_instance_name
from .metrics import ProcessMetricsTracker, metrics_for_instance, read_metrics
from .mod_audit import AuditError, ModAuditStore
from .mods import ModrinthProvider
from .modpacks import ModpackImporter, ModpackProvider
from .net import HttpClient
from .recorder import recorder as game_recorder
from .operations import CancellationToken, OperationCancelled
from .plugin_catalogs import PluginCatalog, normalize_plugin_catalogs, remove_installed_plugin
from .plugins import PLUGIN_MAX_ARCHIVE_BYTES, PluginStore
from .platform_support import open_path
from .process_guard import InstanceProcessLock
from .runtime import discover_java_paths, java_version, validate_java_path
from .security import sandbox_status
from .server import (
    SERVER_JAR,
    ServerFiles,
    ServerInstanceStore,
    ServerProcessManager,
    _server_is_running,
    core_options,
    download_server_core,
    server_core_versions,
    server_memory_limit_kib,
)
from .server_network import diagnose as diagnose_server_network, publisher as server_publisher, properties_info as server_network_properties, port_number, public_ipv4
from .server import _apply_server_port
from .skins import SkinStore, apply_cape, apply_skin, clear_cape, clear_skin, profile as fetch_skin_profile
from .snapshots import SnapshotStore

# Категории снапшотов Bedrock: контент игры в com.mojang называется иначе,
# чем каталоги Java-экземпляра (см. SnapshotStore.categories).
BEDROCK_SNAPSHOT_CATEGORIES = {
    "resourcepacks": "resource_packs",
    "addons": "behavior_packs",
    "worlds": "minecraftWorlds",
}
from .crash_reports import analyze_crash
from .auto_repair import RepairContext, apply_fixes, diagnose as diagnose_repairs, issue_kinds_public, normalize_muted, pick_loader_version, record_launch_result
from .settings_profiles import (
    apply_profile as apply_settings_profile,
    save_profile as save_settings_profile,
)
from .world_backups import WorldBackupStore
from .mod_dependencies import dependency_graph, removal_impact
from .modpack_export import export_instance_as_mrpack
from .optimizer import JVM_PRESETS, analyze_instance
from .themes import GitHubThemeCatalog, ThemeStore


_MAX_SESSION_LOG_LINES = 500
_SESSION_LOGS: dict[str, deque[str]] = {}
_SESSION_RUNNING: dict[str, bool] = {}
_SESSION_STARTED: dict[str, float] = {}
_INSTANCE_METRIC_TRACKERS: dict[str, ProcessMetricsTracker] = {}


def _session_start(instance_name: str) -> None:
    _SESSION_LOGS[instance_name] = deque(maxlen=_MAX_SESSION_LOG_LINES)
    _SESSION_RUNNING[instance_name] = True
    _SESSION_STARTED[instance_name] = time.time()


def _session_append(instance_name: str, line: str) -> None:
    _SESSION_LOGS.setdefault(instance_name, deque(maxlen=_MAX_SESSION_LOG_LINES)).append(line)


def _session_finish(instance_name: str) -> None:
    _SESSION_RUNNING[instance_name] = False


def _session_snapshot(instance_name: str) -> tuple[list[str], bool]:
    return list(_SESSION_LOGS.get(instance_name, ())), bool(_SESSION_RUNNING.get(instance_name, False))


def _account_public(account: Any) -> dict[str, Any]:
    return {
        "account_id": account.account_id,
        "kind": account.kind,
        "username": account.username,
        "alias": account.alias,
        "display_name": account.alias or account.username,
        "expires_at": account.expires_at,
        "limit_seconds": account.limit_seconds,
        "used_seconds": account.used_seconds,
        "is_default": account.is_default,
    }


def _instance_public(value: Any) -> dict[str, Any]:
    return {
        "name": value.name,
        "display_name": value.display_name or value.name,
        "description": value.description,
        "group": value.group,
        "icon": value.icon,
        "banner": value.banner,
        "played_seconds": value.played_seconds,
        "version_id": value.version_id,
        "minecraft_version": value.minecraft_version,
        "loader": value.loader,
        "loader_version": _instance_loader_version(value),
        "path": str(value.path),
        "kind": getattr(value, "kind", "client"),
        "game_edition": getattr(value, "game_edition", "java"),
        "bedrock_profile": getattr(value, "bedrock_profile", ""),
        "server_core": getattr(value, "server_core", ""),
        "server_core_version": getattr(value, "server_core_version", ""),
        "server_port": getattr(value, "server_port", 0),
        "server_eula": getattr(value, "server_eula", False),
        "running": bool(getattr(value, "running", False)),
        "notes": getattr(value, "notes", ""),
        "tags": list(getattr(value, "tags", []) or []),
    }


def _external_instance_public(value: Any) -> dict[str, Any]:
    return {
        "launcher": value.launcher,
        "display_name": value.display_name,
        "suggested_name": value.suggested_name,
        "source_path": str(value.source_path),
        "game_path": str(value.game_path),
        "version_id": value.version_id,
        "minecraft_version": value.minecraft_version,
        "loader": value.loader,
        "notes": value.notes,
    }


def _instance_metrics(
    info: Any,
    pid: int,
    started_at: float,
    running: bool,
    *,
    root: Path | None = None,
) -> dict[str, Any]:
    metrics_root = root or info.path
    if pid <= 0:
        _INSTANCE_METRIC_TRACKERS.pop(info.name, None)
        last = read_metrics(metrics_root).get("last", {})
        if isinstance(last, dict) and last:
            return {**last, "pid": 0, "running": False}
        return metrics_for_instance(metrics_root)
    tracker = _INSTANCE_METRIC_TRACKERS.get(info.name)
    if tracker is None or tracker.pid != pid or abs(tracker.started_at - started_at) > 0.5:
        tracker = ProcessMetricsTracker(metrics_root, pid, started_at=started_at or None)
        _INSTANCE_METRIC_TRACKERS[info.name] = tracker
    snapshot = tracker.sample(running=running)
    if not snapshot["running"] and not running:
        _INSTANCE_METRIC_TRACKERS.pop(info.name, None)
    return snapshot


# Кэш списка модов для оверлея: окно опрашивает состояние каждую секунду,
# а полный скан каталога модов нужен не чаще пары раз в минуту.

# Фоновый планировщик резервных копий — один поток на процесс, независимо от
# того, сколько раз дергается handle().
_BACKUP_SCHEDULER_LOCK = threading.Lock()
_BACKUP_SCHEDULER_STARTED = False


def _ensure_backup_scheduler() -> None:
    global _BACKUP_SCHEDULER_STARTED
    with _BACKUP_SCHEDULER_LOCK:
        if _BACKUP_SCHEDULER_STARTED:
            return
        _BACKUP_SCHEDULER_STARTED = True

    def worker() -> None:
        while True:
            time.sleep(60)
            try:
                scheduler_config = ConfigStore()
                scheduler_instances = InstanceStore(scheduler_config)
                WorldBackupStore(scheduler_config, scheduler_instances).run_due(
                    running_check=lambda name: scheduler_instances.get(name).running
                )
            except Exception:
                continue

    threading.Thread(target=worker, daemon=True, name="nivora-backup-scheduler").start()


def _settings_public(config: ConfigStore) -> dict[str, Any]:
    settings = config.settings
    memory_kib = parse_memory_kib(settings.memory, recommended_memory_kib())
    return {
        "game_dir": settings.game_dir,
        "java_path": settings.java_path,
        "memory": format_memory_kib(memory_kib),
        "memory_kib": memory_kib,
        "memory_total_kib": system_memory_kib(),
        "memory_recommended_kib": recommended_memory_kib(),
        "max_downloads": settings.max_downloads,
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
        "auto_repair_enabled": settings.auto_repair_enabled,
        "auto_repair_muted": list(settings.auto_repair_muted),
        "auto_repair_kinds": issue_kinds_public(),
        "show_activity_panel": settings.show_activity_panel,
        "show_toasts": settings.show_toasts,
        "minimize_on_launch": settings.minimize_on_launch,
        "close_to_tray": settings.close_to_tray,
        "language": settings.language,
        "font_family": settings.font_family,
        "keep_logs_days": settings.keep_logs_days,
        "telemetry": settings.telemetry,
        "keyboard_shortcuts": settings.keyboard_shortcuts,
        "graphics_mode": settings.graphics_mode,
        "launcher_profiles": settings.launcher_profiles,
        "active_launcher_profile_id": settings.active_launcher_profile_id,
        "http_proxy": settings.http_proxy,
        "download_source": settings.download_source,
        "has_client_id": bool(settings.client_id),
        "developer_mode": settings.developer_mode,
        "theme": settings.theme,
        "active_theme_id": settings.active_theme_id,
        "wallpaper_path": settings.wallpaper_path,
        "wallpaper_data": _wallpaper_data(config),
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
        "overlay_settings": settings.overlay_settings,
        "backup_schedule": settings.backup_schedule,
        "density": settings.density,
        "ui_scale": settings.ui_scale,
        "radius_tokens": settings.radius_tokens,
        "page_themes": settings.page_themes,
        "page_layouts": settings.page_layouts,
        "widget_visibility": settings.widget_visibility,
        "color_tokens": settings.color_tokens,
        "instance_groups": settings.instance_groups,
        "pinned_instance_groups": settings.pinned_instance_groups,
        "login_source": settings.login_source,
        "login_api_url": settings.login_api_url,
        "catalog_source": settings.catalog_source,
        "catalog_api_url": settings.catalog_api_url,
        "data_source": settings.data_source,
        "data_api_url": settings.data_api_url,
        "resource_catalogs": settings.resource_catalogs,
        "active_catalog_id": settings.active_catalog_id,
        "plugin_catalogs": normalize_plugin_catalogs(settings.plugin_catalogs),
        "active_plugin_catalog_id": settings.active_plugin_catalog_id,
        "cloud_url": settings.cloud_url,
        "cloud_username": settings.cloud_username,
        "cloud_remote_path": settings.cloud_remote_path,
        "cloud_include_accounts": settings.cloud_include_accounts,
        "cloud_last_sync": settings.cloud_last_sync,
        "cloud_configured": cloud_settings_public(config)["configured"],
    }


def _wallpaper_data(config: ConfigStore) -> str:
    if not config.settings.wallpaper_path:
        return ""
    root = (config.paths.data_home / "wallpapers").resolve()
    path = Path(config.settings.wallpaper_path).expanduser()
    if path.is_symlink() or not path.is_file():
        return ""
    try:
        if not path.resolve().is_relative_to(root) or path.stat().st_size > 16 * 1024 * 1024:
            return ""
        content = path.read_bytes()
    except OSError:
        return ""
    mime = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp"}.get(path.suffix.lower())
    return f"data:{mime};base64,{base64.b64encode(content).decode('ascii')}" if mime else ""


def _resource_catalog(config: ConfigStore, catalog_id: str = "") -> dict[str, str]:
    requested = catalog_id.strip().lower()
    catalogs = config.settings.resource_catalogs
    selected = next((item for item in catalogs if item.get("id") == requested), None) if requested else None
    selected = selected or next((item for item in catalogs if item.get("id") == config.settings.active_catalog_id), None)
    if not selected:
        raise LauncherError("Каталог ресурсов не настроен")
    return selected


def _plugin_catalog(config: ConfigStore, catalog_id: str = "") -> dict[str, Any]:
    catalogs = normalize_plugin_catalogs(config.settings.plugin_catalogs)
    requested = catalog_id.strip().lower()
    selected = next((item for item in catalogs if item.get("id") == requested), None) if requested else None
    selected = selected or next((item for item in catalogs if item.get("id") == config.settings.active_plugin_catalog_id), None)
    selected = selected or next((item for item in catalogs if item.get("enabled", True)), None)
    if not selected:
        raise LauncherError("Каталог плагинов не настроен")
    if not selected.get("enabled", True):
        raise LauncherError("Выбранный каталог плагинов выключен")
    return selected


def _server_plugin_catalog(config: ConfigStore, catalog_id: str = "") -> dict[str, Any]:
    return _plugin_catalog(config, catalog_id)


def _sort_project_results(values: list[dict[str, Any]], sort_by: str) -> list[dict[str, Any]]:
    mode = str(sort_by or "relevance").strip().lower()
    if mode not in {"relevance", "downloads", "newest", "updated"}:
        raise LauncherError(f"Неизвестный порядок сортировки: {mode}")
    if mode == "relevance":
        return values

    def value_for(item: dict[str, Any]) -> float | int | None:
        if mode == "downloads":
            try:
                return max(0, int(item.get("downloads", 0)))
            except (TypeError, ValueError, OverflowError):
                return None
        fields = ("date_created", "created_at", "createdAt", "published_at") if mode == "newest" else ("date_modified", "updated_at", "updatedAt", "published_at")
        for key in fields:
            value = str(item.get(key) or "").strip()
            if not value:
                continue
            try:
                parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
                if parsed.tzinfo is None:
                    parsed = parsed.replace(tzinfo=timezone.utc)
                return parsed.timestamp()
            except (OverflowError, OSError, ValueError):
                continue
        return None

    indexed = list(enumerate(values))
    indexed.sort(
        key=lambda pair: (
            value_for(pair[1]) is None,
            -(value_for(pair[1]) or 0),
            str(pair[1].get("project_id") or pair[1].get("id") or "").casefold(),
            pair[0],
        )
    )
    return [item for _, item in indexed]


def _valid_wallpaper_bytes(content: bytes, suffix: str) -> bool:
    if suffix == ".png":
        return content.startswith(b"\x89PNG\r\n\x1a\n")
    if suffix in {".jpg", ".jpeg"}:
        return content.startswith(b"\xff\xd8\xff")
    return len(content) >= 12 and content[:4] == b"RIFF" and content[8:12] == b"WEBP"


def _decode_base64(value: Any, limit: int, label: str) -> bytes:
    encoded = str(value or "")
    if len(encoded) > 4 * ((limit + 2) // 3):
        raise LauncherError(f"{label} слишком большой")
    try:
        content = base64.b64decode(encoded, validate=True)
    except (ValueError, TypeError) as exc:
        raise LauncherError(f"{label} повреждён") from exc
    if len(content) > limit:
        raise LauncherError(f"{label} слишком большой")
    return content


def _context(
    emit: Callable[[dict[str, Any]], None] | None = None,
    cancellation: CancellationToken | None = None,
) -> tuple[ConfigStore, HttpClient, AccountStore, VersionRepository, MinecraftInstaller, GameLauncher, InstanceStore]:
    config = ConfigStore()
    _prune_logs(config)

    def observe(event: str, url: str, destination: Path, current: int, total: int | None) -> None:
        if emit:
            emit(
                {
                    "kind": "download",
                    "event": event,
                    "url": url,
                    "destination": destination.name,
                    "current": current,
                    "total": total,
                }
            )

    def observe_request(event: str, url: str, attempt: int) -> None:
        if not emit:
            return
        from urllib.parse import urlsplit

        parsed = urlsplit(url)
        host = parsed.netloc or parsed.path.split("/", 1)[0]
        endpoint = parsed.path.rsplit("/", 1)[-1] or "/"
        if event == "retry":
            message = f"Сеть: повторяю {host}{endpoint} (попытка {attempt + 1})"
        elif event == "mirror":
            message = f"Сеть: загружаю через зеркало {host}"
        elif event == "download":
            message = f"Загрузка: {host}{endpoint} (попытка {attempt})"
        else:
            message = f"Сеть: подключаюсь к {host}{endpoint} (попытка {attempt})"
        emit({"kind": "status", "message": message})

    http = HttpClient(
        timeout=config.settings.download_timeout_seconds,
        retries=config.settings.download_retries,
        download_observer=observe if emit else None,
        request_observer=observe_request if emit else None,
        proxy=config.settings.http_proxy,
        cancellation=cancellation,
        download_source=config.settings.download_source,
    )
    accounts = AccountStore(config)
    repository = VersionRepository(config, http)
    installer = MinecraftInstaller(config, repository=repository, http=http)
    launcher = GameLauncher(config, installer, accounts, repository, cancellation=cancellation)
    return config, http, accounts, repository, installer, launcher, InstanceStore(config)


def _prune_logs(config: ConfigStore) -> None:
    days = int(config.settings.keep_logs_days)
    if days <= 0:
        return
    root = config.paths.logs_dir
    if root.is_symlink() or not root.is_dir():
        return
    cutoff = time.time() - days * 86400
    for path in root.glob("*.log"):
        try:
            if not path.is_symlink() and path.is_file() and path.stat().st_mtime < cutoff:
                path.unlink()
        except OSError:
            continue


def _emit_telemetry_report(
    config: ConfigStore,
    *,
    event: str,
    instance: Any | None,
    version_id: str,
    exit_code: int | None,
    message: str,
    logs: list[str],
) -> None:
    endpoint = str(config.settings.telemetry_endpoint or "").strip()
    if not config.settings.telemetry or not endpoint:
        return
    payload = {
        "event": event,
        "app": {"name": "nivora-launcher", "version": APP_VERSION},
        "timestamp": time.time(),
        "system": {
            "platform": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
            "graphics_mode": str(config.settings.graphics_mode or "auto"),
        },
        "instance": {
            "name": str(getattr(instance, "name", "") or ""),
            "kind": str(getattr(instance, "kind", "client") or "client"),
            "loader": str(getattr(instance, "loader", "vanilla") or "vanilla"),
            "minecraft_version": str(getattr(instance, "minecraft_version", "") or ""),
            "version_id": version_id or str(getattr(instance, "version_id", "") or ""),
        },
        "error": {
            "message": str(message or "")[:600],
            "exit_code": exit_code,
            "log_tail": [str(line)[:500] for line in logs[-120:]],
        },
    }
    try:
        HttpClient(timeout=10, retries=1, proxy=config.settings.http_proxy).post_json(endpoint, payload)
    except Exception:
        return


def _bootstrap(config: ConfigStore, accounts: AccountStore, instances: InstanceStore, plugin_store: PluginStore) -> dict[str, Any]:
    return {
        # Фронтенд сверяет метку со своей: несовпадение означает, что
        # под столом лежит старая версия бэкенда.
        "build_stamp": BUILD_STAMP,
        "instances": [_instance_public(value) for value in instances.list()],
        "accounts": [_account_public(value) for value in accounts.all()],
        "plugins": plugin_store.list(),
        "server_cores": core_options(),
        "plugin_catalogs": normalize_plugin_catalogs(config.settings.plugin_catalogs),
        "active_plugin_catalog_id": config.settings.active_plugin_catalog_id,
        "settings": _settings_public(config),
        "system": {
            "platform": platform.system(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
            "session": os.environ.get("XDG_SESSION_TYPE", "unknown"),
            # Реальный графический бэкенд приложения: пакетные врапперы
            # форсируют GDK_BACKEND=x11 (XWayland), и тогда глобальные
            # горячие клавиши и «поверх всех» у окна оверлея РАБОТАЮТ,
            # даже если сессия номинально Wayland. Решать «wayland-режим»
            # только по XDG_SESSION_TYPE — ошибка (оверлей лишался клавиш).
            "gdk_backend": os.environ.get("GDK_BACKEND", ""),
            "display": os.environ.get("WAYLAND_DISPLAY") or os.environ.get("DISPLAY") or "none",
            "nvidia_smi": bool(shutil.which("nvidia-smi")),
            "glxinfo": bool(shutil.which("glxinfo")),
            "security": sandbox_status(),
        },
    }


def _graphics_diagnostics(config: ConfigStore, launcher: GameLauncher) -> dict[str, Any]:
    def probe(command: list[str]) -> dict[str, Any]:
        executable = shutil.which(command[0])
        if not executable:
            return {"available": False, "output": "Не найдено"}
        try:
            result = subprocess.run(
                [executable, *command[1:]],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
                env=environment,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            return {"available": True, "output": str(exc)}
        output = (result.stdout + "\n" + result.stderr).strip()
        return {"available": True, "exit_code": result.returncode, "output": output[-4000:]}

    environment = launcher._launch_environment()
    return {
        "mode": config.settings.graphics_mode,
        "effective_mode": environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE", "auto"),
        "session": os.environ.get("XDG_SESSION_TYPE", "unknown"),
        "display": os.environ.get("WAYLAND_DISPLAY") or os.environ.get("DISPLAY") or "none",
        "environment": {
            key: environment.get(key, "")
            for key in (
                "TRIAL_LAUNCHER_GRAPHICS_MODE",
                "__GLX_VENDOR_LIBRARY_NAME",
                "__NV_PRIME_RENDER_OFFLOAD",
                "__VK_LAYER_NV_optimus",
                "__GL_THREADED_OPTIMIZATIONS",
                "LIBGL_ALWAYS_SOFTWARE",
                "GALLIUM_DRIVER",
            )
        },
        "glxinfo": probe(["glxinfo", "-B"]),
        "nvidia_smi": probe(["nvidia-smi", "--query-gpu=name,driver_version", "--format=csv,noheader"]),
    }


def _mod_audit_target(instances: InstanceStore, request: dict[str, Any]) -> tuple[Path, str, str]:
    """Resolve the JAR file to audit: (path, label, kind)."""
    raw_path = str(request.get("path", "")).strip()
    instance_name = str(request.get("instance_name", "")).strip()
    if not raw_path:
        raise LauncherError("Укажите файл мода или плагина для проверки")
    if instance_name:
        info = instances.get(instance_name)
        is_server = str(getattr(info, "kind", "client")).strip().lower() == "server"
        parts = [part for part in raw_path.replace("\\", "/").split("/") if part not in ("", ".", "..")]
        if not parts or raw_path.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:", raw_path):
            raise LauncherError("Путь должен быть относительным путём внутри экземпляра")
        if is_server and parts and parts[0] == "plugins":
            parts = parts[1:]
        base = (info.path / ("plugins" if is_server else "mods")).resolve()
        target = base.joinpath(*parts)
        if target.is_symlink() or not target.is_file():
            raise LauncherError("Файл мода или плагина не найден")
        resolved = target.resolve()
        if not resolved.is_relative_to(base):
            raise LauncherError("Путь выходит за пределы экземпляра")
        return resolved, parts[-1], "plugin" if is_server else "mod"
    target = Path(raw_path).expanduser()
    if target.is_symlink() or not target.is_file():
        raise LauncherError("Файл не найден")
    return target, target.name, ""


# ─── Minecraft Bedrock ──────────────────────────────────────────────────────
# Все операции Bedrock (статус среды, версии, установка, вход в Store,
# запуск игры, контент) обслуживает Rust-движок desktop-приложения
# (rust_engine/bedrock.rs) нативно на всех платформах: Linux — BedrockOnLinux,
# Windows — официальный Minecraft for Windows из Microsoft Store.
# Python-воркеру остаются только лёгкие пути профилей для вкладки «Файлы»
# и метрик (см. nivora_launcher/bedrock.py).

def handle(request: dict[str, Any], emit: Callable[[dict[str, Any]], None] | None = None) -> dict[str, Any]:
    from .plugin_guard import guard
    plugin_id = str(request.get("plugin_id", ""))
    operation = str(request.get("operation", ""))
    if not plugin_id or not operation.startswith("plugin_"):
        return _handle(request, emit=emit)
    started = time.monotonic()
    root = ConfigStore().paths.data_home / "audit"
    outcome = "denied"
    try:
        try:
            guard.consume(plugin_id, request, bucket="audit" if operation == "plugin_api_operation_trace" else "rpc")
        except LauncherError:
            outcome = "limited"
            raise
        guard.audit(root, plugin_id, operation, request, "begin", 0)
        result = _handle(request, emit=emit)
        guard.consume(plugin_id, {"response": result}, bucket="audit" if operation == "plugin_api_operation_trace" else "rpc", counts_as_call=False)
        outcome = "ok"
        return result
    finally:
        guard.audit(root, plugin_id, operation, request, outcome, (time.monotonic() - started) * 1000)


def _handle(request: dict[str, Any], emit: Callable[[dict[str, Any]], None] | None = None) -> dict[str, Any]:
    operation_id = str(request.get("operation_id", "")).strip()
    cancellation = CancellationToken(operation_id)
    default_instance_name = str(request.get("instance_name") or request.get("name") or "").strip()

    def scoped_emit(value: dict[str, Any]) -> None:
        if not emit:
            return
        event = dict(value)
        if operation_id:
            event.setdefault("operation_id", operation_id)
        if default_instance_name:
            event.setdefault("instance_name", default_instance_name)
        emit(event)

    operation = str(request.get("operation", "bootstrap"))
    cancellation.check()
    config, http, accounts, repository, installer, launcher, instances = _context(scoped_emit, cancellation)
    _ensure_backup_scheduler()
    workspace = InstanceWorkspace(config, instances)
    bedrock = BedrockProfileStore(config, instances, cancellation)
    instance_files = InstanceFiles(config, instances, bedrock)

    def snapshot_store_for(name: str) -> SnapshotStore:
        """Java снапшотирует каталоги экземпляра (mods/resourcepacks/…),
        Bedrock — контент com.mojang (resource_packs/behavior_packs/миры):
        каталоги игры другие, механизм тот же."""
        info = instances.get(name)
        if info.game_edition == "bedrock":
            return SnapshotStore(
                bedrock.snapshot_root(info),
                categories=BEDROCK_SNAPSHOT_CATEGORIES,
            )
        return SnapshotStore(info.path)

    plugin_store = PluginStore(config)
    theme_store: ThemeStore | None = None

    def get_theme_store() -> ThemeStore:
        nonlocal theme_store
        if theme_store is None:
            theme_store = ThemeStore(config)
        return theme_store

    server_store = ServerInstanceStore(config, instances)
    server_files = ServerFiles(server_store)
    server_processes = ServerProcessManager(config, server_store)

    def run_bedrock_instance(name: str) -> dict[str, Any]:
        # Bedrock-сессии ведёт Rust-движок (intercept_launch в lib.rs).
        # Сюда воркер попадает, только если перехват не сработал: сборка
        # устарела или экземпляр не распознался. Вместо глухой заглушки
        # показываем точное состояние диска — причина видна сразу.
        from .metadata import safe_instance_name

        details = [f"имя={name!r}", f"папка_игры={config.game_dir}"]
        instance_dir = config.game_dir / "instances" / safe_instance_name(name)
        details.append(f"каталог_экземпляра={'есть' if instance_dir.is_dir() else 'НЕТ'}")
        metadata_path = instance_dir / ".nivora-launcher-instance.json"
        if metadata_path.is_file():
            try:
                metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
                details.append(f"kind={str(metadata.get('kind', ''))!r}")
                details.append(f"game_edition={str(metadata.get('game_edition', ''))!r}")
            except (OSError, ValueError, TypeError, RecursionError) as error:
                details.append(f"metadata_не_читается: {error}")
        else:
            details.append("metadata_отсутствует")
        raise LauncherError(
            "Запуск Bedrock ушёл в резервный путь вместо Rust-движка — "
            "сборка устарела или экземпляр не распознан. "
            "Диагностика: " + "; ".join(details)
        )

    if operation == "plugin_dev_changes":
        from .plugin_reload import changes
        return changes(config)

    if operation == "plugin_api_operation_trace":
        plugin_id = str(request.get("plugin_id", ""))
        plugin_store.runtime_source(plugin_id)
        from .plugin_guard import guard
        method = str(request.get("method", ""))
        if not re.fullmatch(r"[a-zA-Z0-9.]{1,128}", method):
            raise LauncherError("Invalid plugin audit method")
        arguments = request.get("arguments", {})
        if not isinstance(arguments, dict): raise LauncherError("Invalid audit arguments")
        outcome = request.get("outcome", "begin")
        if outcome not in {"begin", "ok", "denied"}: raise LauncherError("Invalid audit outcome")
        elapsed = request.get("elapsed_ms", 0)
        if type(elapsed) not in {int, float} or not 0 <= elapsed <= 300000: elapsed = 0
        arguments = {**arguments, "event": request.get("event"), "listener_id": request.get("listener_id")}
        guard.audit(config.paths.data_home / "audit", plugin_id, method, arguments, outcome, elapsed)
        return {"recorded": True}

    if operation == "bootstrap":
        return _bootstrap(config, accounts, instances, plugin_store)

    if operation == "security_status":
        return sandbox_status()

    if operation == "versions":
        values = repository.list_versions(include_snapshots=bool(request.get("include_snapshots", True)))
        return {
            "versions": [
                {
                    "id": str(value.get("id", "")),
                    "type": str(value.get("type", "release")),
                    "release_time": str(value.get("releaseTime", "")),
                }
                for value in values
            ]
        }

    if operation == "java_validate":
        raw_path = str(request.get("path", "")).strip()
        if not raw_path:
            raise LauncherError("Укажите путь к Java executable")
        path = validate_java_path(raw_path, allow_directory=True)
        version = java_version(path)
        if not version:
            raise LauncherError(f"Не удалось определить версию Java: {path}")
        return {"path": str(path.resolve()), "version": version}

    if operation == "graphics_diagnostics":
        return _graphics_diagnostics(config, launcher)

    if operation == "loader_choices":
        loader = str(request.get("loader", "fabric"))
        minecraft_version = str(request.get("minecraft_version", ""))
        choices = LoaderRepository(http).choices(loader, minecraft_version)
        return {"choices": [asdict(value) for value in choices]}

    if operation == "save_settings":
        values = request.get("values", {})
        if not isinstance(values, dict):
            raise LauncherError("Настройки имеют неожиданный формат")
        allowed = {
            "game_dir",
            "java_path",
            "memory",
            "max_downloads",
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
            "auto_repair_enabled",
            "auto_repair_muted",
            "show_activity_panel",
            "show_toasts",
            "minimize_on_launch",
            "close_to_tray",
            "language",
            "font_family",
            "keep_logs_days",
            "telemetry",
            "telemetry_endpoint",
            "keyboard_shortcuts",
            "graphics_mode",
            "launcher_profiles",
            "active_launcher_profile_id",
            "http_proxy",
            "download_source",
            "client_id",
            "developer_mode",
            "theme",
            "active_theme_id",
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
            "overlay_settings",
            "backup_schedule",
            "density",
            "ui_scale",
            "radius_tokens",
            "page_themes",
            "page_layouts",
            "widget_visibility",
            "color_tokens",
            "instance_groups",
            "pinned_instance_groups",
            "resource_catalogs",
            "active_catalog_id",
            "plugin_catalogs",
            "active_plugin_catalog_id",
            "login_source",
            "login_api_url",
            "catalog_source",
            "catalog_api_url",
            "data_source",
            "data_api_url",
            "cloud_url",
            "cloud_username",
            "cloud_remote_path",
            "cloud_include_accounts",
        }
        structured = {"page_themes", "page_layouts", "widget_visibility", "color_tokens", "radius_tokens", "instance_groups", "pinned_instance_groups", "resource_catalogs", "plugin_catalogs", "keyboard_shortcuts", "launcher_profiles", "animation_settings", "overlay_settings", "backup_schedule", "auto_repair_muted"}
        normalized_values = {
            # language: null — валидное значение («язык не выбран»), str(None)
            # его бы испортил.
            key: value if key in structured or key in {"sidebar_floating", "sidebar_width", "language"} else str(value)
            for key, value in values.items()
            if key in allowed
        }
        config.set_values(normalized_values)
        return {"settings": _settings_public(config)}

    if operation == "cloud_status":
        return {"status": cloud_settings_public(config)}

    if operation == "cloud_configure":
        next_url = str(request.get("url", "")).strip().rstrip("/")
        next_username = str(request.get("username", "")).strip()
        WebDavStorage._validated_url(next_url)
        credentials_changed = next_url != config.settings.cloud_url or next_username != config.settings.cloud_username
        values = {
            "cloud_url": next_url,
            "cloud_username": next_username,
            "cloud_remote_path": str(request.get("remote_path", "trial-launcher")),
            "cloud_include_accounts": bool(request.get("include_accounts", False)),
        }
        # Remove the old key before changing its libsecret attributes. Otherwise
        # changing endpoint or username would leave an orphaned credential.
        if credentials_changed:
            save_cloud_password(config, "")
        config.set_values(values)
        password = str(request.get("password", ""))
        if password:
            save_cloud_password(config, password)
        elif credentials_changed:
            save_cloud_password(config, "")
        return {"status": cloud_settings_public(config), "settings": _settings_public(config)}

    if operation == "cloud_test":
        storage = WebDavStorage(config)
        files = storage.list_files()
        return {
            "status": cloud_settings_public(config),
            "files": [value.as_dict() for value in files],
        }

    if operation == "cloud_list":
        storage = WebDavStorage(config)
        return {
            "status": cloud_settings_public(config),
            "files": [value.as_dict() for value in storage.list_files()],
        }

    if operation == "cloud_sync":
        result = sync_cloud(config)
        storage = WebDavStorage(config)
        files = storage.list_files()
        scoped_emit({"kind": "status", "event": "cloud-synced", "message": "Файлы сохранены в облако."})
        return {
            "result": result,
            "status": cloud_settings_public(config),
            "files": [value.as_dict() for value in files],
        }

    if operation == "cloud_restore":
        result = restore_cloud(
            config,
            confirmed=bool(request.get("confirmed", False)),
            manifest_id=str(request.get("manifest_id", "")),
        )
        if result.get("preview"):
            return {"result": result, "status": cloud_settings_public(config)}
        scoped_emit({"kind": "status", "event": "cloud-restored", "message": "Файлы восстановлены из облака."})
        return {"result": result, "status": cloud_settings_public(config)}

    if operation == "cloud_delete":
        delete_cloud_file(config, str(request.get("key", "")))
        storage = WebDavStorage(config)
        return {
            "status": cloud_settings_public(config),
            "files": [value.as_dict() for value in storage.list_files()],
        }

    if operation == "cloud_disconnect":
        save_cloud_password(config, "")
        config.set_values({"cloud_url": "", "cloud_username": "", "cloud_remote_path": "trial-launcher", "cloud_include_accounts": False, "cloud_last_sync": 0})
        return {"status": cloud_settings_public(config), "settings": _settings_public(config)}

    if operation == "wallpaper_import":
        encoded = str(request.get("data", ""))
        try:
            content = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as exc:
            raise LauncherError("Обои повреждены") from exc
        if len(content) > 32 * 1024 * 1024:
            raise LauncherError("Файл обоев слишком большой (максимум 32 MiB)")
        file_name = Path(str(request.get("file_name", "wallpaper.png"))).name
        suffix = Path(file_name).suffix.lower()
        if suffix not in {".png", ".jpg", ".jpeg", ".webp"}:
            raise LauncherError("Поддерживаются PNG, JPEG и WebP")
        if not _valid_wallpaper_bytes(content, suffix):
            raise LauncherError("Файл не является корректным изображением")
        wallpaper_dir = config.paths.data_home / "wallpapers"
        if wallpaper_dir.is_symlink():
            raise LauncherError("Каталог обоев является символьной ссылкой")
        wallpaper_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        if wallpaper_dir.is_symlink() or not wallpaper_dir.is_dir():
            raise LauncherError("Каталог обоев недоступен")
        target = wallpaper_dir / f"background{suffix}"
        for previous in wallpaper_dir.glob("background.*"):
            if previous == target:
                continue
            if previous.is_symlink() or previous.is_file():
                previous.unlink()
        temporary = wallpaper_dir / f".{target.name}.{os.getpid()}.part"
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
        temporary.replace(target)
        config.set_value("wallpaper_path", str(target))
        return {"settings": _settings_public(config)}

    if operation == "wallpaper_clear":
        current = Path(config.settings.wallpaper_path) if config.settings.wallpaper_path else None
        wallpaper_dir = config.paths.data_home / "wallpapers"
        if wallpaper_dir.is_symlink():
            raise LauncherError("Каталог обоев является символьной ссылкой")
        if current and current.parent == wallpaper_dir and not current.is_symlink() and current.exists():
            current.unlink()
        config.set_value("wallpaper_path", "")
        return {"settings": _settings_public(config)}

    # ── Аккаунты ───────────────────────────────────────────────────────────
    # Файловые операции обслуживаются нативно (rust_engine/accounts.rs);
    # сюда запрос попадает только при legacy Microsoft-токенах в файле —
    # их миграция в Secret Service выполняется здесь. Сетевой вход
    # Microsoft также остаётся здесь.

    if operation == "create_offline_account":
        username = str(request.get("username", "Player"))
        account = OfflineDebugAccount(config, accounts).create(username)
        return {"account": _account_public(account)}

    if operation == "login_microsoft":
        if config.settings.login_source != "microsoft":
            raise LauncherError("Источник входа Microsoft отключён в настройках сторонних источников")
        messages: list[str] = []
        def notify_login(message: str) -> None:
            messages.append(message)
            event: dict[str, Any] = {"kind": "status", "message": message, "event": "microsoft-auth"}
            if message.startswith("КОД MICROSOFT:"):
                event["code"] = message.split(":", 1)[1].strip()
            if message.startswith("Откройте "):
                event["verification_uri"] = message.removeprefix("Откройте ").split(" и введите", 1)[0].strip()
            scoped_emit(event)

        account = MicrosoftAuthenticator(config, accounts, http, notify=notify_login, cancellation=cancellation).login_device_code()
        return {"account": _account_public(account), "messages": messages}

    if operation == "microsoft_login_status":
        return microsoft_login_status(config)

    if operation == "delete_account":
        account_id = str(request.get("account_id", ""))
        return {"removed": accounts.remove(account_id)}

    if operation == "set_default_account":
        account = accounts.set_default(str(request.get("account_id", "")))
        return {"account": _account_public(account), "accounts": [_account_public(value) for value in accounts.all()]}

    if operation == "move_account":
        values = accounts.move(str(request.get("account_id", "")), str(request.get("direction", "down")))
        return {"accounts": [_account_public(value) for value in values]}

    if operation == "server_core_versions":
        core = str(request.get("core", "custom")).strip().lower()
        return {"core": core, "versions": server_core_versions(http, core)}

    if operation == "server_plugin_catalogs":
        return {
            "catalogs": normalize_plugin_catalogs(config.settings.plugin_catalogs),
            "active_catalog_id": config.settings.active_plugin_catalog_id,
        }

    if operation == "save_server_plugin_catalogs":
        catalogs = normalize_plugin_catalogs(request.get("catalogs"))
        active = str(request.get("active_catalog_id", config.settings.active_plugin_catalog_id)).strip().lower()
        if not any(item["id"] == active for item in catalogs):
            active = next((item["id"] for item in catalogs if item.get("enabled", True)), catalogs[0]["id"])
        config.set_values({"plugin_catalogs": catalogs, "active_plugin_catalog_id": active})
        return {
            "catalogs": normalize_plugin_catalogs(config.settings.plugin_catalogs),
            "active_catalog_id": config.settings.active_plugin_catalog_id,
        }

    if operation == "server_create":
        custom_jar_data = None
        custom_jar_name = str(request.get("custom_jar_name", "")).strip()
        if request.get("custom_jar_data"):
            custom_jar_data = _decode_base64(request.get("custom_jar_data"), 512 * 1024 * 1024, "Файл ядра")
        info = server_store.create(
            str(request.get("name", "")),
            display_name=str(request.get("display_name", "")),
            core=str(request.get("core", "custom")),
            version=str(request.get("version", "")),
            port=int(request.get("port", 25565)),
            memory=str(request.get("memory", "4GiB")),
            eula_accepted=bool(request.get("eula_accepted", False)),
            custom_jar_name=custom_jar_name,
            custom_jar_data=custom_jar_data,
            replace_existing=bool(request.get("replace_existing", False)),
        )
        scoped_emit({"kind": "status", "event": "server-created", "message": f"Сервер «{info.display_name or info.name}» создан."})
        return {
            "server": server_store.public(info.name),
            "instances": [_instance_public(value) for value in instances.list()],
        }

    if operation == "server_details":
        name = str(request.get("name", ""))
        server_store.get(name)
        snapshots = SnapshotStore(server_store.get(name).path)
        memory_total_kib = system_memory_kib()
        return {
            "server": server_store.public(name),
            "config": server_store.config(name),
            "memory_total_kib": memory_total_kib,
            "memory_limit_kib": server_memory_limit_kib(memory_total_kib),
            "status": server_processes.status(name),
            "files": server_files.list(name, str(request.get("directory", ""))),
            "plugin_files": server_files.list(name, "plugins"),
            "logs": server_processes.log(name),
            "snapshots": {
                "mods": snapshots.list("mods"),
                "plugins": snapshots.list("plugins"),
            },
        }

    if operation == "server_save_config":
        name = str(request.get("name", ""))
        values = request.get("values", {})
        if not isinstance(values, dict):
            raise LauncherError("Настройки сервера имеют неожиданный формат")
        result = server_store.save_config(name, values)
        scoped_emit({"kind": "status", "event": "server-config-changed", "message": "Настройки сервера сохранены."})
        return {"config": result, "server": server_store.public(name)}

    if operation == "server_upload_core":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        content = _decode_base64(request.get("data"), 512 * 1024 * 1024, "Файл ядра")
        file_name = str(request.get("file_name", SERVER_JAR)).strip()
        path = server_store.write_core(info.path, file_name, content)
        scoped_emit({"kind": "status", "event": "server-core-uploaded", "message": "Ядро сервера загружено."})
        return {"path": str(path), "server": server_store.public(name)}

    if operation == "server_core_download":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        core = str(request.get("core", settings["core"])).strip().lower()
        version = str(request.get("version", settings["version"])).strip()
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            result = download_server_core(
                http,
                info.path,
                core,
                version,
                str(request.get("build", settings["build"])).strip(),
                confirm_unverified=bool(request.get("confirm_unverified", False)),
            )
        finally:
            process_lock.release()
        server_store.save_config(name, {"core": core, "version": version, "build": result["build"]})
        scoped_emit({"kind": "status", "event": "server-core-downloaded", "message": "Рекомендованное ядро загружено."})
        return {"download": result, "server": server_store.public(name)}

    if operation in {"server_network_diagnose", "server_network_status", "server_network_open", "server_network_close", "server_network_prepare"}:
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        port = port_number(settings["port"])
        if operation == "server_network_status":
            return {"mapping": server_publisher.state(info.path)}
        if operation == "server_network_close":
            return {"mapping": server_publisher.close(info.path)}
        if operation == "server_network_prepare":
            if request.get("confirmed") is not True:
                raise LauncherError("Подтвердите разрешение входящих подключений к игровому серверу")
            if settings["core"] in {"velocity", "waterfall"}:
                raise LauncherError("Для прокси настройте listener в конфигурации ядра, не в server.properties")
            if _server_is_running(info.path):
                raise LauncherError("Остановите сервер перед изменением server.properties")
            lock = InstanceProcessLock(config.paths.data_home, info.name)
            lock.acquire()
            try:
                _apply_server_port(info.path, port, public_bind=True)
            finally:
                lock.release()
            return {"prepared": True, "port": port}
        proxy_core = settings["core"] in {"velocity", "waterfall"}
        properties = server_files.read_text(name, "server.properties") if not proxy_core and (info.path / "server.properties").exists() else ""
        if operation == "server_network_diagnose":
            result = diagnose_server_network(info.path, port, properties, _server_is_running(info.path),
                                             external_port=request.get("external_port", port))
            if proxy_core:
                result["warnings"].append("У прокси проверьте listener, аутентификацию и доступ к backend-серверам в конфигурации ядра")
            return result
        if request.get("confirmed") is not True:
            raise LauncherError("Подтвердите публикацию игрового TCP-порта в интернете")
        observed = str(request.get("observed_ip", ""))
        if observed and not public_ipv4(observed):
            raise LauncherError("Некорректный внешний IPv4")
        network_info = {**server_network_properties(properties, port), "observed_ip": observed}
        mapping = server_publisher.open(info.path, port, port_number(request.get("external_port", port)),
                                        running=lambda: _server_is_running(info.path), confirmed=True, info=network_info)
        return {"mapping": mapping}

    if operation == "server_start":
        name = str(request.get("name", ""))
        return server_processes.start(name, emit=scoped_emit)

    if operation == "server_stop":
        name = str(request.get("name", ""))
        try:
            server_publisher.close(server_store.get(name).path)
        except LauncherError as exc:
            scoped_emit({"kind": "status", "event": "server-network-close-failed", "message": f"Проброс не удалён: {exc}. Временное правило истечёт без продления."})
        return server_processes.stop(name)

    if operation == "server_restart":
        name = str(request.get("name", ""))
        try:
            server_publisher.close(server_store.get(name).path)
        except LauncherError as exc:
            scoped_emit({"kind": "status", "event": "server-network-close-failed", "message": str(exc)})
        return server_processes.restart(name, emit=scoped_emit)

    if operation == "server_command":
        return server_processes.command(str(request.get("name", "")), str(request.get("command", "")))

    if operation == "server_log":
        return server_processes.log(str(request.get("name", "")), int(request.get("since", 0) or 0))

    if operation == "server_status":
        return server_processes.status(str(request.get("name", "")))

    if operation == "server_file_list":
        return {"files": server_files.list(str(request.get("name", "")), str(request.get("directory", "")))}

    if operation == "server_file_read":
        return {"content": server_files.read_text(str(request.get("name", "")), str(request.get("path", "")))}

    if operation == "server_file_write":
        server_files.write_text(str(request.get("name", "")), str(request.get("path", "")), str(request.get("content", "")))
        return {"saved": True}

    if operation == "server_file_mkdir":
        server_files.mkdir(str(request.get("name", "")), str(request.get("path", "")))
        return {"created": True}

    if operation == "server_file_upload":
        server_files.upload(
            str(request.get("name", "")),
            str(request.get("path", "")),
            _decode_base64(request.get("data"), 256 * 1024 * 1024, "Файл сервера"),
        )
        return {"uploaded": True}

    if operation == "server_file_delete":
        server_files.delete(str(request.get("name", "")), str(request.get("path", "")))
        return {"removed": True}

    if operation == "server_plugin_search":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        catalog = _server_plugin_catalog(config, str(request.get("catalog_id", "")))
        provider = PluginCatalog(http, catalog)
        results = provider.search(
            str(request.get("query", "")),
            str(settings.get("core", "custom")),
            str(settings.get("version", "")),
            max(1, min(30, int(request.get("limit", 12)))),
            sort_by=str(request.get("sort_by", "relevance")),
        )
        installed = {item["project_id"]: item for item in provider.installed(info.path / "plugins")}
        result_rows = []
        for item in results:
            result = item.as_dict()
            installed_item = installed.get(str(result.get("project_id", "")))
            result["installed"] = installed_item is not None
            if installed_item:
                result["file_name"] = installed_item["file_name"]
            result_rows.append(result)
        return {
            "results": result_rows,
            "catalog_id": catalog["id"],
            "core": settings["core"],
            "version": settings["version"],
        }

    if operation == "server_plugin_details":
        name = str(request.get("name", ""))
        settings = server_store.config(name)
        catalog = _server_plugin_catalog(config, str(request.get("catalog_id", "")))
        details = PluginCatalog(http, catalog).details(
            str(request.get("project_id", "")),
            str(settings.get("core", "custom")),
            str(settings.get("version", "")),
        )
        return {"details": details, "catalog_id": catalog["id"], "core": settings["core"], "version": settings["version"]}

    if operation == "server_plugin_updates":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        updates: list[dict[str, Any]] = []
        for catalog in normalize_plugin_catalogs(config.settings.plugin_catalogs):
            if not catalog.get("enabled", True):
                continue
            values = PluginCatalog(http, catalog).check_updates(
                info.path / "plugins",
                str(settings.get("core", "custom")),
                str(settings.get("version", "")),
            )
            updates.extend({**value, "catalog_name": catalog["name"]} for value in values)
        return {"updates": updates, "files": server_files.list(name, "plugins")}

    if operation == "server_plugin_update":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        catalog = _server_plugin_catalog(config, str(request.get("catalog_id", "")))
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            result = PluginCatalog(http, catalog).update_installed(
                info.path / "plugins",
                str(request.get("file_name", "")),
                str(settings.get("core", "custom")),
                str(settings.get("version", "")),
                confirm_unverified=bool(request.get("confirm_unverified", False)),
            )
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "server-plugins-changed", "message": f"Плагин «{result['file_name']}» обновлён."})
        return {"plugin": result, "files": server_files.list(name, "plugins"), "catalog_id": catalog["id"]}

    if operation == "server_plugin_install":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        catalog = _server_plugin_catalog(config, str(request.get("catalog_id", "")))
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            result = PluginCatalog(http, catalog).install(
                str(request.get("project_id", "")),
                info.path / "plugins",
                str(settings.get("version", "")),
                str(settings.get("core", "custom")),
                confirm_unverified=bool(request.get("confirm_unverified", False)),
            )
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "server-plugins-changed", "message": f"Плагин «{result['file_name']}» установлен."})
        return {"plugin": result, "files": server_files.list(name, "plugins"), "catalog_id": catalog["id"]}

    if operation == "server_plugin_remove":
        name = str(request.get("name", ""))
        file_name = str(request.get("file_name", "")).replace("\\", "/")
        if Path(file_name).name != file_name or not file_name.lower().endswith(".jar"):
            raise LauncherError("Укажите имя JAR-файла плагина без пути")
        info = server_store.get(name)
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            if _server_is_running(info.path):
                raise LauncherError("Остановите сервер перед удалением плагина")
            if not remove_installed_plugin(info.path / "plugins", file_name):
                server_files.delete(name, f"plugins/{file_name}")
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "server-plugins-changed", "message": f"Плагин «{file_name}» удалён."})
        return {"removed": True, "files": server_files.list(name, "plugins")}

    if operation == "server_snapshot_list":
        name = str(request.get("name", ""))
        snapshots = SnapshotStore(server_store.get(name).path)
        category = str(request.get("category", "plugins"))
        return {"snapshots": snapshots.list(category), "category": category}

    if operation == "server_snapshot_create":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        settings = server_store.config(name)
        category = str(request.get("category", "plugins"))
        snapshot = SnapshotStore(info.path).create(
            category,
            retention_hours=int(settings["snapshot_retention_hours"]),
            max_count=int(settings["snapshot_max_count"]),
        )
        return {"snapshot": snapshot, "snapshots": SnapshotStore(info.path).list(category)}

    if operation == "server_snapshot_restore":
        name = str(request.get("name", ""))
        info = server_store.get(name)
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            if _server_is_running(info.path):
                raise LauncherError("Остановите сервер перед восстановлением снапшота")
            snapshot_store = SnapshotStore(info.path)
            category = str(request.get("category", "plugins"))
            restored = snapshot_store.restore(category, str(request.get("snapshot_id", "")))
        finally:
            process_lock.release()
        return {"restored": restored, "snapshots": snapshot_store.list(category), "files": server_files.list(name, category)}

    if operation == "server_snapshot_delete":
        name = str(request.get("name", ""))
        snapshot_store = SnapshotStore(server_store.get(name).path)
        category = str(request.get("category", "plugins"))
        return {"removed": snapshot_store.delete(category, str(request.get("snapshot_id", ""))), "snapshots": snapshot_store.list(category)}

    if operation == "delete_instance":
        name = str(request.get("name", ""))
        instances.delete(name)
        return {"instances": [_instance_public(value) for value in instances.list()]}

    if operation == "duplicate_instance":
        source = str(request.get("source", ""))
        target = str(request.get("target", ""))
        value = instances.duplicate(source, target)
        return {"instance": _instance_public(value)}

    # ── Профили настроек ──────────────────────────────────────────────────

    if operation == "settings_profile_save":
        return save_settings_profile(config, str(request.get("name", "")))

    if operation == "settings_profile_apply":
        applied = apply_settings_profile(config, str(request.get("name", "")))
        return {"settings": _settings_public(config), "applied": sorted(applied)}

    # ── Резервные копии миров ─────────────────────────────────────────────
    # Локальные копии и расписания обслуживаются нативно (rust_engine/
    # backups.rs); сюда запрос попадает только при настроенном облаке
    # WebDAV или с явным destination="cloud" / backup_id "@cloud".

    if operation == "world_backup_list":
        name = str(request.get("name", ""))
        backups = WorldBackupStore(config, instances)
        return {"backups": backups.list(name), "schedule": backups.schedule(name)}

    if operation == "world_backup_create":
        name = str(request.get("name", ""))
        backups = WorldBackupStore(config, instances)
        result = backups.create(name, destination=str(request.get("destination", "local")), reason=str(request.get("reason", "manual"))[:24] or "manual")
        return {"backup": result, "schedule": backups.schedule(name)}

    if operation == "world_backup_restore":
        name = str(request.get("name", ""))
        backups = WorldBackupStore(config, instances)
        return backups.restore(name, str(request.get("backup_id", "")))

    if operation == "world_backup_delete":
        name = str(request.get("name", ""))
        backups = WorldBackupStore(config, instances)
        return backups.delete(name, str(request.get("backup_id", "")))

    if operation == "world_backup_schedule_get":
        return {"schedule": WorldBackupStore(config, instances).schedule(str(request.get("name", "")))}

    if operation == "world_backup_schedule_set":
        name = str(request.get("name", ""))
        values = request.get("values", {})
        if not isinstance(values, dict):
            raise LauncherError("Расписание резервных копий имеет неожиданный формат")
        override = request.get("override")
        return {"schedule": WorldBackupStore(config, instances).set_schedule(name, values, override=bool(override) if override is not None else None)}

    # ── Системные метрики ─────────────────────────────────────────────────
    # system_metrics обслуживается до этого роутера (rust_engine/metrics.rs).

    # ── Дерево зависимостей модов ─────────────────────────────────────────

    if operation == "mod_dependency_graph":
        info = instances.get(str(request.get("name", "")))
        if info.game_edition != "java":
            raise LauncherError("Дерево зависимостей доступно для Java-экземпляров")
        graph = dependency_graph(info.path / "mods")
        return graph

    if operation == "mod_removal_impact":
        info = instances.get(str(request.get("name", "")))
        graph = dependency_graph(info.path / "mods")
        return removal_impact(graph, str(request.get("mod_id", "")))

    # ── Экспорт модпака ───────────────────────────────────────────────────

    if operation == "export_modpack":
        info = instances.get(str(request.get("name", "")))
        result = export_instance_as_mrpack(
            http,
            info,
            config.game_dir / "exports",
            display_name=str(request.get("display_name", "") or ""),
            version_label=str(request.get("version_label", "") or ""),
        )
        scoped_emit({"kind": "status", "event": "modpack-exported", "message": f"Модпак собран: {result['file_name']}"})
        return result

    if operation == "external_instances_scan":
        return {"candidates": [_external_instance_public(value) for value in instances.list_import_candidates()]}

    if operation == "external_instance_import":
        source_path = str(request.get("source_path", ""))
        target_name = str(request.get("target_name", "")).strip() or None
        value = instances.import_external_instance(source_path, target_name=target_name)
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Экземпляр импортирован."})
        return {"instance": _instance_public(value), "instances": [_instance_public(item) for item in instances.list()]}


    # ── Optimization operations ───────────────────────────────────────────

    # jvm_presets_list обслуживается до этого роутера (rust_engine/ops.rs);
    # JVM_PRESETS здесь по-прежнему нужны optimization_analyze/apply.

    if operation == "optimization_analyze":
        instance_name = str(request.get("name", "")).strip() or None
        instance_dir: Path | None = None
        loader = ""
        mc_version = ""
        current_memory = config.settings.memory
        current_preset = "none"
        if instance_name:
            try:
                info = instances.get(instance_name)
                if info and info.path:
                    instance_dir = info.path
                inst_settings = workspace.settings(instance_name)
                loader = str(inst_settings.get("loader", "")).strip()
                mc_version = str(inst_settings.get("version", "")).strip()
                current_memory = str(inst_settings.get("memory", "")).strip() or current_memory
                current_preset = str(inst_settings.get("jvm_preset", "none")).strip() or "none"
            except (LauncherError, AttributeError, KeyError):
                pass
        report = analyze_instance(
            instance_dir=instance_dir,
            loader=loader,
            mc_version=mc_version,
            current_memory=current_memory,
            current_preset=current_preset,
        )
        return {"report": report.to_dict()}

    if operation == "optimization_apply":
        instance_name = str(request.get("name", "")).strip() or None
        preset_id = str(request.get("preset_id", "none")).strip()
        memory_mib = request.get("memory_mib")
        graphics_mode = str(request.get("graphics_mode", "")).strip()
        if preset_id not in JVM_PRESETS:
            raise LauncherError(f"Неизвестный пресет: {preset_id}")
        if instance_name:
            existing = workspace.settings(instance_name)
            updates: dict[str, Any] = {"jvm_preset": preset_id}
            if memory_mib is not None:
                mib = max(512, min(32768, int(memory_mib)))
                updates["memory"] = format_memory_kib(mib * 1024)
            if graphics_mode in {"auto", "nvidia", "mesa", "software"}:
                updates["graphics_mode"] = graphics_mode
            for key, val in updates.items():
                workspace.set_setting(instance_name, key, val)
        else:
            # Apply globally
            if memory_mib is not None:
                mib = max(512, min(32768, int(memory_mib)))
                config.set_setting("memory", format_memory_kib(mib * 1024))
            if graphics_mode in {"auto", "nvidia", "mesa", "software"}:
                config.set_setting("graphics_mode", graphics_mode)
            # For global preset: store in active launcher profile
            profiles = list(config.settings.launcher_profiles)
            active_id = config.settings.active_launcher_profile_id
            for profile in profiles:
                if profile.get("id") == active_id:
                    profile["jvm_preset"] = preset_id
                    break
            config.set_setting("launcher_profiles", profiles)
        return {"ok": True, "preset_id": preset_id}

    # launch_history, cache_stats и cache_prune обслуживаются до этого
    # роутера (rust_engine/ops.rs).

    if operation == "instance_details":
        return workspace.snapshot(str(request.get("name", "")))

    if operation == "instance_snapshot_list":
        name = str(request.get("name", ""))
        category = str(request.get("category", "mods"))
        return {"category": category, "snapshots": snapshot_store_for(name).list(category)}

    if operation == "instance_snapshot_create":
        name = str(request.get("name", ""))
        category = str(request.get("category", "mods"))
        snapshot = snapshot_store_for(name).create(category, retention_hours=168, max_count=20)
        return {"snapshot": snapshot, "snapshots": snapshot_store_for(name).list(category), "details": workspace.snapshot(name)}

    if operation == "instance_snapshot_restore":
        name = str(request.get("name", ""))
        info = instances.get(name)
        if info.running:
            raise LauncherError("Остановите экземпляр перед восстановлением снапшота")
        category = str(request.get("category", "mods"))
        restored = snapshot_store_for(name).restore(category, str(request.get("snapshot_id", "")))
        return {"restored": restored, "snapshots": snapshot_store_for(name).list(category), "details": workspace.snapshot(name)}

    if operation == "instance_snapshot_delete":
        name = str(request.get("name", ""))
        category = str(request.get("category", "mods"))
        store = snapshot_store_for(name)
        removed = store.delete(category, str(request.get("snapshot_id", "")))
        return {"removed": removed, "snapshots": store.list(category), "details": workspace.snapshot(name)}

    if operation == "instance_groups":
        return {"groups": instances.groups()}

    if operation == "create_instance_group":
        return {"groups": instances.create_group(str(request.get("name", "")))}

    if operation == "delete_instance_group":
        return {"groups": instances.delete_group(str(request.get("name", "")))}

    if operation == "rename_instance_group":
        return {"groups": instances.rename_group(str(request.get("old_name", "")), str(request.get("new_name", "")))}

    if operation == "update_instance_profile":
        name = str(request.get("name", ""))
        profile = instances.update_profile(
            name,
            display_name=str(request.get("display_name", "")),
            description=str(request.get("description", "")),
            group=str(request.get("group", "")),
            icon_file_name=str(request.get("icon_file_name", "")),
            icon_data=str(request.get("icon_data", "")),
            banner_file_name=str(request.get("banner_file_name", "")),
            banner_data=str(request.get("banner_data", "")),
            clear_icon=bool(request.get("clear_icon", False)),
            clear_banner=bool(request.get("clear_banner", False)),
            notes=request.get("notes") if request.get("notes") is not None else None,
            tags=request.get("tags") if isinstance(request.get("tags"), list) else None,
        )
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Профиль экземпляра обновлён."})
        return {"instance": _instance_public(profile), "groups": instances.groups()}

    if operation == "instance_log":
        name = str(request.get("name", ""))
        info = instances.get(name)
        state_path = info.path / ".trial-launcher-live.json"
        lines, running = _session_snapshot(name)
        started_at = 0.0
        pid = 0
        log_id = ""
        try:
            if (
                state_path.is_symlink()
                or not state_path.is_file()
                or state_path.stat().st_size > MAX_INSTANCE_LIVE_STATE_JSON_BYTES
            ):
                raise OSError("instance live state is unavailable")
            state = json.loads(state_path.read_text(encoding="utf-8"))
            if not isinstance(state, dict):
                raise ValueError("instance live state is not an object")
            running = bool(state.get("running"))
            pid = int(state.get("pid", 0)) if running else 0
            started_at = float(state.get("started_at", 0) or 0)
            log_id = str(state.get("log_id", ""))
            if pid > 0:
                try:
                    os.kill(pid, 0)
                    running = True
                except ProcessLookupError:
                    running = False
                except PermissionError:
                    running = True
        except (OSError, ValueError, TypeError, RecursionError):
            pass
        if log_id:
            try:
                lines = instance_files.read_log_tail(name, log_id, _MAX_SESSION_LOG_LINES)
            except LauncherError:
                pass
        metrics_root = bedrock.profile_path(info) if info.game_edition == "bedrock" else info.path
        return {
            "lines": lines,
            "running": running,
            "metrics": _instance_metrics(info, pid, started_at, running, root=metrics_root),
        }

    if operation == "instance_file_list":
        return {"files": instance_files.list_files(str(request.get("name", "")), request.get("directory", ""))}

    if operation == "instance_file_read":
        return instance_files.read_file(
            str(request.get("name", "")),
            request.get("path", ""),
            request.get("encoding", "text"),
        )

    if operation == "instance_file_write":
        content = request.get("content", "")
        if not isinstance(content, str):
            raise LauncherError("Текстовый файл должен содержать UTF-8 текст")
        instance_files.write_file(str(request.get("name", "")), request.get("path", ""), content)
        return {"saved": True}

    if operation == "instance_file_mkdir":
        instance_files.mkdir(str(request.get("name", "")), request.get("path", ""))
        return {"created": True}

    if operation == "instance_file_upload":
        content = _decode_base64(request.get("data"), MAX_INSTANCE_UPLOAD_BYTES, "Файл экземпляра")
        instance_files.upload(str(request.get("name", "")), request.get("path", ""), content)
        return {"uploaded": True}

    if operation == "instance_file_delete":
        instance_files.delete(str(request.get("name", "")), request.get("path", ""))
        return {"removed": True}

    if operation == "instance_log_archives":
        return instance_files.list_log_archives(
            str(request.get("name", "")),
            request.get("offset", 0),
            request.get("limit", 50),
        )

    if operation == "instance_log_archive_read":
        return instance_files.read_log_archive(
            str(request.get("name", "")),
            request.get("log_id", ""),
            request.get("offset", 0),
            request.get("limit", 131072),
        )

    if operation == "save_instance_settings":
        name = str(request.get("name", ""))
        values = request.get("values", {})
        if not isinstance(values, dict):
            raise LauncherError("Настройки экземпляра имеют неожиданный формат")
        settings = workspace.save_settings(name, values)
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Настройки экземпляра обновлены."})
        return {"settings": settings}

    if operation == "plugin_list":
        return {"plugins": plugin_store.list()}

    if operation == "plugin_security_status":
        return plugin_store.security_status()

    if operation == "plugin_permission_grants_set":
        plugin = plugin_store.set_permission_grants(
            str(request.get("plugin_id", "")).strip(),
            request.get("permissions"),
        )
        return {"plugin": plugin, "plugins": plugin_store.list()}

    if operation == "plugin_local_untrusted_execution":
        plugin = plugin_store.set_local_untrusted_execution(
            str(request.get("plugin_id", "")).strip(),
            request.get("allowed", False),
        )
        return {"plugin": plugin, "plugins": plugin_store.list()}

    if operation == "plugin_publisher_pin":
        return plugin_store.pin_publisher(
            str(request.get("key_id", "")).strip(),
            request.get("public_key"),
            revocation_authority=request.get("revocation_authority", False),
        )

    if operation == "plugin_publisher_unpin":
        return plugin_store.unpin_publisher(str(request.get("key_id", "")).strip())

    if operation == "plugin_publisher_revocation_authority_set":
        return plugin_store.set_publisher_revocation_authority(
            str(request.get("key_id", "")).strip(),
            request.get("enabled"),
        )

    if operation == "plugin_revocations_import":
        imported = plugin_store.import_revocations_json(request.get("document_json"))
        return {"imported": imported, **plugin_store.security_status()}

    if operation == "plugin_local_revoke":
        plugin = plugin_store.revoke_plugin(
            str(request.get("plugin_id", "")).strip(),
            request.get("reason", ""),
        )
        return {"plugin": plugin, "plugins": plugin_store.list()}

    if operation == "plugin_local_unrevoke":
        plugin = plugin_store.unrevoke_plugin(str(request.get("plugin_id", "")).strip())
        return {"plugin": plugin, "plugins": plugin_store.list()}

    if operation == "github_theme_search":
        page = int(request.get("page", 1))
        catalog = GitHubThemeCatalog(http, get_theme_store())
        return {"results": catalog.search(str(request.get("query", "")), page=page), "page": page}

    if operation == "github_theme_install":
        store = get_theme_store()
        catalog = GitHubThemeCatalog(http, store)
        theme, metadata = catalog.install(
            str(request.get("owner", "")),
            str(request.get("repo", "")),
            replace=bool(request.get("replace", False)),
        )
        scoped_emit({"kind": "status", "event": "theme-imported", "message": f"Тема «{theme['name']}» загружена из GitHub."})
        return {"theme": theme, "themes": store.list(), "metadata": metadata}

    if operation == "plugin_install":
        encoded = str(request.get("data", ""))
        if len(encoded) > 4 * ((64 * 1024 * 1024 + 2) // 3):
            raise LauncherError("Архив плагина слишком большой")
        try:
            content = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as exc:
            raise LauncherError("Архив плагина повреждён") from exc
        plugin = plugin_store.install_archive(
            content,
            enable=bool(request.get("enable", False)),
            replace=bool(request.get("replace", False)),
        )
        scoped_emit({"kind": "status", "event": "plugins-changed", "message": f"Плагин «{plugin['name']}» установлен."})
        return {"plugin": plugin, "plugins": plugin_store.list()}

    if operation == "plugin_set_enabled":
        plugin_id = str(request.get("plugin_id", "")).strip()
        plugin = plugin_store.set_enabled(plugin_id, bool(request.get("enabled", False)))
        scoped_emit({"kind": "status", "event": "plugins-changed", "message": f"Плагин «{plugin['name']}» {('включён' if plugin['enabled'] else 'выключен')}."})
        return {"plugin": plugin, "plugins": plugin_store.list()}

    if operation == "plugin_uninstall":
        plugin_id = str(request.get("plugin_id", "")).strip()
        plugin = next((item for item in plugin_store.list() if item.get("id") == plugin_id), {"name": plugin_id})
        plugin_store.uninstall(plugin_id)
        scoped_emit({"kind": "status", "event": "plugins-changed", "message": f"Плагин «{plugin['name']}» удалён."})
        return {"removed": True, "plugins": plugin_store.list()}

    if operation == "plugin_api_source":
        return plugin_store.runtime_source(str(request.get("plugin_id", "")).strip())

    if operation == "plugin_api_runtime_info":
        return plugin_store.runtime_info(str(request.get("plugin_id", "")).strip())

    if operation == "plugin_api_capabilities":
        info = plugin_store.runtime_info(str(request.get("plugin_id", "")).strip())
        return {
            "plugin_id": info["plugin_id"],
            "api_version": info["api_version"],
            "capabilities": info["capabilities"],
            "sandbox_profile": info["sandbox_profile"],
        }

    if operation == "plugin_api_quota_usage":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "runtime:observe")
        from .plugin_guard import guard
        return {**plugin_store.quota_usage(plugin_id), "operations": guard.usage(plugin_id)}

    if operation == "plugin_api_permissions_get":
        return plugin_store.permission_summary(str(request.get("plugin_id", "")).strip())

    if operation == "plugin_api_permissions_set":
        plugin = plugin_store.set_permission_grants(
            str(request.get("plugin_id", "")).strip(),
            request.get("permissions", []),
        )
        return {"plugin": plugin, "permissions": plugin_store.permission_summary(plugin["id"])}

    if operation == "plugin_api_authorize":
        plugin_id = str(request.get("plugin_id", "")).strip()
        permission = request.get("permission")
        if not isinstance(permission, str):
            raise LauncherError("Разрешение плагина должно быть строкой")
        plugin_store.authorize_operation(plugin_id, permission)
        return {"authorized": True}

    if operation == "plugin_api_open_external":
        plugin_id = str(request.get("plugin_id", "")).strip()
        manifest = _require_plugin_permission_any(plugin_store, plugin_id, ["os:open-url", "links:open"])
        url = request.get("url")
        if not isinstance(url, str) or not url or url != url.strip() or any(ord(char) < 0x20 or ord(char) == 0x7F for char in url):
            raise LauncherError("Некорректная внешняя ссылка плагина")
        try:
            parsed = urlsplit(url)
            hostname = parsed.hostname
            parsed.port
        except ValueError as exc:
            raise LauncherError("Некорректная внешняя ссылка плагина") from exc
        if parsed.scheme.lower() not in {"http", "https"} or not hostname or parsed.username or parsed.password:
            raise LauncherError("Плагинам разрешены только HTTP(S)-ссылки без credentials")
        if parsed.scheme.lower() == "http" and "network_hosts" in manifest and manifest.get("network_hosts"):
            _plugin_network_host(manifest, hostname)
        try:
            open_path(Path(url))
        except (OSError, RuntimeError) as exc:
            raise LauncherError(f"Не удалось открыть ссылку плагина: {exc}") from exc
        return {"opened": True}

    if operation == "open_external":
        # Внутренние ссылки лаунчера (вход Microsoft и пр.): webview сам их не
        # открывает, действуем через системный открыватель — тот же, что у плагинов.
        url = request.get("url")
        if not isinstance(url, str) or not url or url != url.strip() or any(ord(char) < 0x20 or ord(char) == 0x7F for char in url):
            raise LauncherError("Некорректная внешняя ссылка")
        try:
            parsed = urlsplit(url)
            hostname = parsed.hostname
            parsed.port
        except ValueError as exc:
            raise LauncherError("Некорректная внешняя ссылка") from exc
        if parsed.scheme.lower() not in {"http", "https"} or not hostname or parsed.username or parsed.password:
            raise LauncherError("Разрешены только HTTP(S)-ссылки без credentials")
        try:
            open_path(Path(url))
        except (OSError, RuntimeError) as exc:
            raise LauncherError(f"Не удалось открыть ссылку: {exc}") from exc
        return {"opened": True}

    if operation.startswith("github_plugin_"):
        github = GitHubPluginService(http=http, token_provider=SecretToolTokenProvider())

        if operation == "github_plugin_status":
            return github.status()
        if operation == "github_plugin_search":
            page = int(request.get("page", 1))
            return {"results": github.search(str(request.get("query", "")), page=page), "page": page}
        if operation == "github_plugin_details":
            return github.details(str(request.get("owner", "")), str(request.get("repo", "")))
        if operation == "github_plugin_install":
            metadata, content = github.download_plugin_archive(str(request.get("owner", "")), str(request.get("repo", "")))
            plugin = plugin_store.install_archive(content, replace=bool(request.get("replace", False)))
            scoped_emit({"kind": "status", "event": "plugins-changed", "message": f"Архив плагина «{plugin['name']}» сохранён."})
            return {
                "plugin": plugin,
                "plugins": plugin_store.list(),
                "metadata": {key: metadata[key] for key in ("full_name", "version", "size", "sha256")},
            }
        if operation == "github_plugin_oauth_start":
            return github.start_oauth_device_flow()
        if operation == "github_plugin_oauth_poll":
            return github.poll_oauth_device_flow(str(request.get("device_code", "")))
        if operation == "github_plugin_sign_out":
            SecretToolTokenProvider().clear_token()
            return {"authorized": False}
        if operation == "github_plugin_publish":
            archive = _decode_base64(request.get("data"), PLUGIN_MAX_ARCHIVE_BYTES, "Архив плагина")
            return github.publish_plugin(archive, description=str(request.get("description", "")))

    if operation == "plugin_network_fetch":
        plugin_id = str(request.get("plugin_id", "")).strip()
        manifest = _require_plugin_permission_any(plugin_store, plugin_id, ["network:fetch", "network:https"])
        if getattr(http, "proxy", ""):
            raise LauncherError("Сетевые запросы плагинов через прокси запрещены: адрес узла невозможно проверить на стороне соединения")
        url = str(request.get("url", "")).strip()
        try:
            parsed = urlsplit(url)
            host = (parsed.hostname or "").lower().rstrip(".")
            port = parsed.port
        except ValueError as exc:
            raise LauncherError("Некорректный адрес плагина") from exc
        if parsed.scheme.lower() != "https" or not host or parsed.username or parsed.password or port:
            raise LauncherError("Плагинам разрешены только HTTPS-адреса без credentials и нестандартного порта")
        if host not in set(manifest.get("network_hosts", [])):
            raise LauncherError("Домен не входит в allowlist плагина")
        _resolve_public_plugin_host(manifest, host, port or 443)

        def validate_redirect(target: str) -> None:
            try:
                redirected = urlsplit(target)
                redirected_host = (redirected.hostname or "").lower().rstrip(".")
                redirected_port = redirected.port
            except ValueError as exc:
                raise LauncherError("Сервис плагина вернул некорректный redirect") from exc
            if redirected.scheme.lower() != "https" or not redirected_host or redirected.username or redirected.password or redirected_port:
                raise LauncherError("Redirect плагина должен оставаться HTTPS без credentials и нестандартного порта")
            if redirected_host not in set(manifest.get("network_hosts", [])):
                raise LauncherError("Redirect плагина вышел за пределы allowlist")
            _resolve_public_plugin_host(manifest, redirected_host, redirected_port or 443)

        request_options = request.get("request", {})
        if not isinstance(request_options, dict):
            raise LauncherError("Параметры сетевого запроса плагина должны быть объектом")
        method = str(request_options.get("method", "GET")).upper()
        if method not in {"GET", "POST", "PUT", "DELETE"}:
            raise LauncherError("Метод сетевого запроса плагина не поддерживается")
        raw_headers = request_options.get("headers", {})
        if not isinstance(raw_headers, dict) or len(raw_headers) > 64:
            raise LauncherError("Заголовки сетевого запроса плагина имеют неверный формат")
        headers = {"Accept": "application/json"}
        for name, value in raw_headers.items():
            if (
                not isinstance(name, str)
                or not re.fullmatch(r"[A-Za-z0-9-]{1,64}", name)
                or name.lower() in {"host", "cookie", "content-length", "connection", "origin", "referer", "user-agent", "proxy-authorization"}
                or not isinstance(value, str)
                or len(value) > 8192
                or "\r" in value
                or "\n" in value
            ):
                raise LauncherError("Сетевой запрос плагина содержит небезопасный заголовок")
            headers[name] = value
        body = request_options.get("body")
        if body is not None and method not in {"POST", "PUT"}:
            raise LauncherError("JSON-тело плагина разрешено только для POST и PUT")
        try:
            body_bytes = json.dumps(body, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8") if body is not None else None
        except (TypeError, ValueError, UnicodeEncodeError, RecursionError) as exc:
            raise LauncherError("Тело сетевого запроса плагина должно быть JSON") from exc
        if body_bytes is not None and len(body_bytes) > 1024 * 1024:
            raise LauncherError("Тело сетевого запроса плагина слишком большое")
        if body_bytes is not None:
            headers.setdefault("Content-Type", "application/json")
        try:
            value = json.loads(
                http.request(
                    url,
                    method=method,
                    data=body_bytes,
                    headers=headers,
                    max_bytes=4 * 1024 * 1024,
                    redirect_validator=validate_redirect,
                ).decode("utf-8")
            )
        except json.JSONDecodeError as exc:
            raise LauncherError("Сервис плагина вернул некорректный JSON") from exc
        return {"value": value}

    if operation == "plugin_http_request":
        plugin_id = str(request.get("plugin_id", "")).strip()
        request_options = request.get("request", {})
        if not isinstance(request_options, dict):
            raise LauncherError("Параметры HTTP-запроса плагина должны быть объектом")
        url = str(request_options.get("url", "")).strip()
        try:
            parsed = urlsplit(url)
            host = (parsed.hostname or "").lower().rstrip(".")
            port = parsed.port
        except ValueError as exc:
            raise LauncherError("Некорректный адрес HTTP-запроса плагина") from exc
        scheme = parsed.scheme.lower()
        if scheme not in {"http", "https"} or not host or parsed.username or parsed.password:
            raise LauncherError("Плагинам разрешены только HTTP(S)-адреса без credentials")
        required_permission = "network:https" if scheme == "https" else "network:http"
        manifest = _require_plugin_permission_any(plugin_store, plugin_id, [required_permission, "network:fetch"])
        if host not in set(manifest.get("network_hosts", [])):
            raise LauncherError("Домен не входит в allowlist плагина")
        _resolve_public_plugin_host(manifest, host, port or (443 if scheme == "https" else 80))

        method = str(request_options.get("method", "GET")).upper()
        if method not in {"GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"}:
            raise LauncherError("HTTP-метод плагина не поддерживается")
        raw_headers = request_options.get("headers", {})
        if not isinstance(raw_headers, dict) or len(raw_headers) > 64:
            raise LauncherError("Заголовки HTTP-запроса плагина имеют неверный формат")
        headers: dict[str, str] = {}
        for name, value in raw_headers.items():
            if (
                not isinstance(name, str)
                or not re.fullmatch(r"[A-Za-z0-9-]{1,64}", name)
                or name.lower() in {"host", "cookie", "content-length", "connection", "origin", "referer", "user-agent", "proxy-authorization"}
                or not isinstance(value, str)
                or len(value) > 8192
                or "\r" in value
                or "\n" in value
            ):
                raise LauncherError("HTTP-запрос плагина содержит небезопасный заголовок")
            headers[name] = value
        timeout_ms = _plugin_timeout_ms(request_options.get("timeout_ms", 3000))
        body = request_options.get("body")
        body_bytes: bytes | None = None
        if body is not None:
            if isinstance(body, str):
                body_bytes = body.encode("utf-8")
            elif isinstance(body, dict) or isinstance(body, list) or body is None or isinstance(body, (int, float, bool)):
                try:
                    body_bytes = json.dumps(body, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")
                except (TypeError, ValueError, UnicodeEncodeError, RecursionError) as exc:
                    raise LauncherError("Тело HTTP-запроса плагина должно быть сериализуемым JSON или строкой") from exc
                headers.setdefault("Content-Type", "application/json")
            else:
                raise LauncherError("Тело HTTP-запроса плагина должно быть строкой или JSON")
            if len(body_bytes) > 1024 * 1024:
                raise LauncherError("Тело HTTP-запроса плагина слишком большое")

        raw = http.request_detailed(
            url,
            method=method,
            data=body_bytes,
            headers=headers,
            max_bytes=4 * 1024 * 1024,
            redirect_validator=(
                lambda target: _resolve_public_plugin_host(
                    manifest,
                    (urlsplit(target).hostname or "").lower().rstrip("."),
                    urlsplit(target).port or (443 if urlsplit(target).scheme.lower() == "https" else 80),
                )
            ),
        )
        try:
            body_text = raw["body"].decode("utf-8")
        except UnicodeError:
            body_text = raw["body"].decode("utf-8", errors="replace")
        body_json = None
        content_type = str(raw["headers"].get("Content-Type", ""))
        if "json" in content_type.lower():
            try:
                body_json = json.loads(body_text)
            except json.JSONDecodeError:
                body_json = None
        return {
            "status": raw["status"],
            "url": raw["url"],
            "headers": raw["headers"],
            "body_text": body_text,
            "body_json": body_json,
            "timeout_ms": timeout_ms,
        }

    if operation == "plugin_api_instances_list":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "instances:read")
        return {"instances": [_instance_public(item) for item in instances.list()]}

    if operation == "plugin_api_process_metrics":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "instances:read")
        _require_plugin_permission(plugin_store, plugin_id, "process:observe")
        name = str(request.get("instance_name", "")).strip()
        info = instances.get(name)
        state_path = info.path / ".trial-launcher-live.json"
        running = bool(info.running)
        started_at = 0.0
        pid = 0
        try:
            if (
                state_path.is_symlink()
                or not state_path.is_file()
                or state_path.stat().st_size > MAX_INSTANCE_LIVE_STATE_JSON_BYTES
            ):
                raise OSError("instance live state is unavailable")
            state = json.loads(state_path.read_text(encoding="utf-8"))
            if not isinstance(state, dict):
                raise ValueError("instance live state is not an object")
            running = bool(state.get("running"))
            pid = int(state.get("pid", 0)) if running else 0
            started_at = float(state.get("started_at", 0) or 0)
            if pid > 0:
                try:
                    os.kill(pid, 0)
                    running = True
                except ProcessLookupError:
                    running = False
                except PermissionError:
                    running = True
        except (OSError, ValueError, TypeError, RecursionError):
            pass
        return {
            "instance": _instance_public(info),
            "running": running,
            "pid": pid,
            "started_at": started_at,
            "metrics": _instance_metrics(info, pid, started_at, running),
        }

    if operation == "plugin_api_proxy_routes_get":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "network:proxy-control")
        return {"routes": _plugin_proxy_route_list(plugin_store.read_storage(plugin_id, "proxy-routes"))}

    if operation == "plugin_api_proxy_routes_save":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "network:proxy-control")
        routes = _plugin_proxy_route_list(request.get("routes"))
        plugin_store.write_storage(plugin_id, "proxy-routes", routes)
        return {"routes": routes}

    if operation == "plugin_api_proxy_routes_add":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "network:proxy-control")
        routes = _plugin_proxy_route_list(plugin_store.read_storage(plugin_id, "proxy-routes"))
        route = _plugin_proxy_route_entry(request.get("route"), fallback_id=f"route-{uuid.uuid4().hex[:12]}")
        if any(item["id"] == route["id"] for item in routes):
            raise LauncherError(f"Правило маршрутизации уже существует: {route['id']}")
        if len(routes) >= 128:
            raise LauncherError("Список proxy route policy плагина заполнен")
        routes.append(route)
        plugin_store.write_storage(plugin_id, "proxy-routes", routes)
        return {"route": route, "routes": routes}

    if operation == "plugin_api_proxy_routes_remove":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "network:proxy-control")
        route_id = str(request.get("route_id", "")).strip()
        routes = _plugin_proxy_route_list(plugin_store.read_storage(plugin_id, "proxy-routes"))
        next_routes = [item for item in routes if item["id"] != route_id]
        plugin_store.write_storage(plugin_id, "proxy-routes", next_routes)
        return {"removed": len(next_routes) != len(routes), "routes": next_routes}

    if operation == "plugin_network_dns":
        plugin_id = str(request.get("plugin_id", "")).strip()
        manifest = _require_plugin_permission(plugin_store, plugin_id, "network:dns")
        host = _plugin_network_host(manifest, request.get("host"))
        port = _plugin_port(request.get("port"), allow_empty=True)
        records = _resolve_public_plugin_host(manifest, host, port)
        addresses: list[dict[str, str]] = []
        seen: set[tuple[str, str]] = set()
        for family, _, _, _, sockaddr in records:
            address = str(sockaddr[0])
            family_name = "ipv6" if family == socket.AF_INET6 else "ipv4" if family == socket.AF_INET else str(family)
            key = (family_name, address)
            if key not in seen:
                seen.add(key)
                addresses.append({"family": family_name, "address": address})
        return {"host": host, "port": port, "addresses": addresses[:64]}

    if operation in {"plugin_network_ping", "plugin_network_port_check"}:
        plugin_id = str(request.get("plugin_id", "")).strip()
        manifest = _require_plugin_permission(plugin_store, plugin_id, "network:ping")
        host = _plugin_network_host(manifest, request.get("host"))
        port = _plugin_port(request.get("port"), allow_empty=False)
        timeout_ms = _plugin_timeout_ms(request.get("timeout_ms", 3000))
        records = _resolve_public_plugin_host(manifest, host, port)
        started = time.perf_counter()
        reachable = False
        error = ""
        for family, socktype, protocol, _, sockaddr in records:
            try:
                with socket.socket(family, socktype, protocol) as connection:
                    connection.settimeout(timeout_ms / 1000)
                    connection.connect(sockaddr)
                reachable = True
                error = ""
                break
            except OSError as exc:
                error = str(exc)
        return {
            "host": host,
            "port": port,
            "reachable": reachable,
            "latency_ms": round((time.perf_counter() - started) * 1000, 2),
            "error": error,
        }

    if operation == "plugin_server_list_get":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "launcher:servers")
        return {"servers": _plugin_server_list(plugin_store.read_storage(plugin_id, "server-list"))}

    if operation == "plugin_server_list_save":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "launcher:servers")
        servers = _plugin_server_list(request.get("servers"))
        plugin_store.write_storage(plugin_id, "server-list", servers)
        return {"servers": servers}

    if operation == "plugin_server_list_add":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "launcher:servers")
        servers = _plugin_server_list(plugin_store.read_storage(plugin_id, "server-list"))
        entry = _plugin_server_entry(request.get("server"), fallback_id=f"server-{uuid.uuid4().hex[:12]}")
        if any(server["id"] == entry["id"] for server in servers):
            raise LauncherError(f"Сервер уже есть в списке: {entry['id']}")
        if len(servers) >= 200:
            raise LauncherError("Список серверов плагина заполнен")
        servers.append(entry)
        plugin_store.write_storage(plugin_id, "server-list", servers)
        return {"server": entry, "servers": servers}

    if operation == "plugin_server_list_remove":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "launcher:servers")
        server_id = str(request.get("server_id", "")).strip()
        servers = _plugin_server_list(plugin_store.read_storage(plugin_id, "server-list"))
        next_servers = [server for server in servers if server["id"] != server_id]
        plugin_store.write_storage(plugin_id, "server-list", next_servers)
        return {"removed": len(next_servers) != len(servers), "servers": next_servers}

    if operation == "plugin_storage_get":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:workspace")
        return {"value": plugin_store.read_storage(plugin_id, str(request.get("key", "")))}

    if operation == "plugin_storage_set":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:workspace")
        plugin_store.write_storage(plugin_id, str(request.get("key", "")), request.get("value"))
        return {"saved": True}

    if operation == "plugin_storage_remove":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:workspace")
        plugin_store.remove_storage(plugin_id, str(request.get("key", "")))
        return {"removed": True}

    if operation == "plugin_config_get":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:workspace")
        return {"value": plugin_store.config_get(plugin_id, str(request.get("key", "")))}

    if operation == "plugin_config_set":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:workspace")
        plugin_store.config_set(plugin_id, str(request.get("key", "")), request.get("value"))
        return {"saved": True}

    if operation == "plugin_config_remove":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:workspace")
        plugin_store.config_remove(plugin_id, str(request.get("key", "")))
        return {"removed": True}

    if operation == "plugin_file_list":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "files:plugin")
        return {"files": plugin_store.list_files(plugin_id, str(request.get("directory", "")))}

    if operation == "plugin_file_read":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "files:plugin")
        return {"content": plugin_store.read_file(plugin_id, str(request.get("path", "")))}

    if operation == "plugin_file_write":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "files:plugin")
        plugin_store.write_file(plugin_id, str(request.get("path", "")), str(request.get("content", "")))
        return {"saved": True}

    if operation == "plugin_file_remove":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "files:plugin")
        plugin_store.remove_file(plugin_id, str(request.get("path", "")))
        return {"removed": True}

    if operation in {"plugin_api_files_list", "plugin_api_files_read", "plugin_api_files_write", "plugin_api_files_remove"}:
        plugin_id = str(request.get("plugin_id", "")).strip()
        scope = request.get("scope", "plugin")
        path = str(request.get("path", ""))
        if scope == "plugin":
            _require_plugin_permission(plugin_store, plugin_id, "files:plugin")
            if operation == "plugin_api_files_list":
                return {"files": plugin_store.list_file_info(plugin_id, str(request.get("directory", "")))}
            if operation == "plugin_api_files_read":
                return {"content": plugin_store.read_file(plugin_id, path)}
            if operation == "plugin_api_files_write":
                content = request.get("content", "")
                if not isinstance(content, str):
                    raise LauncherError("Текстовый файл должен содержать UTF-8 текст")
                plugin_store.write_file(plugin_id, path, content)
                return {"saved": True}
            plugin_store.remove_file(plugin_id, path)
            return {"removed": True}
        if scope == "instance":
            _require_plugin_permission(plugin_store, plugin_id, "files:instance")
            instance_name = str(request.get("instance_name", "")).strip()
            if operation == "plugin_api_files_list":
                entries = instance_files.list_files(instance_name, str(request.get("directory", "")))
                return {"files": [
                    {"path": item["path"], "size": item["size"], "modified_at": item["modified_at"]}
                    for item in entries if item.get("kind") == "file"
                ]}
            if operation == "plugin_api_files_read":
                return instance_files.read_file(instance_name, path)
            if operation == "plugin_api_files_write":
                content = request.get("content", "")
                if not isinstance(content, str):
                    raise LauncherError("Текстовый файл должен содержать UTF-8 текст")
                instance_files.write_file(instance_name, path, content)
                return {"saved": True}
            instance_files.delete(instance_name, path)
            return {"removed": True}
        if scope == "launcher-root-readonly":
            _require_plugin_permission(plugin_store, plugin_id, "files:launcher-root-read")
            if operation == "plugin_api_files_list":
                return {"files": plugin_store.list_launcher_files(str(request.get("directory", "")))}
            if operation == "plugin_api_files_read":
                return {"content": plugin_store.read_launcher_file(path)}
            raise LauncherError("launcher-root-readonly поддерживает только list/read")
        raise LauncherError("Область файлов плагина должна быть plugin, instance или launcher-root-readonly")

    if operation == "plugin_api_config_projected_read":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "files:launcher-config-projected")
        schema_id = str(request.get("schema_id", "")).strip()
        return plugin_store.projected_launcher_config(schema_id)

    if operation == "plugin_database_query":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:database")
        connection = plugin_store.database(plugin_id, read_only=True)
        try:
            sql = _safe_plugin_sql(str(request.get("sql", "")), read_only=True)
            parameters = request.get("parameters", [])
            if not isinstance(parameters, list):
                raise LauncherError("Параметры SQL должны быть списком")
            deadline = time.monotonic() + 2.0
            connection.set_progress_handler(lambda: int(time.monotonic() >= deadline), 1000)
            cursor = connection.execute(sql, parameters)
            rows = cursor.fetchmany(500)
            columns = [column[0] for column in cursor.description or []]
            return {"rows": [dict(zip(columns, row)) for row in rows]}
        except sqlite3.Error as exc:
            raise LauncherError("SQL-запрос плагина не выполнен") from exc
        finally:
            connection.set_progress_handler(None, 0)
            connection.close()

    if operation == "plugin_database_execute":
        plugin_id = str(request.get("plugin_id", "")).strip()
        _require_plugin_permission(plugin_store, plugin_id, "storage:database")
        connection = plugin_store.database(plugin_id, read_only=False)
        try:
            sql = _safe_plugin_sql(str(request.get("sql", "")), read_only=False)
            parameters = request.get("parameters", [])
            if not isinstance(parameters, list):
                raise LauncherError("Параметры SQL должны быть списком")
            deadline = time.monotonic() + 2.0
            connection.set_progress_handler(lambda: int(time.monotonic() >= deadline), 1000)
            cursor = connection.execute(sql, parameters)
            connection.commit()
            return {"changes": cursor.rowcount if cursor.rowcount >= 0 else 0}
        except sqlite3.Error as exc:
            connection.rollback()
            raise LauncherError("SQL-запрос плагина не выполнен") from exc
        finally:
            connection.set_progress_handler(None, 0)
            connection.close()

    if operation == "remove_instance_asset":
        workspace.remove_asset(
            str(request.get("name", "")),
            str(request.get("kind", "")),
            str(request.get("path", "")),
        )
        name = str(request.get("name", ""))
        snapshot = workspace.snapshot(name)
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Файл ресурса удалён."})
        return snapshot

    if operation == "toggle_mod":
        name = str(request.get("name", ""))
        result = workspace.toggle_mod(
            name,
            str(request.get("path", "")),
            bool(request.get("enabled", False)),
        )
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Состояние мода обновлено."})
        return result

    if operation in {"world_import_begin", "world_import_chunk", "world_import_finish", "world_import_cancel"}:
        from . import world_import

        name = str(request.get("name", "")).strip()
        info = workspace._java_instance(name)
        if bool(getattr(info, "running", False)) and operation != "world_import_cancel":
            raise LauncherError("Остановите игру перед импортом мира")
        if operation == "world_import_begin":
            upload_id = world_import.begin(info.path, str(request.get("kind", "")), str(request.get("label", "")))
            return {"upload_id": upload_id, "chunk_bytes": 8 * 1024 * 1024}
        upload_id = str(request.get("upload_id", ""))
        if operation == "world_import_chunk":
            encoded = str(request.get("data", ""))
            if len(encoded) > 48 * 1024 * 1024:
                raise LauncherError("Слишком большой фрагмент загрузки")
            try:
                content = base64.b64decode(encoded, validate=True)
            except (ValueError, TypeError) as exc:
                raise LauncherError("Фрагмент мира повреждён") from exc
            try:
                offset = int(request.get("offset", 0))
            except (TypeError, ValueError) as exc:
                raise LauncherError("Некорректное смещение фрагмента") from exc
            return world_import.chunk(info.path, upload_id, str(request.get("path", "")), content, offset)
        if operation == "world_import_cancel":
            world_import.cancel(info.path, upload_id)
            return {"cancelled": True}
        imported = world_import.finish(info.path, upload_id)
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": f"Импортировано миров: {len(imported)}."})
        return {"imported": imported, "details": workspace.snapshot(info.name)}

    if operation == "mods_remove_files":
        name = str(request.get("name", "")).strip()
        info = workspace._java_instance(name)
        if bool(getattr(info, "running", False)):
            raise LauncherError("Остановите игру перед удалением модов")
        paths = request.get("paths")
        if not isinstance(paths, list) or not paths:
            raise LauncherError("Не выбрано ни одного мода")
        removed: list[str] = []
        failed: list[dict[str, str]] = []
        for value in paths[:2000]:
            relative = str(value)
            try:
                workspace.remove_asset(name, "mods", relative)
                removed.append(relative)
            except LauncherError as exc:
                failed.append({"path": relative, "error": str(exc)})
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": f"Удалено модов: {len(removed)}."})
        return {"removed": removed, "failed": failed, "details": workspace.snapshot(name)}

    if operation == "import_asset":
        encoded = str(request.get("data", ""))
        if len(encoded) > 360 * 1024 * 1024:
            raise LauncherError("Файл ресурса слишком большой")
        try:
            content = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as exc:
            raise LauncherError("Импортируемый файл повреждён") from exc
        name = str(request.get("name", ""))
        result = workspace.import_asset(
            name,
            str(request.get("kind", "")),
            str(request.get("file_name", "")),
            content,
        )
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Ресурс импортирован."})
        return result

    if operation == "install":
        if bool(request.get("fail_if_exists", False)):
            name = str(request.get("instance_name", "")).strip()
            if not name:
                raise LauncherError("Для нового экземпляра укажите уникальное имя")
            target = instances.root / safe_instance_name(name)
            if os.path.lexists(target):
                raise LauncherError(f"Экземпляр уже существует: {target.name}. Для модпака выберите другое имя.")
        if "java_path" in request:
            config.set_value("java_path", str(request.get("java_path", "")).strip())
        memory = str(request.get("memory", "")).strip()
        if memory:
            config.set_value("memory", memory)
        result = installer.install(
            str(request.get("version", "")),
            loader=str(request.get("loader", "vanilla")),
            loader_version=str(request.get("loader_version", "")) or None,
            instance_name=str(request.get("instance_name", "")) or None,
            assets=not bool(request.get("no_assets", False)),
            replace_existing=bool(request.get("replace_existing", False)),
        )
        return {
            "version_id": result.version_id,
            "instance_dir": str(result.instance_dir),
            "instances": [_instance_public(value) for value in instances.list()],
        }

    if operation == "instance_repair_version":
        name = str(request.get("instance_name", "")).strip()
        if not name:
            raise LauncherError("Сначала выберите экземпляр")
        info = instances.get(name)
        if str(getattr(info, "game_edition", "java")).strip().lower() == "bedrock":
            raise LauncherError("Восстановление версий доступно только для Java-экземпляров")
        if str(getattr(info, "kind", "client")).strip().lower() == "server":
            raise LauncherError("Восстановление доступно только для клиентских экземпляров")
        if bool(getattr(info, "running", False)):
            raise LauncherError("Остановите экземпляр перед восстановлением")
        loader = str(request.get("loader", "")).strip().lower() or str(getattr(info, "loader", "vanilla")).strip().lower() or "vanilla"
        if loader not in {"vanilla", "fabric", "forge", "neoforge", "quilt"}:
            raise LauncherError(f"Неизвестный loader: {loader}")
        minecraft_version = str(request.get("version", "")).strip() or str(getattr(info, "minecraft_version", "")).strip()
        if not minecraft_version:
            raise LauncherError(f"У экземпляра «{info.name}» не определена версия Minecraft")
        loader_version = str(request.get("loader_version", "")).strip() or _instance_loader_version(info) or None
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            result = installer.repair_version(
                info.path,
                minecraft_version,
                loader=loader,
                loader_version=loader_version,
                version_id=str(getattr(info, "version_id", "")).strip() or None,
            )
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Версия и модлоадер восстановлены."})
        return {
            "version_id": result.version_id,
            "instance_dir": str(result.instance_dir),
            "instances": [_instance_public(value) for value in instances.list()],
        }

    if operation == "launch":
        instance_name = str(request.get("instance_name", "")) or None
        if "java_path" in request and not instance_name:
            config.set_value("java_path", str(request.get("java_path", "")).strip())
        memory = str(request.get("memory", "")).strip()
        if memory and not instance_name:
            config.set_value("memory", memory)
        if instance_name:
            info = instances.get(instance_name)
            if info.game_edition == "bedrock":
                return run_bedrock_instance(instance_name)
            if info.running:
                raise LauncherError("Экземпляр уже запущен")
            result = installer.load_existing(info.path, info.version_id)
        else:
            result = installer.install(
                str(request.get("version", "")),
                loader=str(request.get("loader", "vanilla")),
                loader_version=str(request.get("loader_version", "")) or None,
                instance_name=None,
                assets=not bool(request.get("no_assets", False)),
            )
        launch_settings = _effective_launch_settings(workspace, config, instance_name, request)
        account_kind = str(request.get("account", "auto"))
        if account_kind == "auto":
            account_kind = str(launch_settings.get("account_id", "auto"))
        account = _select_account(account_kind, config, accounts)
        session_name = instance_name or result.instance_dir.name
        _session_start(session_name)
        lines: list[str] = []

        def collect(line: str) -> None:
            lines.append(line)
            if len(lines) > _MAX_SESSION_LOG_LINES:
                del lines[:-_MAX_SESSION_LOG_LINES]
            _session_append(session_name, line)
            scoped_emit({"kind": "log", "line": line, "instance_name": session_name})

        try:
            exit_code = launcher.run(
                result,
                account,
                dry_run=bool(request.get("dry_run", False)),
                output=collect,
                settings=launch_settings,
            )
        except Exception as exc:
            _session_finish(session_name)
            _emit_telemetry_report(
                config,
                event="launch_error",
                instance=info if instance_name else None,
                version_id=result.version_id,
                exit_code=None,
                message=str(exc),
                logs=lines,
            )
            scoped_emit({"kind": "status", "event": "operation-finished", "message": "Запуск остановлен или завершился с ошибкой."})
            raise
        _session_finish(session_name)
        crash: dict[str, Any] | None = None
        if exit_code != 0:
            _emit_telemetry_report(
                config,
                event="launch_exit_nonzero",
                instance=info if instance_name else None,
                version_id=result.version_id,
                exit_code=exit_code,
                message=f"Minecraft exited with code {exit_code}",
                logs=lines,
            )
            from .crash_reports import analyze_crash

            crash = analyze_crash(
                lines,
                exit_code,
                instance_path=info.path if info is not None else None,
                memory=str(launch_settings.get("memory", "")),
                minecraft_version=getattr(info, "minecraft_version", "") if info is not None else "",
                loader=getattr(info, "loader", "") if info is not None else "",
            )
        repair = None
        if instance_name:
            record_launch_result(info.path, exit_code)
            if exit_code != 0:
                repair = _auto_repair_report(config, workspace, info, lines, exit_code, launch_settings, since=_SESSION_STARTED.get(session_name, 0.0), account_id=str(getattr(account, "account_id", "")))
        scoped_emit({"kind": "status", "event": "operation-finished", "message": f"Игра завершила работу (код {exit_code})."})
        return {"exit_code": exit_code, "logs": lines[-_MAX_SESSION_LOG_LINES:], "version_id": result.version_id, "crash": crash, "repair": repair}

    if operation == "launch_instance":
        name = str(request.get("name", ""))
        info = instances.get(name)
        if info.game_edition == "bedrock":
            return run_bedrock_instance(name)
        if info.running:
            raise LauncherError("Экземпляр уже запущен")
        launch_settings = _effective_launch_settings(workspace, config, name, request)
        world = str(request.get("world", "")).strip()
        server = str(request.get("server", "")).strip()
        if world:
            launch_settings["quick_play_world"] = world
            launch_settings["quick_play_server"] = ""
        elif server:
            launch_settings["quick_play_server"] = server
            launch_settings["quick_play_world"] = ""
        account_kind = str(request.get("account", "auto"))
        if account_kind == "auto":
            account_kind = str(launch_settings.get("account_id", "auto"))
        account = _select_account(account_kind, config, accounts)
        result = installer.load_existing(info.path, info.version_id)
        _session_start(name)
        lines: list[str] = []

        def collect_existing(line: str) -> None:
            lines.append(line)
            if len(lines) > _MAX_SESSION_LOG_LINES:
                del lines[:-_MAX_SESSION_LOG_LINES]
            _session_append(name, line)
            scoped_emit({"kind": "log", "line": line, "instance_name": name})

        try:
            exit_code = launcher.run(
                result,
                account,
                dry_run=bool(request.get("dry_run", False)),
                output=collect_existing,
                settings=launch_settings,
            )
        except Exception as exc:
            _session_finish(name)
            _emit_telemetry_report(
                config,
                event="launch_error",
                instance=info,
                version_id=result.version_id,
                exit_code=None,
                message=str(exc),
                logs=lines,
            )
            scoped_emit({"kind": "status", "event": "operation-finished", "message": "Запуск остановлен или завершился с ошибкой."})
            raise
        _session_finish(name)
        record_launch_result(info.path, exit_code)
        crash_existing: dict[str, Any] | None = None
        repair_existing: dict[str, Any] | None = None
        if exit_code != 0:
            _emit_telemetry_report(
                config,
                event="launch_exit_nonzero",
                instance=info,
                version_id=result.version_id,
                exit_code=exit_code,
                message=f"Minecraft exited with code {exit_code}",
                logs=lines,
            )
            crash_existing = analyze_crash(
                lines,
                exit_code,
                instance_path=info.path,
                memory=str(launch_settings.get("memory", "")),
                minecraft_version=str(getattr(info, "minecraft_version", "")),
                loader=str(getattr(info, "loader", "")),
            )
            repair_existing = _auto_repair_report(config, workspace, info, lines, exit_code, launch_settings, since=_SESSION_STARTED.get(name, 0.0), account_id=str(getattr(account, "account_id", "")))
        scoped_emit({"kind": "status", "event": "operation-finished", "message": f"Игра завершила работу (код {exit_code})."})
        return {
            "exit_code": exit_code,
            "logs": lines[-_MAX_SESSION_LOG_LINES:],
            "version_id": result.version_id,
            "crash": crash_existing,
            "repair": repair_existing,
        }

    if operation == "auto_repair_diagnose":
        name = str(request.get("instance_name", "")).strip()
        if not name:
            raise LauncherError("Сначала выберите экземпляр")
        manual = bool(request.get("manual", False))
        if not manual and not config.settings.auto_repair_enabled:
            return {"instance_name": name, "issues": [], "muted_issues": [], "disabled": True}
        info = instances.get(name)
        lines, running = _session_snapshot(info.name)
        if running:
            raise LauncherError("Экземпляр ещё запущен — диагностика будет доступна после выхода из игры")
        raw_exit = request.get("exit_code")
        try:
            exit_code = int(raw_exit) if raw_exit is not None else None
        except (TypeError, ValueError):
            exit_code = None
        try:
            launch_settings = _effective_launch_settings(workspace, config, info.name, {})
        except LauncherError:
            launch_settings = {}
        report = _auto_repair_report(
            config,
            workspace,
            info,
            lines if (exit_code is not None or request.get("error")) else [],
            exit_code,
            launch_settings,
            error=str(request.get("error", ""))[:4000],
            since=_SESSION_STARTED.get(info.name, 0.0),
            manual=manual,
        )
        if report is None:
            raise LauncherError("Авто-починка доступна только для клиентских Java-экземпляров")
        return report

    if operation == "auto_repair_apply":
        name = str(request.get("instance_name", "")).strip()
        if not name:
            raise LauncherError("Сначала выберите экземпляр")
        info = instances.get(name)
        if str(getattr(info, "kind", "client")).strip().lower() != "client" or str(getattr(info, "game_edition", "java")).strip().lower() != "java":
            raise LauncherError("Авто-починка доступна только для клиентских Java-экземпляров")
        if bool(getattr(info, "running", False)):
            raise LauncherError("Остановите экземпляр перед починкой")
        fixes = request.get("fixes")
        if not isinstance(fixes, list):
            raise LauncherError("Не выбрано ни одной починки")

        def repair_version() -> None:
            loader = str(getattr(info, "loader", "vanilla")).strip().lower() or "vanilla"
            minecraft_version = str(getattr(info, "minecraft_version", "")).strip()
            if not minecraft_version:
                raise LauncherError(f"У экземпляра «{info.name}» не определена версия Minecraft")
            installer.repair_version(
                info.path,
                minecraft_version,
                loader=loader,
                loader_version=_instance_loader_version(info) or None,
                version_id=str(getattr(info, "version_id", "")).strip() or None,
            )

        def install_mod(project_id: str) -> None:
            game_version, loader = _instance_mod_context(info)
            catalog = next((item for item in config.settings.resource_catalogs if item.get("kind") == "modrinth"), None)
            if not catalog:
                raise LauncherError("Каталог Modrinth не настроен")
            provider = ModrinthProvider(http, catalog["api_url"], catalog_id=catalog["id"])
            provider.install(project_id, info.path, game_version, loader, project_type="mod")

        def update_loader(minimum: str) -> str:
            return _update_instance_loader(installer, info, minimum)

        def set_account(account_id: str) -> str:
            account = accounts.set_default(account_id)
            config.set_value("default_account_id", account.account_id)
            return account.alias or account.username or account.account_id

        context = RepairContext(
            instance_name=info.name,
            instance_path=info.path,
            save_settings=lambda values: workspace.save_settings(info.name, values),
            settings=lambda: workspace.settings(info.name),
            toggle_mod=lambda file_name: workspace.toggle_mod(info.name, file_name, False),
            repair_version=repair_version,
            install_mod=install_mod,
            update_loader=update_loader,
            set_account=set_account,
        )
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            results = apply_fixes(context, fixes)
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Авто-починка применена."})
        return {
            "instance_name": info.name,
            "results": results,
            "instances": [_instance_public(value) for value in instances.list()],
        }

    if operation == "auto_repair_mute":
        kinds = normalize_muted(request.get("kinds"))
        muted = list(config.settings.auto_repair_muted)
        if bool(request.get("muted", True)):
            muted.extend(kind for kind in kinds if kind not in muted)
        else:
            muted = [kind for kind in muted if kind not in kinds]
        config.set_value("auto_repair_muted", muted)
        return {"settings": _settings_public(config)}

    if operation == "open_folder":
        launcher.open_folder(Path(str(request.get("path", ""))).expanduser() if request.get("path") else None)
        return {"opened": True}

    if operation == "modpack_import":
        importer = ModpackImporter(http)
        file_name = str(request.get("file_name", "modpack.zip"))
        content = _decode_base64(request.get("data"), 512 * 1024 * 1024, "Архив модпака")
        confirm_unverified = bool(request.get("confirm_unverified", False))
        expected_game_version = str(request.get("game_version", "")).strip()
        expected_loader = str(request.get("loader", "")).strip().lower()
        expected_loader_version = str(request.get("loader_version", "")).strip()
        if bool(request.get("preflight_only", False)):
            dependencies = importer.preflight_bytes(
                content,
                file_name=file_name,
                confirm_unverified=confirm_unverified,
                expected_game_version=expected_game_version,
                expected_loader=expected_loader,
                expected_loader_version=expected_loader_version,
            )
            # Предпросмотр модов: что именно окажется в папке mods после
            # импорта. Ошибка превью не должна блокировать сам импорт.
            try:
                dependencies["mods"] = importer.mod_preview_bytes(content, file_name=file_name)
            except LauncherError:
                pass
            return {"preflight": True, "dependencies": dependencies}
        name = str(request.get("instance_name", "")).strip()
        if not name:
            raise LauncherError("Сначала выберите экземпляр")
        info = instances.get(name)
        if info.kind == "server":
            raise LauncherError("Импорт модпака доступен только для клиентского экземпляра")
        instance_version, instance_loader = _instance_mod_context(info)
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            result = importer.import_bytes(
                content,
                info.path,
                file_name=file_name,
                source="local",
                project_id=str(request.get("project_id", "")),
                display_name=str(request.get("display_name", "")),
                version=str(request.get("version", "")),
                confirm_unverified=confirm_unverified,
                expected_game_version=instance_version,
                expected_loader=instance_loader or "vanilla",
                expected_loader_version=_instance_loader_version(info) or None,
            )
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Модпак импортирован."})
        return {**result, "instance_name": name, "details": workspace.snapshot(name)}

    if operation == "mods_details":
        project_id = str(request.get("project_id", "")).strip()
        project_type = str(request.get("project_type", "mod")).strip().lower()
        instance_name = str(request.get("instance_name", "")).strip()
        info = instances.get(instance_name) if instance_name else None
        if info is not None:
            game_version, loader = _instance_mod_context(info)
            if project_type == "plugin" and getattr(info, "kind", "client") != "server":
                raise LauncherError("Каталог плагинов доступен только для серверных экземпляров")
            if project_type != "plugin" and getattr(info, "kind", "client") == "server":
                raise LauncherError("Каталог ресурсов доступен только для клиентских экземпляров")
        else:
            game_version = str(request.get("game_version", "")).strip()
            loader_value = str(request.get("loader", "vanilla")).strip().lower() or "vanilla"
            loader = None if loader_value == "vanilla" else loader_value
        if project_type == "plugin":
            catalog = _plugin_catalog(config, str(request.get("catalog_id", "")))
            core = str(request.get("server_core", "") or getattr(info, "server_core", "") or "paper").strip().lower()
            details = PluginCatalog(http, catalog).details(project_id, core, game_version)
            return {"details": {**details, "project_type": "plugin"}, "compatible_versions": [], "catalog_id": catalog["id"]}
        catalog = _resource_catalog(config, str(request.get("catalog_id", "")))
        if project_type == "modpack":
            details = ModpackProvider(http, catalog).project_details(project_id)
            return {"details": details, "catalog_id": catalog["id"], "compatible_versions": []}
        if catalog.get("kind") in {"ftb", "technic"}:
            raise LauncherError("Этот каталог поддерживает только подробности модпаков")
        provider = ModrinthProvider(http, catalog["api_url"], catalog_id=catalog["id"])
        details = provider.project_details(project_id, project_type)
        versions = provider.compatible_versions(project_id, game_version, loader, project_type)
        return {
            "details": details,
            "compatible_versions": [
                {
                    "id": str(value.get("id") or ""),
                    "version_number": str(value.get("version_number") or ""),
                    "name": str(value.get("name") or ""),
                    "date_published": str(value.get("date_published") or ""),
                    "game_versions": value.get("game_versions", []),
                    "loaders": value.get("loaders", []),
                }
                for value in versions
            ],
            "catalog_id": catalog["id"],
        }

    # overlay_state обслуживается до этого роутера (rust_engine/state.rs).

    if operation == "recording_start":
        summary = game_recorder.start(config, str(request.get("instance_name", "")).strip())
        scoped_emit({"kind": "status", "event": "recording-changed", "message": "Запись игры начата"})
        return summary

    if operation == "recording_stop":
        summary = game_recorder.stop()
        scoped_emit({"kind": "status", "event": "recording-changed", "message": "Запись игры остановлена"})
        return summary

    if operation == "recording_status":
        return game_recorder.status()

    if operation == "system_notify":
        # Системные уведомления отправляет только свёрнутый лаунчер (решает UI).
        title = str(request.get("title", "")).strip()[:120]
        body = str(request.get("body", "")).strip()[:400]
        notifier = shutil.which("notify-send")
        if not notifier or not (title or body):
            return {"sent": False}
        try:
            subprocess.run(
                [notifier, "--app-name", "Nivora Launcher", "--expire-time", "8000", title or "Nivora Launcher", body or title],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                stdin=subprocess.DEVNULL,
                timeout=5,
                check=False,
            )
        except (OSError, subprocess.SubprocessError):
            return {"sent": False}
        return {"sent": True}

    if operation == "mods_search":
        instance_name = str(request.get("instance_name", "")).strip()
        info = instances.get(instance_name) if instance_name else None
        if info is not None and str(getattr(info, "game_edition", "java")).strip().lower() == "bedrock":
            # Bedrock-профиль не может принимать Java-контент: ищем по явной версии,
            # а не ломаем каталог ошибкой выбранного экземпляра.
            info = None
            instance_name = ""
        project_type = str(request.get("project_type", "mod")).strip().lower()
        # Постраничный каталог: page с нуля, limit — размер страницы.
        try:
            page = max(0, min(500, int(request.get("page", 0) or 0)))
            page_size = max(1, min(30, int(request.get("limit", 12))))
        except (TypeError, ValueError) as exc:
            raise LauncherError("Номер страницы каталога должен быть числом") from exc
        offset = page * page_size
        # У каталогов без смещения забираем начало выдачи и режем локально.
        local_window = min(30, offset + page_size)
        total: int | None = None
        local_sliced = False
        if info is not None:
            instance_kind = str(getattr(info, "kind", "client")).strip().lower()
            if project_type == "plugin" and instance_kind != "server":
                raise LauncherError("Каталог плагинов доступен только для серверных экземпляров")
            if project_type != "plugin" and instance_kind == "server":
                raise LauncherError("Каталог ресурсов доступен только для клиентских экземпляров")
            game_version, loader = _instance_mod_context(info)
        else:
            game_version = str(request.get("game_version", "")).strip()
            loader_value = str(request.get("loader", "vanilla")).strip().lower() or "vanilla"
            if not game_version:
                raise LauncherError("Укажите версию Minecraft для поиска")
            loader = None if loader_value == "vanilla" else loader_value
        if project_type == "plugin":
            catalog = _plugin_catalog(config, str(request.get("catalog_id", "")))
            core = str(request.get("server_core", "") or getattr(info, "server_core", "") or "paper").strip().lower()
            provider = PluginCatalog(http, catalog)
            values = [
                {**item.as_dict(), "project_type": "plugin"}
                for item in provider.search(
                    str(request.get("query", "")),
                    core,
                    game_version,
                    local_window if page else page_size,
                    sort_by=str(request.get("sort_by", "relevance")),
                )
            ]
            local_sliced = True
        else:
            catalog = _resource_catalog(config, str(request.get("catalog_id", "")))
        if project_type == "modpack":
            provider = ModpackProvider(http, catalog)
            native_offset = str(catalog.get("kind", "modrinth")).strip().lower() == "modrinth"
            if page and native_offset:
                found = provider.search(str(request.get("query", "")), game_version, page_size, offset=offset)
            else:
                found = provider.search(str(request.get("query", "")), game_version, local_window if page else page_size)
                local_sliced = not native_offset
            values = [item.as_dict() for item in found]
            total = getattr(provider, "last_total", None) if native_offset else None
            values = _sort_project_results(values, str(request.get("sort_by", "relevance")))
        elif project_type != "plugin":
            if catalog.get("kind") in {"ftb", "technic"}:
                raise LauncherError("Каталоги FTB и Technic предназначены для поиска модпаков")
            provider = ModrinthProvider(http, catalog["api_url"])
            values = provider.search(
                str(request.get("query", "")),
                game_version,
                loader,
                page_size,
                project_type=project_type,
                sort_by=str(request.get("sort_by", "relevance")),
                **({"offset": offset} if offset else {}),
            )
            total = getattr(provider, "last_total", None)
        if local_sliced:
            fetched = len(values)
            values = values[offset : offset + page_size] if page else values
            has_more = fetched >= local_window and offset + page_size < 30
        elif isinstance(total, int):
            has_more = offset + len(values) < total
        else:
            has_more = len(values) >= page_size
        installed = (
            workspace.installed_projects(instance_name, project_type, catalog_id=str(catalog["id"]))
            if instance_name and project_type != "plugin"
            else set()
        )
        installed_plugins = (
            {
                item["project_id"]: item
                for item in PluginCatalog(http, catalog).installed(info.path / "plugins")
            }
            if instance_name and project_type == "plugin" and info is not None
            else {}
        )
        values = [
            {
                **value,
                "icon_url": str(value.get("icon_url") or value.get("icon") or value.get("logo_url") or ""),
                "installed": (
                    str(value.get("project_id") or value.get("id") or "") in installed_plugins
                    if project_type == "plugin"
                    else str(value.get("project_id") or value.get("id") or "") in installed
                ),
                "file_name": installed_plugins.get(str(value.get("project_id") or value.get("id") or ""), {}).get("file_name")
                or value.get("file_name", ""),
                "catalog_id": catalog["id"],
            }
            for value in values
            if isinstance(value, dict)
        ]
        return {
            "results": values,
            "page": page,
            "page_size": page_size,
            "total": total if isinstance(total, int) else None,
            "has_more": bool(has_more),
            "instance_name": instance_name,
            "game_version": game_version,
            "loader": loader or "vanilla",
            "catalog_id": catalog["id"],
            "server_core": str(request.get("server_core", "") or getattr(info, "server_core", "") or "paper"),
        }

    if operation == "mods_check_updates":
        name = str(request.get("instance_name", "")).strip()
        project_type = str(request.get("project_type", "mod")).strip().lower()
        if project_type not in {"mod", "resourcepack", "shader", "datapack"}:
            raise LauncherError("Проверка обновлений доступна для модов, ресурспаков, шейдеров и датапаков")
        info = instances.get(name)
        if getattr(info, "kind", "client") == "server":
            raise LauncherError("Выберите клиентский экземпляр для проверки ресурсов")
        game_version, loader = _instance_mod_context(info)
        requested_catalog_id = str(request.get("catalog_id", "")).strip()
        catalogs = (
            [_resource_catalog(config, requested_catalog_id)]
            if requested_catalog_id
            else [catalog for catalog in config.settings.resource_catalogs if catalog.get("kind") == "modrinth"]
        )
        if not catalogs or any(catalog.get("kind") != "modrinth" for catalog in catalogs):
            raise LauncherError("Проверка обновлений доступна только для установок Modrinth")
        updates: list[dict[str, Any]] = []
        errors: list[str] = []
        worlds = workspace.list_worlds(name) if project_type == "datapack" else []
        for catalog in catalogs:
            provider = ModrinthProvider(http, catalog["api_url"], catalog_id=catalog["id"])
            try:
                if project_type == "datapack":
                    for world in worlds:
                        world_id = str(world.get("id", ""))
                        if not world_id:
                            continue
                        target = workspace.world_directory(name, world_id)
                        updates.extend(
                            {**item, "world_name": world_id, "world_title": str(world.get("name") or world_id)}
                            for item in provider.check_updates(
                                target,
                                game_version,
                                loader,
                                project_type,
                                target_dir=target,
                            )
                        )
                else:
                    updates.extend(provider.check_updates(info.path, game_version, loader, project_type))
            except OperationCancelled:
                raise
            except LauncherError as exc:
                errors.append(f"{catalog['name']}: {exc}")
        active_catalog_id = str(config.settings.active_catalog_id)
        result_catalog_id = next(
            (catalog["id"] for catalog in catalogs if catalog["id"] == active_catalog_id),
            catalogs[0]["id"],
        )
        if requested_catalog_id:
            result_catalog_id = catalogs[0]["id"]
        return {
            "updates": updates,
            "errors": errors,
            "project_type": project_type,
            "instance_name": name,
            "catalog_id": result_catalog_id,
        }

    if operation == "mods_update":
        name = str(request.get("instance_name", "")).strip()
        project_type = str(request.get("project_type", "mod")).strip().lower()
        if project_type not in {"mod", "resourcepack", "shader", "datapack"}:
            raise LauncherError("Обновление доступно только для ресурсов Modrinth")
        info = instances.get(name)
        if getattr(info, "kind", "client") == "server":
            raise LauncherError("Выберите клиентский экземпляр для обновления ресурса")
        game_version, loader = _instance_mod_context(info)
        catalog = _resource_catalog(config, str(request.get("catalog_id", "")))
        if catalog.get("kind") != "modrinth":
            raise LauncherError("Обновление доступно только для установок Modrinth")
        world_id = str(request.get("world_name", "")).strip()
        target = workspace.world_directory(name, world_id) if project_type == "datapack" else None
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            result = ModrinthProvider(http, catalog["api_url"], catalog_id=catalog["id"]).install(
                str(request.get("project_id", "")),
                info.path,
                game_version,
                loader,
                project_type=project_type,
                target_dir=target,
                confirm_unverified=bool(request.get("confirm_unverified", False)),
                version_id=str(request.get("version_id", "")).strip(),
                allow_replace=True,
            )
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": f"Ресурс «{result.name}» обновлён."})
        return {
            "project_id": result.project_id,
            "name": result.name,
            "version": result.version,
            "version_id": result.version_id,
            "file_name": result.file_name,
            "dependencies": result.dependencies,
            "details": workspace.snapshot(name),
            "instance_name": name,
        }

    if operation == "mods_install":
        name = str(request.get("instance_name", ""))
        project_type = str(request.get("project_type", "mod")).strip().lower()
        if project_type == "modpack" and bool(request.get("preflight_only", False)):
            catalog = _resource_catalog(config, str(request.get("catalog_id", "")))
            dependencies = ModpackProvider(http, catalog).preflight(
                str(request.get("project_id", "")),
                str(request.get("game_version", "")).strip(),
                version=str(request.get("version", "")),
                loader=str(request.get("loader", "")).strip().lower(),
                loader_version=str(request.get("loader_version", "")).strip(),
                confirm_unverified=bool(request.get("confirm_unverified", False)),
            )
            return {"preflight": True, "dependencies": dependencies}
        info = instances.get(name)
        if project_type == "plugin":
            if str(getattr(info, "kind", "client")).strip().lower() != "server":
                raise LauncherError("Плагины можно устанавливать только в серверные экземпляры")
            game_version, _ = _instance_mod_context(info)
            catalog = _plugin_catalog(config, str(request.get("catalog_id", "")))
            process_lock = InstanceProcessLock(config.paths.data_home, info.name)
            process_lock.acquire()
            try:
                result = PluginCatalog(http, catalog).install(
                    str(request.get("project_id", "")),
                    info.path / "plugins",
                    game_version,
                    str(getattr(info, "server_core", "paper") or "paper"),
                    confirm_unverified=bool(request.get("confirm_unverified", False)),
                )
            finally:
                process_lock.release()
            scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Плагин установлен в сервер."})
            return {**result, "catalog_id": catalog["id"], "instance_name": name}
        if str(getattr(info, "kind", "client")).strip().lower() == "server":
            raise LauncherError("Ресурсы можно устанавливать только в клиентские экземпляры")
        game_version, loader = _instance_mod_context(info)
        catalog = _resource_catalog(config, str(request.get("catalog_id", "")))
        if project_type == "modpack":
            process_lock = InstanceProcessLock(config.paths.data_home, info.name)
            process_lock.acquire()
            try:
                result = ModpackProvider(http, catalog).install(
                    str(request.get("project_id", "")),
                    info.path,
                    game_version,
                    version=str(request.get("version", "")),
                    display_name=str(request.get("display_name", "")),
                    loader=loader or "vanilla",
                    loader_version=_instance_loader_version(info) or None,
                    confirm_unverified=bool(request.get("confirm_unverified", False)),
                )
            finally:
                process_lock.release()
        else:
            if catalog.get("kind") in {"ftb", "technic"}:
                raise LauncherError("Каталоги FTB и Technic предназначены для установки модпаков")
            provider = ModrinthProvider(http, catalog["api_url"], catalog_id=catalog["id"])
            process_lock = InstanceProcessLock(config.paths.data_home, info.name)
            process_lock.acquire()
            try:
                result = provider.install(
                    str(request.get("project_id", "")),
                    info.path,
                    game_version,
                    loader,
                    project_type=project_type,
                    target_dir=workspace.world_directory(name, str(request.get("world_name", ""))) if project_type == "datapack" else None,
                    confirm_unverified=bool(request.get("confirm_unverified", False)),
                    version_id=str(request.get("version_id", "")).strip() or None,
                )
            finally:
                process_lock.release()
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Ресурс установлен."})
        if project_type == "modpack":
            return {**result, "catalog_id": catalog["id"], "details": workspace.snapshot(name)}
        return {
            "source": result.source,
            "project_id": result.project_id,
            "name": result.name,
            "version": result.version,
            "version_id": result.version_id,
            "file_name": result.file_name,
            "dependencies": result.dependencies,
            "path": str(result.path or ""),
            "catalog_id": catalog["id"],
            "details": workspace.snapshot(name),
        }

    if operation == "mods_remove":
        name = str(request.get("instance_name", "")).strip()
        project_type = str(request.get("project_type", "mod")).strip().lower()
        if project_type == "modpack":
            raise LauncherError("Удаление целого модпака через каталог не поддерживается. Удалите или восстановите файлы экземпляра отдельно.")
        info = instances.get(name)
        if bool(getattr(info, "running", False)):
            raise LauncherError("Остановите экземпляр перед удалением ресурса")
        process_lock = InstanceProcessLock(config.paths.data_home, info.name)
        process_lock.acquire()
        try:
            details, removed = workspace.remove_installed_project(
                name,
                project_type,
                str(request.get("project_id", "")),
                catalog_id=str(request.get("catalog_id", "")),
            )
        finally:
            process_lock.release()
        scoped_emit({"kind": "status", "event": "workspace-changed", "message": "Ресурс удалён."})
        return {"removed": removed, "instance_name": info.name, "details": details}

    if operation in {"mod_audit_prepare", "mod_audit_accept", "mod_audit_cancel", "mod_audit_status"}:
        audit_store = ModAuditStore(config.paths.data_home / "mod-audit")

        if operation == "mod_audit_prepare":
            target, label, kind = _mod_audit_target(instances, request)
            payload = audit_store.prepare(target, label=label, kind=kind)
            scoped_emit(
                {
                    "kind": "status",
                    "event": "mod-audit-prepared",
                    "message": f"Файл декомпилирован для проверки: {label}",
                    "session_id": payload["session_id"],
                }
            )
            return payload

        session_id = str(request.get("session_id", "")).strip()
        if operation == "mod_audit_cancel":
            result = audit_store.cancel(session_id)
            scoped_emit(
                {
                    "kind": "status",
                    "event": "mod-audit-cancelled",
                    "message": "Декомпилированные исходники уничтожены без проверки.",
                    "session_id": session_id,
                }
            )
            return result
        if operation == "mod_audit_status":
            return audit_store.status(session_id)

        def audit_progress(percent: int, source: str) -> None:
            scoped_emit(
                {
                    "kind": "status",
                    "event": "mod-audit-progress",
                    "message": f"Проверка исходников: {percent}%",
                    "percent": percent,
                    "session_id": session_id,
                }
            )

        report = audit_store.accept(
            session_id,
            progress=audit_progress,
            cancelled=cancellation.cancelled,
        )
        scoped_emit(
            {
                "kind": "status",
                "event": "mod-audit-finished",
                "message": f"Проверка завершена: {report.get('verdict_label', '')}",
                "session_id": session_id,
            }
        )
        return {"report": report}

    if operation == "skins_list":
        values = SkinStore(config).list()
        return {
            "skins": [
                {
                    "skin_id": value.skin_id,
                    "name": value.name,
                    "model": value.model,
                    "path": str(value.path),
                    "image": value.image,
                }
                for value in values
            ],
            "folder": str(SkinStore(config).root),
        }

    if operation == "skins_import":
        encoded = str(request.get("data", ""))
        try:
            content = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as exc:
            raise LauncherError("Файл скина повреждён") from exc
        if len(content) > 16 * 1024 * 1024:
            raise LauncherError("Файл скина слишком большой")
        value = SkinStore(config).import_bytes(content, str(request.get("file_name", "skin.png")), model=str(request.get("model", "classic")))
        return {"skin": {"skin_id": value.skin_id, "name": value.name, "model": value.model, "path": str(value.path), "image": value.image}}

    if operation == "skin_profile":
        account = _microsoft_account(str(request.get("account_id", "")), config, accounts, http)
        return fetch_skin_profile(
            http,
            account,
            refresh_account=lambda: MicrosoftAuthenticator(config, accounts, http).get_valid(
                account.account_id,
                force_refresh=True,
            ),
        )

    if operation == "apply_skin":
        account = _microsoft_account(str(request.get("account_id", "")), config, accounts, http)
        skin = SkinStore(config).get(str(request.get("skin_id", "")))
        apply_skin(http, account, skin, str(request.get("model", skin.model)))
        return {"ok": True}

    if operation == "clear_skin":
        account = _microsoft_account(str(request.get("account_id", "")), config, accounts, http)
        clear_skin(http, account)
        return {"ok": True}

    if operation == "apply_cape":
        account = _microsoft_account(str(request.get("account_id", "")), config, accounts, http)
        apply_cape(http, account, str(request.get("cape_id", "")))
        return {"ok": True}

    if operation == "clear_cape":
        account = _microsoft_account(str(request.get("account_id", "")), config, accounts, http)
        clear_cape(http, account)
        return {"ok": True}

    if operation == "open_skins_folder":
        root = SkinStore(config).root
        try:
            open_path(root)
        except (OSError, RuntimeError) as exc:
            raise LauncherError(f"Не удалось открыть папку скинов: {exc}") from exc
        return {"opened": True}

    raise LauncherError(f"Неизвестная GUI-операция: {operation}")


def _require_plugin_permission(store: PluginStore, plugin_id: str, permission: str) -> dict[str, Any]:
    return store.authorize_operation(plugin_id, permission)


def _require_plugin_permission_any(store: PluginStore, plugin_id: str, permissions: list[str]) -> dict[str, Any]:
    last_error: LauncherError | None = None
    for permission in permissions:
        try:
            return store.authorize_operation(plugin_id, permission)
        except LauncherError as exc:
            last_error = exc
    assert last_error is not None
    raise last_error


def _plugin_network_host(manifest: dict[str, Any], value: Any) -> str:
    host = str(value or "").strip().lower().rstrip(".")
    if not host or len(host) > 253 or any(char.isspace() for char in host) or "/" in host or "@" in host:
        raise LauncherError("Некорректное имя хоста плагина")
    try:
        ipaddress.ip_address(host)
    except ValueError:
        pass
    else:
        raise LauncherError("Плагинам запрещены IP-адреса; укажите DNS-имя")
    if host not in set(manifest.get("network_hosts", [])):
        raise LauncherError("Хост не входит в allowlist плагина")
    return host


def _resolve_public_plugin_host(
    manifest: dict[str, Any],
    host: str,
    port: int | None,
) -> list[tuple[int, int, int, str, tuple[Any, ...]]]:
    host = _plugin_network_host(manifest, host)
    try:
        records = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except socket.gaierror as exc:
        raise LauncherError(f"DNS не разрешил {host}: {exc}") from exc
    if not records:
        raise LauncherError(f"DNS не вернул адреса для {host}")
    for record in records:
        address = str(record[4][0]).split("%", 1)[0]
        try:
            parsed = ipaddress.ip_address(address)
        except ValueError as exc:
            raise LauncherError(f"DNS вернул некорректный адрес для {host}") from exc
        if not parsed.is_global:
            raise LauncherError(f"Плагинам запрещены localhost и private network: {host} ({parsed})")
    return records


def _plugin_port(value: Any, *, allow_empty: bool) -> int | None:
    if value is None or str(value).strip() == "":
        return None if allow_empty else 25565
    try:
        port = int(value)
    except (TypeError, ValueError) as exc:
        raise LauncherError("Порт должен быть числом от 1 до 65535") from exc
    if not 1 <= port <= 65535:
        raise LauncherError("Порт должен быть числом от 1 до 65535")
    return port


def _plugin_timeout_ms(value: Any) -> int:
    try:
        timeout = int(value)
    except (TypeError, ValueError) as exc:
        raise LauncherError("Timeout должен быть числом от 100 до 5000 мс") from exc
    if not 100 <= timeout <= 5000:
        raise LauncherError("Timeout должен быть числом от 100 до 5000 мс")
    return timeout


def _plugin_server_entry(value: Any, *, fallback_id: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise LauncherError("Сервер плагина должен быть объектом")
    server_id = str(value.get("id", fallback_id)).strip()
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,80}", server_id):
        raise LauncherError("Некорректный id сервера плагина")
    name = str(value.get("name", "")).strip()
    address = str(value.get("address", "")).strip().strip("[]")
    if not name or len(name) > 120:
        raise LauncherError("Имя сервера плагина обязательно и должно быть не длиннее 120 символов")
    if not address or len(address) > 253 or any(char.isspace() for char in address) or "/" in address or "@" in address:
        raise LauncherError("Некорректный адрес сервера плагина")
    port = _plugin_port(value.get("port"), allow_empty=False)
    tags = value.get("tags", [])
    if not isinstance(tags, list):
        raise LauncherError("Теги сервера плагина должны быть списком")
    normalized_tags = [str(tag).strip()[:40] for tag in tags if str(tag).strip()][:16]
    return {
        "id": server_id,
        "name": name,
        "address": address,
        "port": port,
        "description": str(value.get("description", "")).strip()[:500],
        "tags": normalized_tags,
    }


def _plugin_server_list(value: Any) -> list[dict[str, Any]]:
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > 200:
        raise LauncherError("Список серверов плагина должен содержать не более 200 записей")
    result: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, item in enumerate(value):
        entry = _plugin_server_entry(item, fallback_id=f"server-{index + 1}")
        if entry["id"] in seen:
            raise LauncherError(f"Повторяющийся id сервера: {entry['id']}")
        seen.add(entry["id"])
        result.append(entry)
    return result


def _plugin_proxy_route_entry(value: Any, *, fallback_id: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise LauncherError("Proxy route policy плагина должна быть объектом")
    route_id = str(value.get("id", fallback_id)).strip()
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,80}", route_id):
        raise LauncherError("Proxy route policy содержит небезопасный id")
    priority_value = value.get("priority", 100)
    if type(priority_value) not in {int, float}:
        raise LauncherError("Proxy route policy.priority должен быть числом")
    priority = int(priority_value)
    if priority < 0 or priority > 100000:
        raise LauncherError("Proxy route policy.priority вне допустимого диапазона")
    match = value.get("match", {})
    if not isinstance(match, dict):
        raise LauncherError("Proxy route policy.match должен быть объектом")
    action = value.get("action", {})
    if not isinstance(action, dict):
        raise LauncherError("Proxy route policy.action должен быть объектом")
    action_type = str(action.get("type", "")).strip().upper()
    if action_type not in {"DIRECT", "PROXY", "RELAY", "DROP", "MIRROR"}:
        raise LauncherError("Proxy route policy.action.type не поддерживается")

    def _text_list(raw: Any, label: str, *, maximum: int = 64) -> list[str]:
        if raw is None:
            return []
        if not isinstance(raw, list) or len(raw) > maximum:
            raise LauncherError(f"{label} должен быть списком ограниченного размера")
        values: list[str] = []
        for item in raw:
            text = str(item).strip()
            if not text or len(text) > 253:
                raise LauncherError(f"{label} содержит некорректное значение")
            values.append(text)
        return values

    def _port_list(raw: Any, label: str) -> list[int]:
        if raw is None:
            return []
        if not isinstance(raw, list) or len(raw) > 64:
            raise LauncherError(f"{label} должен быть списком портов")
        ports: list[int] = []
        for item in raw:
            port = _plugin_port(item, allow_empty=False)
            assert port is not None
            ports.append(port)
        return ports

    normalized_match = {
        "hostEquals": _text_list(match.get("hostEquals"), "Proxy route hostEquals"),
        "hostSuffix": _text_list(match.get("hostSuffix"), "Proxy route hostSuffix"),
        "portIn": _port_list(match.get("portIn"), "Proxy route portIn"),
        "protocol": [item.lower() for item in _text_list(match.get("protocol"), "Proxy route protocol", maximum=8)],
        "sniEquals": _text_list(match.get("sniEquals"), "Proxy route sniEquals"),
        "processTag": _text_list(match.get("processTag"), "Proxy route processTag"),
        "instanceId": _text_list(match.get("instanceId"), "Proxy route instanceId"),
    }
    for item in normalized_match["protocol"]:
        if item not in {"tcp", "udp", "quic"}:
            raise LauncherError("Proxy route protocol поддерживает только tcp/udp/quic")

    normalized_action: dict[str, Any] = {"type": action_type}
    if "sessionId" in action:
        session_id = str(action.get("sessionId", "")).strip()
        if not re.fullmatch(r"[A-Za-z0-9._:-]{1,120}", session_id):
            raise LauncherError("Proxy route sessionId содержит небезопасное значение")
        normalized_action["sessionId"] = session_id
    rewrite = action.get("rewrite")
    if rewrite is not None:
        if not isinstance(rewrite, dict):
            raise LauncherError("Proxy route rewrite должен быть объектом")
        normalized_rewrite: dict[str, Any] = {}
        if "targetHost" in rewrite:
            target_host = str(rewrite.get("targetHost", "")).strip()
            if not target_host or len(target_host) > 253:
                raise LauncherError("Proxy route rewrite.targetHost содержит некорректный host")
            normalized_rewrite["targetHost"] = target_host
        if "targetPort" in rewrite:
            normalized_rewrite["targetPort"] = _plugin_port(rewrite.get("targetPort"), allow_empty=False)
        if "overrideSni" in rewrite:
            override_sni = str(rewrite.get("overrideSni", "")).strip()
            if not override_sni or len(override_sni) > 253:
                raise LauncherError("Proxy route rewrite.overrideSni содержит некорректное значение")
            normalized_rewrite["overrideSni"] = override_sni
        if "addHeaders" in rewrite:
            add_headers = rewrite.get("addHeaders")
            if not isinstance(add_headers, dict) or len(add_headers) > 32:
                raise LauncherError("Proxy route rewrite.addHeaders должен быть ограниченным объектом")
            normalized_rewrite["addHeaders"] = {str(name): str(header_value) for name, header_value in add_headers.items()}
        if "removeHeaders" in rewrite:
            normalized_rewrite["removeHeaders"] = _text_list(rewrite.get("removeHeaders"), "Proxy route removeHeaders", maximum=32)
        normalized_action["rewrite"] = normalized_rewrite

    failover = value.get("failover")
    normalized_failover: dict[str, Any] | None = None
    if failover is not None:
        if not isinstance(failover, dict):
            raise LauncherError("Proxy route failover должен быть объектом")
        strategy = str(failover.get("strategy", "none")).strip().lower()
        if strategy not in {"none", "next-route", "direct-fallback", "retry-same"}:
            raise LauncherError("Proxy route failover.strategy не поддерживается")
        normalized_failover = {"strategy": strategy}
        if "maxAttempts" in failover:
            attempts = int(failover.get("maxAttempts", 0))
            if attempts < 0 or attempts > 32:
                raise LauncherError("Proxy route failover.maxAttempts вне диапазона")
            normalized_failover["maxAttempts"] = attempts

    result = {
        "id": route_id,
        "priority": priority,
        "match": normalized_match,
        "action": normalized_action,
    }
    if normalized_failover is not None:
        result["failover"] = normalized_failover
    return result


def _plugin_proxy_route_list(value: Any) -> list[dict[str, Any]]:
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > 128:
        raise LauncherError("Список proxy route policy плагина должен быть ограниченным списком")
    result: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, item in enumerate(value):
        entry = _plugin_proxy_route_entry(item, fallback_id=f"route-{index + 1}")
        if entry["id"] in seen:
            raise LauncherError(f"Повторяющийся id proxy route policy: {entry['id']}")
        seen.add(entry["id"])
        result.append(entry)
    return result


def _safe_plugin_sql(sql: str, *, read_only: bool) -> str:
    value = sql.strip()
    if not value or len(value) > 16 * 1024 or ";" in value or "\x00" in value:
        raise LauncherError("SQL-запрос плагина имеет небезопасный формат")
    lowered = value.lower()
    forbidden = ("attach", "detach", "load_extension", "pragma", "vacuum")
    if any(token in lowered for token in forbidden):
        raise LauncherError("SQL-запрос плагина содержит запрещённую операцию")
    first_word = lowered.split(None, 1)[0] if lowered.split(None, 1) else ""
    if read_only and first_word not in {"select", "explain"}:
        raise LauncherError("Разрешены только read-only SQL-запросы")
    if not read_only and first_word not in {"create", "insert", "update", "delete", "replace", "alter", "drop"}:
        raise LauncherError("SQL-запрос плагина не поддерживается")
    return value


def _instance_mod_context(info: Any) -> tuple[str, str | None]:
    if str(getattr(info, "game_edition", "java")).strip().lower() == "bedrock":
        raise LauncherError("Моды, ресурспаки и модпаки доступны только для Java-экземпляров")
    is_server = str(getattr(info, "kind", "client")).strip().lower() == "server"
    game_version = str(
        (getattr(info, "server_core_version", "") if is_server else "")
        or getattr(info, "minecraft_version", "")
        or getattr(info, "version_id", "")
    ).strip()
    if not game_version:
        raise LauncherError(f"У экземпляра «{info.name}» не определена версия Minecraft")
    loader = "vanilla" if is_server else str(getattr(info, "loader", "vanilla")).strip().lower()
    return game_version, None if loader == "vanilla" else loader


def _instance_loader_version(info: Any) -> str:
    loader = str(getattr(info, "loader", "vanilla")).strip().lower()
    if loader not in {"fabric", "forge", "neoforge", "quilt"}:
        return ""

    version_id = str(getattr(info, "version_id", "")).strip()
    game_version = str(getattr(info, "minecraft_version", "")).strip()
    profile: dict[str, Any] = {}
    instance_path = getattr(info, "path", None)
    if instance_path and version_id and version_id not in {".", ".."} and "/" not in version_id and "\\" not in version_id and "\x00" not in version_id:
        root = Path(instance_path)
        versions = root / "versions"
        metadata = versions / version_id / f"{version_id}.json"
        if (
            not root.is_symlink()
            and not versions.is_symlink()
            and not metadata.parent.is_symlink()
            and not metadata.is_symlink()
            and metadata.is_file()
        ):
            try:
                if metadata.stat().st_size <= 4 * 1024 * 1024:
                    value = json.loads(metadata.read_text(encoding="utf-8"))
                    profile = value if isinstance(value, dict) else {}
            except (OSError, ValueError, TypeError):
                profile = {}

    artifacts = {
        "fabric": ("net.fabricmc", "fabric-loader"),
        "forge": ("net.minecraftforge", "forge"),
        "neoforge": ("net.neoforged", "neoforge"),
        "quilt": ("org.quiltmc", "quilt-loader"),
    }
    group, artifact = artifacts[loader]
    libraries = profile.get("libraries", [])
    for library in libraries if isinstance(libraries, list) else []:
        name = str(library.get("name", "")) if isinstance(library, dict) else ""
        parts = name.split(":")
        if len(parts) < 3 or parts[:2] != [group, artifact]:
            continue
        version = parts[2]
        if loader == "forge" and game_version and version.startswith(f"{game_version}-"):
            version = version[len(game_version) + 1 :]
        if version:
            return version

    lowered = version_id.lower()
    if loader in {"fabric", "quilt"}:
        prefix = f"{loader}-loader-"
        if lowered.startswith(prefix):
            version = version_id[len(prefix) :]
            suffix = f"-{game_version}"
            return version[: -len(suffix)] if game_version and version.endswith(suffix) else version

    marker = f"{loader}-"
    marker_index = lowered.rfind(marker)
    if marker_index >= 0:
        version = version_id[marker_index + len(marker) :]
        game_prefix = f"{game_version}-"
        if game_version and version.startswith(game_prefix):
            version = version[len(game_prefix) :]
        return version
    return ""


def _update_instance_loader(installer: Any, info: Any, minimum: str = "") -> str:
    """Поставить новейшую стабильную версию загрузчика (не ниже ``minimum``).

    Новая версия ставится рядом со старой; старый каталог версии удаляется
    только после успешной установки — экземпляр подхватывает самый свежий
    version JSON, поэтому прервавшаяся установка ничего не ломает.
    """
    loader = str(getattr(info, "loader", "vanilla")).strip().lower()
    if loader not in {"fabric", "quilt", "forge", "neoforge"}:
        raise LauncherError("У экземпляра нет модлоадера, который можно обновить")
    minecraft_version = str(getattr(info, "minecraft_version", "")).strip()
    if not minecraft_version:
        raise LauncherError(f"У экземпляра «{info.name}» не определена версия Minecraft")
    choices = [choice.version for choice in installer.loaders.choices(loader, minecraft_version)]
    target = pick_loader_version(choices, minimum)
    if not target:
        raise LauncherError(
            f"Для Minecraft {minecraft_version} нет версии {loader} "
            + (f"{minimum} или новее" if minimum else "в каталоге загрузчика")
        )
    old_version_id = str(getattr(info, "version_id", "")).strip()
    result = installer.install(
        minecraft_version,
        loader=loader,
        loader_version=target,
        instance_name=info.name,
        assets=False,
        replace_existing=False,
    )
    new_version_id = str(result.version_id)
    if old_version_id and old_version_id not in {new_version_id, minecraft_version} and re.fullmatch(r"[A-Za-z0-9._+-]+", old_version_id):
        old_dir = Path(info.path) / "versions" / old_version_id
        try:
            if old_dir.is_dir() and not old_dir.is_symlink():
                shutil.rmtree(old_dir)
        except OSError:
            pass
    return target


def _auto_repair_report(
    config: ConfigStore,
    workspace: InstanceWorkspace,
    info: Any,
    lines: list[str],
    exit_code: int | None,
    launch_settings: dict[str, Any],
    *,
    error: str = "",
    since: float = 0.0,
    manual: bool = False,
    account_id: str = "",
) -> dict[str, Any] | None:
    """Предложения авто-починки; ``None`` — окно показывать не нужно.

    Сбой самой диагностики не должен портить результат запуска, поэтому
    любые ошибки здесь глушатся (кроме ручного вызова — там их видно).
    """
    if info is None or (not manual and not config.settings.auto_repair_enabled):
        return None
    if str(getattr(info, "kind", "client")).lower() != "client" or str(getattr(info, "game_edition", "java")).lower() != "java":
        return None
    try:
        try:
            instance_settings = workspace.settings(info.name)
        except LauncherError:
            instance_settings = {}
        try:
            # Только JSON со списком: токены из системного хранилища не нужны.
            account_records = [
                {key: item.get(key) for key in ("account_id", "kind", "username", "alias", "is_default")}
                for item in AccountStore(config)._read()
            ]
        except LauncherError:
            account_records = []
        if not account_id:
            account_id = str(launch_settings.get("account_id", "") or "").strip()
            if account_id.lower() in {"", "auto", "microsoft", "offline"}:
                account_id = str(getattr(config.settings, "default_account_id", "") or "")
        report = diagnose_repairs(
            lines,
            exit_code,
            instance_path=Path(info.path),
            instance_settings=instance_settings,
            effective_memory=str(launch_settings.get("memory", "")),
            loader=str(getattr(info, "loader", "")),
            minecraft_version=str(getattr(info, "minecraft_version", "")),
            error=error,
            since=since,
            muted=() if manual else config.settings.auto_repair_muted,
            loader_version=_instance_loader_version(info),
            accounts=account_records,
            current_account_id="" if account_id.lower() == "auto" else account_id,
        )
    except Exception:  # noqa: BLE001 — диагностика необязательна
        if manual:
            raise
        return None
    report["instance_name"] = info.name
    report["manual"] = manual
    return report


def _effective_launch_settings(
    workspace: InstanceWorkspace,
    config: ConfigStore,
    instance_name: str | None,
    request: dict[str, Any],
) -> dict[str, Any]:
    profile_id = str(
        request.get("profile_id") or getattr(config.settings, "active_launcher_profile_id", "")
    ).strip().lower()
    profile = next(
        (item for item in config.settings.launcher_profiles if str(item.get("id", "")).lower() == profile_id),
        None,
    ) if profile_id else None
    if not instance_name:
        values = {
            "memory": str(request.get("memory", "")).strip() or config.settings.memory,
            "java_path": str(request.get("java_path", "")).strip() or config.settings.java_path,
            "graphics_mode": config.settings.graphics_mode,
        }
    else:
        values = workspace.settings(instance_name)
    if profile:
        # Явные настройки экземпляра важнее профиля запуска: профиль — это
        # значения по умолчанию, а не способ перетереть ручной выбор.
        instance_keys = {"memory", "java_path", "graphics_mode", "jvm_arguments", "game_arguments", "jvm_preset"} if instance_name else set()
        for key in ("memory", "java_path", "graphics_mode", "window_width", "window_height", "fullscreen", "quick_play_world", "quick_play_server", "jvm_arguments", "game_arguments", "jvm_preset"):
            candidate = profile.get(key)
            if key in {"memory", "java_path", "graphics_mode"} and candidate in {None, "", "auto"}:
                continue
            if candidate is None or candidate == "":
                continue
            if key in instance_keys:
                own = values.get(key)
                if isinstance(own, str) and own.strip() and own.strip().lower() != "auto":
                    continue
                if isinstance(own, list) and own:
                    continue
            values[key] = candidate
        values["account_id"] = str(profile.get("account_id", "auto"))
        values["profile_id"] = str(profile.get("id", ""))
    if values["memory"] == "auto":
        values["memory"] = str(request.get("memory", "")).strip() or config.settings.memory
    if not values["java_path"]:
        values["java_path"] = str(request.get("java_path", "")).strip() or config.settings.java_path
    if values["graphics_mode"] == "auto":
        values["graphics_mode"] = config.settings.graphics_mode
    return values


def _select_account(kind: str, config: ConfigStore, accounts: AccountStore) -> Any:
    if kind != "auto":
        selected = accounts.get(account_id=kind)
        if selected:
            return MicrosoftAuthenticator(config, accounts).get_valid(selected.account_id) if selected.kind == "microsoft" else selected
        if kind == "offline":
            return OfflineDebugAccount(config, accounts).get()
        if kind == "microsoft":
            return MicrosoftAuthenticator(config, accounts).get_valid()
        raise LauncherError(f"Аккаунт не найден: {kind}")

    configured_id = str(getattr(config.settings, "default_account_id", "auto") or "auto")
    selected = accounts.get(account_id=configured_id) if configured_id.lower() != "auto" else None
    if selected:
        if selected.kind == "microsoft":
            return MicrosoftAuthenticator(config, accounts).get_valid(selected.account_id)
        if selected.kind == "offline":
            return OfflineDebugAccount(config, accounts).get()
        return selected

    default = accounts.default()
    if default:
        if default.kind == "microsoft":
            return MicrosoftAuthenticator(config, accounts).get_valid(default.account_id)
        if default.kind == "offline":
            return OfflineDebugAccount(config, accounts).get()
        return default
    microsoft = accounts.get(kind="microsoft")
    if microsoft:
        return MicrosoftAuthenticator(config, accounts).get_valid(microsoft.account_id)
    return OfflineDebugAccount(config, accounts).get()


def _microsoft_account(account_id: str, config: ConfigStore, accounts: AccountStore, http: HttpClient) -> Any:
    account = accounts.get(account_id=account_id)
    if not account or account.kind != "microsoft":
        raise LauncherError("Нужен Microsoft-аккаунт")
    refreshed = MicrosoftAuthenticator(config, accounts, http).get_valid(account.account_id)
    if refreshed.account_id != account.account_id:
        raise LauncherError("Microsoft-аккаунт больше не доступен")
    return refreshed


def _write_event(value: dict[str, Any]) -> None:
    # Вызывается и из главного цикла, и из фоновых ниток Bedrock: без
    # блокировки JSON-строки события и ответа могут склеиться в одну —
    # получатель тогда не распарсит строку и весь бэкенд «умрёт».
    with _STDOUT_LOCK:
        sys.stdout.write(json.dumps({"event": value}, ensure_ascii=False) + "\n")
        sys.stdout.flush()


_STDOUT_LOCK = threading.Lock()


def _process_request(raw: str) -> dict[str, Any]:
    operation_id = ""
    try:
        request = json.loads(raw or "{}")
        if not isinstance(request, dict):
            raise LauncherError("GUI-запрос имеет неожиданный формат")
        operation_id = str(request.get("operation_id", "")).strip()
        return {"ok": True, "data": handle(request, emit=_write_event)}
    except (LauncherError, OSError, ValueError, TypeError) as exc:
        return {"ok": False, "error": str(exc)}
    except Exception as exc:
        return {"ok": False, "error": f"Backend error: {exc}"}
    finally:
        if operation_id:
            CancellationToken(operation_id).cleanup()


def main() -> int:
    if sys.stdin is None:
        # Windowed-exe (PyInstaller --noconsole), запущенный напрямую без
        # пайпа от Tauri: воркеру нечего читать — тихо выходим вместо краша.
        return 0
    for raw in sys.stdin:
        if not raw.strip():
            continue
        response = _process_request(raw)
        with _STDOUT_LOCK:
            sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
            sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
