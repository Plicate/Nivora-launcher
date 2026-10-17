from __future__ import annotations

import base64
import json
import os
import re
import stat
import zlib
import zipfile
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .config import ConfigStore, _atomic_json_write, format_memory_kib, parse_memory_kib, system_memory_kib, _profile_arguments, _safe_preset_id
from .errors import LauncherError
from .instances import InstanceInfo, InstanceStore
from .security import (
    load_protected_settings,
    normalize_security_permissions,
    normalize_security_profile,
    save_protected_settings,
)
from .snapshots import SnapshotStore


INSTANCE_SETTINGS_FILE = ".trial-launcher.json"
SECURITY_POLICY_VERSION = 3
PROJECT_METADATA_SUFFIX = ".trial-project.json"
MODPACK_MARKER = ".trial-launcher-modpacks.json"
MAX_ASSET_METADATA_BYTES = 64 * 1024
MAX_ASSET_ICON_BYTES = 64 * 1024
DEFAULT_CONSOLE_COLORS = {"error": "#ff657a", "info": "#e6edf7", "warning": "#f5c451"}
ASSET_DIRECTORIES = {
    "mods": "mods",
    "resourcepacks": "resourcepacks",
    "shaders": "shaderpacks",
}
PROJECT_TYPE_KINDS = {
    "mod": "mods",
    "resourcepack": "resourcepacks",
    "shader": "shaders",
    "datapack": "datapacks",
}
DEFAULT_INSTANCE_SETTINGS: dict[str, Any] = {
    "memory": "auto",
    "java_path": "",
    "graphics_mode": "auto",
    "window_width": 1280,
    "window_height": 720,
    "fullscreen": False,
    "quick_play_world": "",
    "quick_play_server": "",
    "jvm_arguments": [],
    "game_arguments": [],
    "jvm_preset": "none",
    "console_colors": DEFAULT_CONSOLE_COLORS,
    "security_profile": "strict",
    "security_permissions": ["files:instance", "desktop:display", "desktop:audio"],
    "security_policy_version": SECURITY_POLICY_VERSION,
}
INSTANCE_SNAPSHOT_CATEGORIES = ("mods", "resourcepacks", "shaders", "saves")
MAX_NBT_FILE_BYTES = 64 * 1024 * 1024
MAX_NBT_DECOMPRESSED_BYTES = 64 * 1024 * 1024
MAX_NBT_COLLECTION_ITEMS = 100_000
MAX_NBT_DEPTH = 64
MAX_NBT_NODES = 500_000
INSTANCE_SECURITY_KEYS = frozenset({"java_path", "security_profile", "security_permissions", "security_policy_version"})
INSTANCE_REGULAR_KEYS = tuple(key for key in DEFAULT_INSTANCE_SETTINGS if key not in INSTANCE_SECURITY_KEYS)
INSTANCE_SECURITY_DEFAULTS: dict[str, Any] = {
    "java_path": "",
    "security_profile": "strict",
    "security_permissions": ["files:instance", "desktop:display", "desktop:audio"],
    "security_policy_version": SECURITY_POLICY_VERSION,
}


class InstanceWorkspace:
    def __init__(self, config: ConfigStore, instances: InstanceStore) -> None:
        self.config = config
        self.instances = instances

    def _java_instance(self, name: str) -> InstanceInfo:
        info = self.instances.get(name)
        if info.kind != "client" or info.game_edition != "java":
            raise LauncherError("Эта рабочая область предназначена только для Minecraft Java Edition")
        return info

    def settings_path(self, name: str) -> Path:
        return self._java_instance(name).path / INSTANCE_SETTINGS_FILE

    def settings(self, name: str) -> dict[str, Any]:
        info = self._java_instance(name)
        values = dict(DEFAULT_INSTANCE_SETTINGS)
        values.update(load_protected_settings(self.config, info.name, INSTANCE_SECURITY_DEFAULTS))
        path = info.path / INSTANCE_SETTINGS_FILE
        if path.is_symlink():
            raise LauncherError("Настройки экземпляра являются символьной ссылкой")
        rewrite = not path.exists()
        if path.exists():
            try:
                if path.stat().st_size > 256 * 1024:
                    raise LauncherError("Настройки экземпляра слишком большие")
                raw = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(raw, dict):
                    values.update({key: raw[key] for key in INSTANCE_REGULAR_KEYS if key in raw})
                    rewrite = bool(INSTANCE_SECURITY_KEYS.intersection(raw))
            except (OSError, ValueError, TypeError) as exc:
                raise LauncherError(f"Не удалось прочитать настройки экземпляра: {exc}") from exc
        normalized = _normalize_instance_settings(values)
        save_protected_settings(self.config, info.name, {key: normalized[key] for key in INSTANCE_SECURITY_DEFAULTS})
        if rewrite:
            _write_instance_settings(path, normalized)
        return normalized

    def save_settings(self, name: str, values: dict[str, Any]) -> dict[str, Any]:
        info = self._java_instance(name)
        current = self.settings(name)
        for key in DEFAULT_INSTANCE_SETTINGS:
            if key in values:
                if key == "console_colors" and isinstance(values[key], dict):
                    current[key] = {**current["console_colors"], **values[key]}
                else:
                    current[key] = values[key]
        normalized = _normalize_instance_settings(current)
        _write_instance_settings(info.path / INSTANCE_SETTINGS_FILE, normalized)
        save_protected_settings(self.config, info.name, {key: normalized[key] for key in INSTANCE_SECURITY_DEFAULTS})
        return normalized

    def snapshot(self, name: str) -> dict[str, Any]:
        info = self._java_instance(name)
        return {
            "instance": {
                "name": info.name,
                "display_name": info.display_name or info.name,
                "description": info.description,
                "group": info.group,
                "icon": info.icon,
                "banner": info.banner,
                "played_seconds": info.played_seconds,
                "version_id": info.version_id,
                "minecraft_version": info.minecraft_version,
                "loader": info.loader,
                "path": str(info.path),
                "kind": info.kind,
                "game_edition": info.game_edition,
                "server_core": info.server_core,
                "server_core_version": info.server_core_version,
                "server_port": info.server_port,
                "server_eula": info.server_eula,
                "running": info.running,
                "notes": info.notes,
                "tags": info.tags,
            },
            "settings": self.settings(name),
            "assets": {kind: self.list_assets(name, kind) for kind in ASSET_DIRECTORIES},
            "worlds": self.list_worlds(name),
            "servers": self.list_servers(name),
            "snapshots": {category: SnapshotStore(info.path).list(category) for category in INSTANCE_SNAPSHOT_CATEGORIES},
        }

    def list_assets(self, name: str, kind: str) -> list[dict[str, Any]]:
        directory = self._asset_directory(name, kind)
        return self._list_assets_from_directory(directory, kind)

    def list_world_assets(self, name: str, world_id: str, kind: str = "datapacks") -> list[dict[str, Any]]:
        if kind != "datapacks":
            raise LauncherError(f"Неизвестный тип ресурса мира: {kind}")
        directory = self.world_directory(name, world_id) / "datapacks"
        return self._list_assets_from_directory(directory, kind)

    def installed_projects(
        self,
        name: str,
        project_type: str,
        *,
        catalog_id: str | None = None,
    ) -> set[str]:
        self._java_instance(name)
        if project_type == "modpack":
            marker = self.instances.get(name).path / MODPACK_MARKER
            if marker.is_symlink() or not marker.is_file():
                return set()
            try:
                if marker.stat().st_size > 512 * 1024:
                    return set()
                value = json.loads(marker.read_text(encoding="utf-8"))
            except (OSError, ValueError, TypeError, json.JSONDecodeError):
                return set()
            return {
                str(item.get("project_id", ""))
                for item in value
                if isinstance(item, dict)
                and str(item.get("project_id", "")).strip()
                and (
                    catalog_id is None
                    or str(item.get("catalog_id") or item.get("source") or "").strip().lower()
                    == catalog_id.strip().lower()
                )
            } if isinstance(value, list) else set()
        return {
            project_id
            for metadata_path in self._catalog_metadata_paths(name, project_type)
            for project_id in [self._metadata_project_id(metadata_path, project_type, catalog_id)]
            if project_id
        }

    def remove_installed_project(
        self,
        name: str,
        project_type: str,
        project_id: str,
        *,
        catalog_id: str | None = None,
    ) -> tuple[dict[str, Any], int]:
        self._java_instance(name)
        project_id = project_id.strip()
        if not project_id:
            raise LauncherError("Не указан project ID ресурса")
        removed = 0
        for metadata_path in self._catalog_metadata_paths(name, project_type):
            if self._metadata_project_id(metadata_path, project_type, catalog_id) != project_id:
                continue
            target_name = metadata_path.name[1 : -len(PROJECT_METADATA_SUFFIX)]
            target = metadata_path.parent / target_name
            if target.is_symlink():
                raise LauncherError("Установленный ресурс является символьной ссылкой")
            if target.is_file():
                target.unlink()
                removed += 1
            metadata_path.unlink()
        if not removed:
            raise LauncherError("Установленный ресурс не найден")
        return self.snapshot(name), removed

    def _catalog_metadata_paths(self, name: str, project_type: str) -> list[Path]:
        self._java_instance(name)
        kind = PROJECT_TYPE_KINDS.get(project_type)
        if not kind:
            raise LauncherError(f"Неизвестный тип ресурса: {project_type}")
        directories: list[Path]
        if kind == "datapacks":
            saves = self.instances.get(name).path / "saves"
            if saves.is_symlink() or not saves.exists():
                return []
            directories = [
                world / "datapacks"
                for world in saves.iterdir()
                if world.is_dir() and not world.is_symlink()
            ]
        else:
            directories = [self._asset_directory(name, kind)]
        paths: list[Path] = []
        for directory in directories:
            if directory.is_symlink() or not directory.exists():
                continue
            paths.extend(
                path
                for path in directory.rglob(f"*{PROJECT_METADATA_SUFFIX}")
                if path.is_file() and not path.is_symlink()
            )
        return paths

    @staticmethod
    def _metadata_project_id(path: Path, project_type: str, catalog_id: str | None = None) -> str:
        try:
            if path.stat().st_size > 32 * 1024:
                return ""
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            return ""
        if not isinstance(value, dict):
            return ""
        stored_type = str(value.get("project_type", project_type)).strip().lower()
        source = str(value.get("source", "")).strip().lower()
        stored_catalog = str(value.get("catalog_id") or value.get("catalog") or ("modrinth" if source == "modrinth" else "")).strip().lower()
        if stored_type != project_type or (catalog_id is not None and stored_catalog != catalog_id.strip().lower()):
            return ""
        return str(value.get("project_id", "")).strip()

    def _list_assets_from_directory(self, directory: Path, kind: str) -> list[dict[str, Any]]:
        if directory.is_symlink():
            raise LauncherError("Каталог ресурса является символьной ссылкой")
        if not directory.exists():
            return []
        values: list[dict[str, Any]] = []
        for path in sorted(directory.rglob("*"), key=lambda item: str(item).lower()):
            if path.is_symlink() or not path.is_file() or path.name.endswith(".trial-meta") or path.name.endswith(PROJECT_METADATA_SUFFIX) or path.name.endswith(".part"):
                continue
            stat = path.stat()
            metadata = self._asset_metadata(path)
            values.append(
                {
                    "name": path.name,
                    "display_name": metadata["display_name"],
                    "path": str(path.relative_to(directory)),
                    "size": stat.st_size,
                    "modified_at": stat.st_mtime,
                    "kind": kind,
                    "enabled": not (kind == "mods" and path.name.endswith(".disabled")),
                    "version": metadata["version"],
                    "author": metadata["author"],
                    "source": metadata["source"],
                    "project_id": metadata["project_id"],
                    "icon_url": metadata["icon_url"],
                    "icon_data": metadata["icon_data"],
                }
            )
            if len(values) >= 5000:
                break
        return values

    def _asset_metadata(self, path: Path) -> dict[str, str]:
        metadata = self._read_project_metadata(path)
        archive_metadata = self._read_archive_metadata(path)
        display_name = str(metadata.get("display_name") or metadata.get("name") or archive_metadata.get("display_name") or _pretty_asset_name(path.name))
        version = str(metadata.get("version") or archive_metadata.get("version") or "")
        author = str(metadata.get("author") or archive_metadata.get("author") or "")
        source = str(metadata.get("source") or "Локальный файл")
        project_id = str(metadata.get("project_id") or "")
        icon_url = _safe_asset_icon_url(str(metadata.get("icon_url") or ""))
        icon_data = str(archive_metadata.get("icon_data") or "")
        return {
            "display_name": display_name[:160],
            "version": version[:80],
            "author": author[:120],
            "source": source[:80],
            "project_id": project_id[:160],
            "icon_url": icon_url,
            "icon_data": icon_data,
        }

    @staticmethod
    def _read_project_metadata(path: Path) -> dict[str, Any]:
        metadata_path = path.with_name(f".{path.name}{PROJECT_METADATA_SUFFIX}")
        if metadata_path.is_symlink() or not metadata_path.is_file():
            return {}
        try:
            if metadata_path.stat().st_size > MAX_ASSET_METADATA_BYTES:
                return {}
            value = json.loads(metadata_path.read_text(encoding="utf-8"))
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            return {}
        return value if isinstance(value, dict) else {}

    @staticmethod
    def _read_archive_metadata(path: Path) -> dict[str, str]:
        if path.suffix.lower() not in {".jar", ".zip", ".mcpack", ".disabled"}:
            return {}
        try:
            with zipfile.ZipFile(path) as archive:
                metadata: dict[str, str] = {}
                for entry in ("fabric.mod.json", "quilt.mod.json", "mcmod.info", "pack.mcmeta"):
                    raw = _read_zip_entry(archive, entry, MAX_ASSET_METADATA_BYTES)
                    if not raw:
                        continue
                    parsed = _parse_archive_metadata(entry, raw)
                    for key, value in parsed.items():
                        if value and key not in metadata:
                            metadata[key] = value
                icon_entry = metadata.pop("icon_entry", "")
                if not icon_entry:
                    icon_entry = _find_archive_icon(archive)
                icon = _read_zip_entry(archive, icon_entry, MAX_ASSET_ICON_BYTES) if icon_entry else b""
                metadata["icon_data"] = _asset_image_data_url(icon)
                return metadata
        except (OSError, ValueError, KeyError, zipfile.BadZipFile):
            return {}

    def remove_asset(self, name: str, kind: str, relative_path: str) -> None:
        if kind == "datapacks":
            parts = Path(relative_path).parts
            if len(parts) < 2 or any(part in {"", ".", ".."} for part in parts):
                raise LauncherError("Небезопасный путь датапака")
            world_id = parts[0]
            directory = (self.world_directory(name, world_id) / "datapacks").resolve()
            relative_path = str(Path(*parts[1:]))
        else:
            directory = self._asset_directory(name, kind).resolve()
        raw_target = directory / relative_path
        if raw_target.is_symlink():
            raise LauncherError("Символьные ссылки в ресурсах запрещены")
        target = raw_target.resolve()
        if not _within(target, directory) or not target.is_file():
            raise LauncherError("Файл ресурса не найден")
        target.unlink()
        sidecar = target.with_name(f".{target.name}{PROJECT_METADATA_SUFFIX}")
        if sidecar.exists() or sidecar.is_symlink():
            sidecar.unlink()

    def toggle_mod(self, name: str, relative_path: str, enabled: bool) -> dict[str, Any]:
        directory = self._asset_directory(name, "mods").resolve()
        raw_target = directory / relative_path
        target = raw_target.resolve()
        if raw_target.is_symlink() or not _within(target, directory) or not target.is_file():
            raise LauncherError("Файл мода не найден")
        if enabled and target.name.endswith(".disabled"):
            destination = target.with_name(target.name.removesuffix(".disabled"))
        elif not enabled and not target.name.endswith(".disabled"):
            destination = target.with_name(f"{target.name}.disabled")
        else:
            return self.snapshot(name)
        if destination.exists() or destination.is_symlink():
            raise LauncherError(f"Файл уже существует: {destination.name}")
        target.rename(destination)
        sidecar = target.with_name(f".{target.name}{PROJECT_METADATA_SUFFIX}")
        next_sidecar = destination.with_name(f".{destination.name}{PROJECT_METADATA_SUFFIX}")
        if sidecar.is_symlink() or next_sidecar.is_symlink():
            raise LauncherError("Метаданные мода являются символьной ссылкой")
        if sidecar.exists():
            sidecar.rename(next_sidecar)
        return self.snapshot(name)

    def import_asset(self, name: str, kind: str, file_name: str, content: bytes) -> dict[str, Any]:
        if len(content) > 256 * 1024 * 1024:
            raise LauncherError("Файл ресурса слишком большой (максимум 256 MiB)")
        normalized = file_name.replace("\\", "/")
        safe_name = Path(normalized).name
        if not safe_name or safe_name in {".", ".."} or safe_name != normalized:
            raise LauncherError("Небезопасное имя импортируемого файла")
        directory = self._asset_directory(name, kind)
        if directory.is_symlink():
            raise LauncherError("Каталог ресурса является символьной ссылкой")
        directory.mkdir(parents=True, exist_ok=True)
        if not directory.is_dir():
            raise LauncherError("Каталог ресурса недоступен")
        destination = _unique_file(directory, safe_name)
        _write_private_file(destination, content)
        return self.snapshot(name)

    def list_servers(self, name: str) -> list[dict[str, Any]]:
        path = self._java_instance(name).path / "servers.dat"
        if path.is_symlink() or not path.exists():
            return []
        root = _read_nbt(path)
        values = root.get("servers", []) if isinstance(root, dict) else []
        if not isinstance(values, list):
            return []
        result: list[dict[str, Any]] = []
        for index, server in enumerate(values):
            if not isinstance(server, dict):
                continue
            address = str(server.get("ip", "")).strip()[:256]
            if not address:
                continue
            result.append(
                {
                    "id": f"server-{index}",
                    "name": str(server.get("name") or address)[:256],
                    "address": address,
                    "accept_textures": bool(server.get("acceptTextures", False)),
                }
            )
        return result

    def list_worlds(self, name: str) -> list[dict[str, Any]]:
        saves = self._java_instance(name).path / "saves"
        if saves.is_symlink() or not saves.exists():
            return []
        result: list[dict[str, Any]] = []
        for path in sorted((item for item in saves.iterdir() if item.is_dir() and not item.is_symlink()), key=lambda item: item.name.lower()):
            level = path / "level.dat"
            data: dict[str, Any] = {}
            if level.exists():
                try:
                    root = _read_nbt(level)
                    data = root.get("Data", root) if isinstance(root, dict) else {}
                except LauncherError:
                    data = {}
            icon = path / "icon.png"
            result.append(
                {
                    "id": path.name,
                    "name": str(data.get("LevelName") or path.name),
                    "path": str(path),
                    "size": _directory_size(path),
                    "last_played": _as_int(data.get("LastPlayed")),
                    "icon": _image_data_url(icon),
                    "datapacks": self.list_world_assets(name, path.name),
                }
            )
        return result

    def _asset_directory(self, name: str, kind: str) -> Path:
        try:
            relative = ASSET_DIRECTORIES[kind]
        except KeyError as exc:
            raise LauncherError(f"Неизвестный тип ресурса: {kind}") from exc
        directory = self._java_instance(name).path / relative
        if directory.is_symlink():
            raise LauncherError("Каталог ресурса является символьной ссылкой")
        return directory

    def world_directory(self, name: str, world_id: str) -> Path:
        saves = self._java_instance(name).path / "saves"
        if saves.is_symlink():
            raise LauncherError("Каталог миров является символьной ссылкой")
        safe_id = Path(world_id).name
        if not safe_id or safe_id != world_id or world_id in {".", ".."}:
            raise LauncherError("Небезопасное имя мира")
        raw_world = saves / safe_id
        if raw_world.is_symlink():
            raise LauncherError("Мир является символьной ссылкой")
        world = raw_world.resolve()
        if not _within(world, saves.resolve()) or not world.is_dir():
            raise LauncherError("Мир не найден")
        return world


def _normalize_instance_settings(values: dict[str, Any]) -> dict[str, Any]:
    result = dict(DEFAULT_INSTANCE_SETTINGS)
    result.update(values)
    memory = str(result.get("memory", "auto")).strip() or "auto"
    if memory.lower() in {"auto", "automatic"}:
        result["memory"] = "auto"
    else:
        memory_kib = parse_memory_kib(memory)
        total_kib = system_memory_kib()
        if total_kib and memory_kib > total_kib:
            raise LauncherError(f"Память экземпляра не может превышать доступные {format_memory_kib(total_kib)}")
        result["memory"] = format_memory_kib(memory_kib)
    result["java_path"] = str(result.get("java_path", "")).strip()
    graphics = str(result.get("graphics_mode", "auto")).strip().lower()
    if graphics not in {"auto", "nvidia", "mesa", "software"}:
        raise LauncherError(f"Неизвестный graphics mode: {graphics}")
    result["graphics_mode"] = graphics
    for key, default in (("window_width", 1280), ("window_height", 720)):
        try:
            value = int(result.get(key, default))
        except (TypeError, ValueError) as exc:
            raise LauncherError(f"{key} должен быть числом") from exc
        result[key] = min(7680, max(320, value))
    result["fullscreen"] = bool(result.get("fullscreen", False))
    result["quick_play_world"] = str(result.get("quick_play_world", "")).strip()[:256]
    result["quick_play_server"] = str(result.get("quick_play_server", "")).strip()[:256]
    raw_colors = result.get("console_colors")
    raw_colors = raw_colors if isinstance(raw_colors, dict) else {}
    console_colors: dict[str, str] = {}
    for key, default in DEFAULT_CONSOLE_COLORS.items():
        color = str(raw_colors.get(key, default)).strip()
        console_colors[key] = color.lower() if re.fullmatch(r"#[0-9a-fA-F]{6}", color) else default
    result["console_colors"] = console_colors
    result["security_profile"] = normalize_security_profile(result.get("security_profile", "strict"))
    # Аргументы JVM задаются на экземпляр и перекрывают профиль запуска —
    # нормализация та же, что у глобальных профилей.
    result["jvm_arguments"] = _profile_arguments(result.get("jvm_arguments"))
    result["game_arguments"] = _profile_arguments(result.get("game_arguments"))
    result["jvm_preset"] = _safe_preset_id(str(result.get("jvm_preset", "none") or "none"))
    permissions = normalize_security_permissions(result.get("security_permissions"))
    try:
        previous_policy_version = int(result.get("security_policy_version", 0))
    except (TypeError, ValueError):
        previous_policy_version = 0
    if previous_policy_version < SECURITY_POLICY_VERSION:
        # Older defaults exposed both network and desktop channels without a consent step.
        permissions = [permission for permission in permissions if permission != "network:game"]
        for permission in ("desktop:display", "desktop:audio"):
            if permission not in permissions:
                permissions.append(permission)
    result["security_permissions"] = permissions
    result["security_policy_version"] = SECURITY_POLICY_VERSION
    return result


def _pretty_asset_name(file_name: str) -> str:
    value = file_name
    if value.endswith(".disabled"):
        value = value[: -len(".disabled")]
    for suffix in (".jar", ".zip"):
        if value.lower().endswith(suffix):
            value = value[: -len(suffix)]
            break
    value = re.sub(r"[-_ ]v?\d+(?:\.\d+)+(?:[-+][\w.-]+)?$", "", value, flags=re.IGNORECASE)
    value = re.sub(r"(?<!^)(?=[A-Z])", " ", value)
    value = re.sub(r"[-_]+", " ", value)
    return " ".join(value.split()) or "Неизвестный ресурс"


def _safe_asset_icon_url(value: str) -> str:
    try:
        parsed = urlsplit(value.strip())
    except ValueError:
        return ""
    if parsed.scheme.lower() != "https" or not parsed.hostname or parsed.username or parsed.password:
        return ""
    return value.strip()[:2048]


def _read_zip_entry(archive: zipfile.ZipFile, name: str, limit: int) -> bytes:
    if not name or name.startswith("/") or ".." in Path(name).parts:
        return b""
    try:
        info = archive.getinfo(name)
        if info.is_dir() or info.file_size < 0 or info.file_size > limit:
            return b""
        return archive.read(info)
    except (KeyError, OSError, RuntimeError, ValueError, zipfile.BadZipFile):
        return b""


def _text_value(value: Any) -> str:
    if isinstance(value, str):
        return value.strip()
    if isinstance(value, dict):
        return str(value.get("text") or value.get("translate") or "").strip()
    if isinstance(value, list):
        return " ".join(part for part in (_text_value(item) for item in value) if part).strip()
    return ""


def _author_value(value: Any) -> str:
    if isinstance(value, str):
        return value.strip()
    if isinstance(value, dict):
        return str(value.get("name") or "").strip()
    if isinstance(value, list):
        return ", ".join(part for part in (_author_value(item) for item in value) if part)
    return ""


def _parse_archive_metadata(entry: str, raw: bytes) -> dict[str, str]:
    text = raw.decode("utf-8", errors="replace")
    value: Any = None
    if entry.endswith(".json") or entry.endswith(".mcmeta"):
        try:
            value = json.loads(text)
        except (ValueError, TypeError, json.JSONDecodeError):
            return {}
    if entry == "fabric.mod.json" and isinstance(value, dict):
        return {
            "display_name": _text_value(value.get("name") or value.get("id")),
            "version": _text_value(value.get("version")),
            "author": _author_value(value.get("authors") or value.get("contributors")),
            "icon_entry": _text_value(value.get("icon")),
        }
    if entry == "quilt.mod.json" and isinstance(value, dict):
        loader = value.get("quilt_loader") if isinstance(value.get("quilt_loader"), dict) else {}
        metadata = loader.get("metadata") if isinstance(loader.get("metadata"), dict) else {}
        return {
            "display_name": _text_value(metadata.get("name") or loader.get("id")),
            "version": _text_value(loader.get("version")),
            "author": _author_value(metadata.get("contributors")),
            "icon_entry": _text_value(metadata.get("icon") or value.get("icon")),
        }
    if entry == "mcmod.info" and value is not None:
        item = value[0] if isinstance(value, list) and value else value
        if isinstance(item, dict):
            return {
                "display_name": _text_value(item.get("displayName") or item.get("name")),
                "version": _text_value(item.get("version")),
                "author": _author_value(item.get("authorList") or item.get("authors")),
                "icon_entry": _text_value(item.get("logoFile")),
            }
    if entry == "pack.mcmeta" and isinstance(value, dict):
        pack = value.get("pack") if isinstance(value.get("pack"), dict) else {}
        return {"display_name": _text_value(pack.get("description")), "version": _text_value(pack.get("pack_format"))}
    if entry.endswith("mods.toml"):
        def match(key: str) -> str:
            found = re.search(rf"(?m)^\s*{key}\s*=\s*[\"']([^\"']+)", text)
            return found.group(1).strip() if found else ""

        return {"display_name": match("displayName"), "version": match("version"), "author": match("authors")}
    return {}


def _find_archive_icon(archive: zipfile.ZipFile) -> str:
    preferred = ("icon.png", "logo.png", "pack.png")
    names = archive.namelist()
    for name in preferred:
        if name in names and _read_zip_entry(archive, name, MAX_ASSET_ICON_BYTES):
            return name
    candidates = sorted(
        name
        for name in names
        if Path(name).name.lower() in preferred and _read_zip_entry(archive, name, MAX_ASSET_ICON_BYTES)
    )
    return candidates[0] if candidates else ""


def _asset_image_data_url(value: bytes) -> str:
    if value.startswith(b"\x89PNG\r\n\x1a\n"):
        mime = "image/png"
    elif value.startswith(b"\xff\xd8\xff"):
        mime = "image/jpeg"
    elif value.startswith(b"RIFF") and value[8:12] == b"WEBP":
        mime = "image/webp"
    else:
        return ""
    return f"data:{mime};base64,{base64.b64encode(value).decode('ascii')}"


def _write_instance_settings(path: Path, values: dict[str, Any]) -> None:
    _atomic_json_write(path, {key: values[key] for key in INSTANCE_REGULAR_KEYS}, private=True)


def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _directory_size(path: Path) -> int:
    total = 0
    for item in path.rglob("*"):
        if item.is_file() and not item.is_symlink():
            try:
                total += item.stat().st_size
            except OSError:
                continue
    return total


def _unique_file(directory: Path, file_name: str) -> Path:
    candidate = directory / file_name
    if not os.path.lexists(candidate):
        return candidate
    stem = candidate.stem
    suffix = candidate.suffix
    index = 2
    while True:
        candidate = directory / f"{stem}-{index}{suffix}"
        if not os.path.lexists(candidate):
            return candidate
        index += 1


def _as_int(value: Any) -> int | None:
    try:
        number = int(value)
    except (TypeError, ValueError):
        return None
    return number if number > 0 else None


def _image_data_url(path: Path) -> str:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 2 * 1024 * 1024:
        return ""
    try:
        raw = path.read_bytes()
        if not raw.startswith(b"\x89PNG\r\n\x1a\n"):
            return ""
        return "data:image/png;base64," + base64.b64encode(raw).decode("ascii")
    except OSError:
        return ""


def _read_nbt(path: Path) -> dict[str, Any]:
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
        with os.fdopen(fd, "rb") as handle:
            file_stat = os.fstat(handle.fileno())
            if not stat.S_ISREG(file_stat.st_mode) or file_stat.st_size > MAX_NBT_FILE_BYTES:
                raise LauncherError(f"NBT-файл слишком большой или имеет неверный тип: {path.name}")
            raw = handle.read(MAX_NBT_FILE_BYTES + 1)
            if len(raw) > MAX_NBT_FILE_BYTES:
                raise LauncherError(f"NBT-файл слишком большой: {path.name}")
    except OSError as exc:
        raise LauncherError(f"Не удалось прочитать NBT: {path}") from exc
    if raw.startswith(b"\x1f\x8b"):
        try:
            decompressor = zlib.decompressobj(16 + zlib.MAX_WBITS)
            raw = decompressor.decompress(raw, MAX_NBT_DECOMPRESSED_BYTES + 1)
        except zlib.error as exc:
            raise LauncherError(f"Некорректный сжатый NBT-файл: {path.name}") from exc
        if (
            len(raw) > MAX_NBT_DECOMPRESSED_BYTES
            or decompressor.unconsumed_tail
            or decompressor.unused_data
            or not decompressor.eof
        ):
            raise LauncherError(f"Распакованный NBT-файл слишком большой или повреждён: {path.name}")
    try:
        value = _NbtReader(raw).read_root()
    except (IndexError, ValueError, UnicodeDecodeError, RecursionError) as exc:
        raise LauncherError(f"Некорректный NBT-файл: {path.name}") from exc
    return value if isinstance(value, dict) else {}


class _NbtReader:
    def __init__(self, raw: bytes) -> None:
        self.raw = raw
        self.offset = 0
        self.nodes = 0

    def read_root(self) -> Any:
        tag = self._byte()
        if tag == 0:
            return {}
        self._string()
        return self._payload(tag)

    def _payload(self, tag: int, depth: int = 0) -> Any:
        self.nodes += 1
        if depth > MAX_NBT_DEPTH or self.nodes > MAX_NBT_NODES:
            raise ValueError("NBT structure is too complex")
        if tag == 1:
            return self._signed(1)
        if tag == 2:
            return self._signed(2)
        if tag == 3:
            return self._signed(4)
        if tag == 4:
            return self._signed(8)
        if tag == 5:
            return self._float(4)
        if tag == 6:
            return self._float(8)
        if tag == 7:
            length = self._signed(4)
            return self._take(length)
        if tag == 8:
            return self._string()
        if tag == 9:
            child, length = self._byte(), self._signed(4)
            if length < 0 or length > MAX_NBT_COLLECTION_ITEMS:
                raise ValueError("NBT list is too large")
            return [self._payload(child, depth + 1) for _ in range(max(0, length))]
        if tag == 10:
            value: dict[str, Any] = {}
            while True:
                child = self._byte()
                if child == 0:
                    return value
                name = self._string()
                value[name] = self._payload(child, depth + 1)
        if tag == 11:
            length = self._signed(4)
            if length < 0 or length > MAX_NBT_COLLECTION_ITEMS:
                raise ValueError("NBT int array is too large")
            return [self._signed(4) for _ in range(length)]
        if tag == 12:
            length = self._signed(4)
            if length < 0 or length > MAX_NBT_COLLECTION_ITEMS:
                raise ValueError("NBT long array is too large")
            return [self._signed(8) for _ in range(length)]
        raise ValueError(f"unsupported NBT tag {tag}")

    def _byte(self) -> int:
        value = self._take(1)[0]
        return value

    def _signed(self, size: int) -> int:
        return int.from_bytes(self._take(size), "big", signed=True)

    def _float(self, size: int) -> float:
        import struct

        return struct.unpack(">f" if size == 4 else ">d", self._take(size))[0]

    def _string(self) -> str:
        length = int.from_bytes(self._take(2), "big")
        if length > 1_048_576:
            raise ValueError("NBT string is too large")
        return self._take(length).decode("utf-8")

    def _take(self, size: int) -> bytes:
        if size < 0 or self.offset + size > len(self.raw):
            raise IndexError("NBT buffer exhausted")
        value = self.raw[self.offset : self.offset + size]
        self.offset += size
        return value


def _write_private_file(path: Path, content: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        fd = os.open(path, flags, 0o600)
    except FileExistsError:
        raise LauncherError("Файл ресурса уже существует")
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
    except Exception:
        try:
            path.unlink()
        except OSError:
            pass
        raise
