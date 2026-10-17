from __future__ import annotations

import base64
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .config import ConfigStore, _atomic_json_write
from .constants import (
    MAX_INSTANCE_LIVE_STATE_JSON_BYTES,
    MAX_INSTANCE_PROFILE_JSON_BYTES,
    MAX_INSTANCE_VERSION_JSON_BYTES,
    MAX_SERVER_CONFIG_JSON_BYTES,
)
from .errors import LauncherError
from .metadata import safe_instance_name
from .platform_support import is_reparse_point, process_exists
from .security import CELL_DIRECTORY, SERVICE_JSON_FILES, clear_protected_settings, load_protected_settings, save_protected_settings


@dataclass
class InstanceInfo:
    name: str
    path: Path
    version_id: str = ""
    minecraft_version: str = ""
    loader: str = "vanilla"
    display_name: str = ""
    description: str = ""
    group: str = ""
    icon: str = ""
    banner: str = ""
    played_seconds: int = 0
    kind: str = "client"
    game_edition: str = "java"
    bedrock_profile: str = ""
    server_core: str = ""
    server_core_version: str = ""
    server_port: int = 0
    server_eula: bool = False
    running: bool = False
    notes: str = ""
    tags: list[str] = field(default_factory=list)


@dataclass
class ExternalImportCandidate:
    launcher: str
    source_path: Path
    game_path: Path
    display_name: str
    suggested_name: str
    version_id: str = ""
    minecraft_version: str = ""
    loader: str = "vanilla"
    notes: str = ""


INSTANCE_METADATA_FILE = ".trial-launcher-instance.json"
INSTANCE_ASSETS_DIR = ".trial-launcher-assets"


class InstanceStore:
    def __init__(self, config: ConfigStore) -> None:
        self.config = config

    @property
    def root(self) -> Path:
        path = self.config.game_dir / "instances"
        if is_reparse_point(path):
            raise LauncherError("Каталог экземпляров является ссылкой или junction")
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        if is_reparse_point(path) or not path.is_dir():
            raise LauncherError("Каталог экземпляров недоступен")
        try:
            path.chmod(0o700)
        except OSError:
            pass
        return path

    def list(self) -> list[InstanceInfo]:
        values: list[InstanceInfo] = []
        for path in sorted(self.root.iterdir(), key=lambda item: item.name.lower()):
            if is_reparse_point(path) or not path.is_dir():
                continue
            values.append(self._inspect(path))
        return values

    def get(self, name: str) -> InstanceInfo:
        safe_name = safe_instance_name(name)
        path = self.root / safe_name
        if is_reparse_point(path) or not path.is_dir() or not _within(path.resolve(), self.root.resolve()):
            raise LauncherError(f"Экземпляр не найден: {name}")
        return self._inspect(path)

    def delete(self, name: str) -> None:
        value = self.get(name)
        if _instance_running(value.path):
            raise LauncherError("Нельзя удалить запущенный экземпляр")
        if value.game_edition == "bedrock":
            from .bedrock import BedrockProfileStore

            BedrockProfileStore(self.config, self).delete_profile(value)
        shutil.rmtree(value.path)
        clear_protected_settings(self.config, value.name)

    def duplicate(self, name: str, target_name: str) -> InstanceInfo:
        source = self.get(name)
        if source.game_edition == "bedrock":
            raise LauncherError("Профиль Bedrock нельзя копировать вместе с данными входа Microsoft")
        target = self.root / safe_instance_name(target_name)
        if os.path.lexists(target):
            raise LauncherError(f"Экземпляр уже существует: {target.name}")
        clear_protected_settings(self.config, target.name)
        _assert_no_symlinks(source.path)
        shutil.copytree(source.path, target, copy_function=shutil.copy2)
        target.chmod(0o700)
        # Java и профиль безопасности лежат вне экземпляра — переносим вручную,
        # иначе копия молча откатится на глобальную Java.
        from .instance_workspace import INSTANCE_SECURITY_DEFAULTS

        try:
            save_protected_settings(self.config, target.name, load_protected_settings(self.config, source.name, INSTANCE_SECURITY_DEFAULTS))
        except LauncherError:
            pass
        return self._inspect(target)

    def export_zip(self, name: str, destination: Path) -> Path:
        value = self.get(name)
        if value.game_edition == "bedrock":
            raise LauncherError("Архивирование профиля Bedrock пока не поддерживается")
        destination = destination.expanduser().resolve()
        if _within(destination, value.path.resolve()):
            raise LauncherError("ZIP-архив нельзя создавать внутри экспортируемого экземпляра")
        destination.parent.mkdir(parents=True, exist_ok=True)
        _assert_no_symlinks(value.path)
        fd, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.", suffix=".part", dir=destination.parent)
        os.close(fd)
        temporary = Path(temporary_name)
        try:
            with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
                for path in value.path.rglob("*"):
                    relative = path.relative_to(value.path)
                    if (
                        path.is_file()
                        and not is_reparse_point(path)
                        and CELL_DIRECTORY not in relative.parts
                        and path.name not in SERVICE_JSON_FILES
                        and not path.name.startswith(".trial-loader-installer-")
                    ):
                        archive.write(path, path.relative_to(value.path.parent))
            temporary.chmod(0o600)
            temporary.replace(destination)
        finally:
            if temporary.exists():
                temporary.unlink()
        return destination

    def create_shortcut(self, name: str, destination: Path | None = None) -> Path:
        value = self.get(name)
        if value.game_edition == "bedrock":
            raise LauncherError("Запускайте Bedrock из Nivora, чтобы проверить версию рантайма и лицензию")
        target_dir = (destination or (Path.home() / "Desktop")).expanduser()
        target_dir.mkdir(parents=True, exist_ok=True)
        if os.name == "nt":
            shortcut = target_dir / f"Nivora Launcher - {value.name}.cmd"
            launch_version = value.minecraft_version or value.version_id or value.name
            command = [*_launcher_cli_command(), "launch", launch_version, "--instance", value.name, "--account", "auto"]
            if value.loader != "vanilla":
                command.extend(["--loader", value.loader])
            shortcut.write_text(
                "@echo off\r\n" + subprocess.list2cmdline(command) + "\r\n",
                encoding="utf-8",
            )
            return shortcut
        shortcut = target_dir / f"Nivora Launcher - {value.name}.desktop"
        launch_version = value.minecraft_version or value.version_id or value.name
        command = " ".join(
            _desktop_exec_argument(argument)
            for argument in (*_launcher_cli_command(), "launch", launch_version)
        )
        if value.loader != "vanilla":
            command += " --loader " + _desktop_exec_argument(value.loader)
        command += " --instance " + _desktop_exec_argument(value.name) + " --account auto"
        content = (
            "[Desktop Entry]\n"
            "Type=Application\n"
            f"Name=Nivora Launcher - {value.name}\n"
            f"Exec={command}\n"
            "Terminal=false\n"
            "Categories=Game;\n"
        )
        shortcut.write_text(content, encoding="utf-8")
        shortcut.chmod(0o755)
        return shortcut

    def update_profile(
        self,
        name: str,
        *,
        display_name: str,
        description: str,
        group: str,
        icon_file_name: str = "",
        icon_data: str = "",
        banner_file_name: str = "",
        banner_data: str = "",
        clear_icon: bool = False,
        clear_banner: bool = False,
        notes: str | None = None,
        tags: list[str] | None = None,
    ) -> InstanceInfo:
        value = self.get(name)
        group = group.strip()[:40]
        if group and group not in self.config.settings.instance_groups:
            raise LauncherError("Сначала создайте выбранную группу")
        metadata_path = value.path / INSTANCE_METADATA_FILE
        metadata = self._read_profile(value.path)
        metadata.update(
            {
                "display_name": display_name.strip()[:80] or value.name,
                "description": description.strip()[:500],
                "group": group,
                "played_seconds": _nonnegative_int(metadata.get("played_seconds", 0)),
            }
        )
        if notes is not None:
            metadata["notes"] = notes.strip()[:4000]
        if tags is not None:
            metadata["tags"] = _normalize_tags(tags)
        assets = value.path / INSTANCE_ASSETS_DIR
        if clear_icon:
            _remove_profile_assets(assets, "icon")
            metadata.pop("icon_file", None)
        elif icon_data:
            metadata["icon_file"] = self._write_profile_asset(assets, "icon", icon_file_name, icon_data, 2 * 1024 * 1024)
        if clear_banner:
            _remove_profile_assets(assets, "banner")
            metadata.pop("banner_file", None)
        elif banner_data:
            metadata["banner_file"] = self._write_profile_asset(assets, "banner", banner_file_name, banner_data, 8 * 1024 * 1024)
        _atomic_json_write(metadata_path, metadata, private=True)
        return self.get(name)

    def add_played_seconds(self, path: Path, seconds: int) -> None:
        root = self.root.resolve()
        target = path.resolve()
        if not _within(target, root):
            raise LauncherError("Путь экземпляра находится вне каталога игры")
        metadata_path = target / INSTANCE_METADATA_FILE
        metadata = self._read_profile(target)
        metadata["played_seconds"] = _nonnegative_int(metadata.get("played_seconds", 0)) + max(0, int(seconds))
        _atomic_json_write(metadata_path, metadata, private=True)

    def groups(self) -> list[str]:
        return list(self.config.settings.instance_groups)

    def create_group(self, name: str) -> list[str]:
        value = name.strip()[:40]
        if not value:
            raise LauncherError("Название группы не может быть пустым")
        groups = list(self.config.settings.instance_groups)
        if value not in groups:
            groups.append(value)
            self.config.set_value("instance_groups", groups)
        return self.groups()

    def delete_group(self, name: str) -> list[str]:
        value = name.strip()
        if any(item.group == value for item in self.list()):
            raise LauncherError("Нельзя удалить группу, пока в ней есть экземпляры")
        self.config.set_value("instance_groups", [item for item in self.config.settings.instance_groups if item != value])
        return self.groups()

    def rename_group(self, old_name: str, new_name: str) -> list[str]:
        """Переименование группы с переносом экземпляров: группа — просто
        метка в настройках + поле group у экземпляров, поэтому меняем оба."""
        old_value = old_name.strip()
        value = new_name.strip()[:40]
        if not value:
            raise LauncherError("Название группы не может быть пустым")
        groups = list(self.config.settings.instance_groups)
        if old_value not in groups:
            raise LauncherError("Группа не найдена")
        if value == old_value:
            return self.groups()
        if value in groups:
            raise LauncherError("Группа с таким названием уже существует")
        groups[groups.index(old_value)] = value
        # Закрепления ссылаются на имя группы: читаем их ДО обновления
        # списка групп — нормализация настроек отвязывает несуществующие
        # имена, поэтому сначала переносим pin на новое имя.
        pinned = [value if item == old_value else item for item in self.config.settings.pinned_instance_groups]
        self.config.set_value("instance_groups", groups)
        self.config.set_value("pinned_instance_groups", pinned)
        for info in self.list():
            if info.group == old_value:
                self.update_profile(
                    info.name,
                    display_name=info.display_name,
                    description=info.description,
                    group=value,
                )
        return self.groups()

    def list_import_candidates(self) -> list[ExternalImportCandidate]:
        seen: set[Path] = set()
        values: list[ExternalImportCandidate] = []
        for candidate in _discover_external_instance_candidates():
            try:
                resolved = candidate.source_path.expanduser().resolve()
            except OSError:
                continue
            if resolved in seen:
                continue
            seen.add(resolved)
            values.append(candidate)
        return values

    def import_external_instance(self, source_path: str, *, target_name: str | None = None) -> InstanceInfo:
        requested = Path(source_path).expanduser()
        try:
            requested_resolved = requested.resolve()
        except OSError as exc:
            raise LauncherError("Источник импорта недоступен") from exc
        candidate = next(
            (item for item in self.list_import_candidates() if item.source_path.expanduser().resolve() == requested_resolved),
            None,
        )
        if candidate is None:
            raise LauncherError("Источник импорта не найден. Обновите список и попробуйте снова.")
        source_root = candidate.game_path.expanduser()
        if is_reparse_point(source_root) or not source_root.is_dir():
            raise LauncherError("Каталог исходного экземпляра недоступен")
        target_label = (target_name or candidate.suggested_name or candidate.display_name or candidate.source_path.name).strip()
        target = self.root / safe_instance_name(target_label)
        if os.path.lexists(target):
            raise LauncherError(f"Экземпляр уже существует: {target.name}")
        clear_protected_settings(self.config, target.name)
        _assert_no_symlinks(source_root)
        shutil.copytree(source_root, target, copy_function=shutil.copy2)
        target.chmod(0o700)
        _ensure_import_version_metadata(target, candidate)
        metadata_path = target / INSTANCE_METADATA_FILE
        metadata = self._read_profile(target)
        metadata.update(
            {
                "display_name": candidate.display_name.strip()[:80] or target.name,
                "description": (candidate.notes.strip() or f"Импортировано из {candidate.launcher}.")[:500],
                "kind": "client",
                "played_seconds": _nonnegative_int(metadata.get("played_seconds", 0)),
            }
        )
        _atomic_json_write(metadata_path, metadata, private=True)
        return self._inspect(target)

    def _inspect(self, path: Path) -> InstanceInfo:
        profile = self._read_profile(path)
        profile_values = self._profile_public(path, profile)
        if profile_values["game_edition"] == "bedrock":
            return InstanceInfo(path.name, path, **profile_values)
        candidates: list[tuple[float, Path, dict[str, Any]]] = []
        versions = path / "versions"
        if versions.exists() and not is_reparse_point(versions):
            instance_root = path.resolve()
            for metadata in versions.glob("*/*.json"):
                if is_reparse_point(metadata) or is_reparse_point(metadata.parent) or not metadata.is_file():
                    continue
                try:
                    if not _within(metadata.resolve(), instance_root):
                        continue
                    if metadata.stat().st_size > MAX_INSTANCE_VERSION_JSON_BYTES:
                        continue
                    value = json.loads(metadata.read_text(encoding="utf-8"))
                    modified_at = metadata.stat().st_mtime
                except (OSError, ValueError, TypeError, RecursionError):
                    continue
                if not isinstance(value, dict):
                    continue
                candidates.append((modified_at, metadata, value))
        if not candidates:
            return InstanceInfo(path.name, path, **profile_values)
        _, metadata, value = max(candidates, key=lambda item: item[0])
        version_id = str(value.get("id") or metadata.stem)
        if len(version_id) > 200 or not re.fullmatch(r"[A-Za-z0-9._+-]+", version_id) or version_id in {".", ".."}:
            version_id = metadata.stem
        if len(version_id) > 200 or not re.fullmatch(r"[A-Za-z0-9._+-]+", version_id) or version_id in {".", ".."}:
            return InstanceInfo(path.name, path, **profile_values)
        minecraft_version = str(value.get("inheritsFrom") or value.get("minecraft") or "")[:80]
        if not minecraft_version:
            matches = re.findall(r"\d+\.\d+(?:\.\d+)?", version_id)
            preferred = [match for match in reversed(matches) if match.startswith(("1.", "2."))]
            minecraft_version = (preferred or matches or [version_id])[-1]
        loader = _loader_from_version(version_id)
        return InstanceInfo(path.name, path, version_id=version_id, minecraft_version=minecraft_version, loader=loader, **profile_values)

    @staticmethod
    def _read_profile(path: Path) -> dict[str, Any]:
        # Метаданные писались в .trial-launcher-instance.json; Rust-движок
        # пишет .nivora-launcher-instance.json (bedrock-экземпляры и нативный
        # install). Читаем оба, предпочитаем «питоновский» файл — его пишут
        # операции обновления профиля, работающие через воркер.
        for metadata_name in (INSTANCE_METADATA_FILE, ".nivora-launcher-instance.json"):
            metadata_path = path / metadata_name
            if is_reparse_point(metadata_path) or not metadata_path.exists():
                continue
            try:
                if metadata_path.stat().st_size > MAX_INSTANCE_PROFILE_JSON_BYTES:
                    return {}
                value = json.loads(metadata_path.read_text(encoding="utf-8"))
                return value if isinstance(value, dict) else {}
            except (OSError, ValueError, TypeError, RecursionError):
                return {}
        return {}

    @classmethod
    def _profile_public(cls, path: Path, metadata: dict[str, Any]) -> dict[str, Any]:
        running_seconds = _live_play_seconds(path)
        played = _nonnegative_int(metadata.get("played_seconds", 0))
        display_name = str(metadata.get("display_name", "")).strip()[:80]
        icon_file = str(metadata.get("icon_file", "")).strip()
        banner_file = str(metadata.get("banner_file", "")).strip()
        server_config: dict[str, Any] = {}
        server_config_path = path / ".trial-launcher-server.json"
        try:
            if (
                server_config_path.is_file()
                and not is_reparse_point(server_config_path)
                and server_config_path.stat().st_size <= MAX_SERVER_CONFIG_JSON_BYTES
            ):
                value = json.loads(server_config_path.read_text(encoding="utf-8"))
                if isinstance(value, dict):
                    server_config = value
        except (OSError, ValueError, TypeError, RecursionError):
            server_config = {}
        is_server = str(metadata.get("kind", "")).strip().lower() == "server" or bool(server_config)
        game_edition = str(metadata.get("game_edition", "java")).strip().lower()
        if game_edition not in {"java", "bedrock"} or is_server:
            game_edition = "java"
        return {
            "display_name": display_name or path.name,
            "description": str(metadata.get("description", "")).strip()[:500],
            "group": str(metadata.get("group", "")).strip()[:40],
            "icon": _image_data_url(path / INSTANCE_ASSETS_DIR / icon_file, 2 * 1024 * 1024) if icon_file else "",
            "banner": _image_data_url(path / INSTANCE_ASSETS_DIR / banner_file, 8 * 1024 * 1024) if banner_file else "",
            "played_seconds": played + running_seconds,
            "kind": "server" if is_server else "client",
            "game_edition": game_edition,
            "bedrock_profile": str(metadata.get("bedrock_profile", "")).strip()[:64] if game_edition == "bedrock" else "",
            "server_core": str(metadata.get("server_core") or server_config.get("core", "")).strip()[:64],
            "server_core_version": str(metadata.get("server_core_version") or server_config.get("version", "")).strip()[:32],
            "server_port": _nonnegative_int(metadata.get("server_port") or server_config.get("port", 0)),
            "server_eula": bool(metadata.get("server_eula", server_config.get("eula_accepted", False))),
            "running": _instance_running(path),
            "notes": str(metadata.get("notes", ""))[:4000],
            "tags": _normalize_tags(metadata.get("tags")),
        }

    def _write_profile_asset(self, assets: Path, kind: str, file_name: str, encoded: str, limit: int) -> str:
        normalized = file_name.replace("\\", "/")
        safe_name = Path(normalized).name
        suffix = Path(safe_name).suffix.lower()
        if not safe_name or safe_name != normalized or suffix not in {".png", ".jpg", ".jpeg", ".webp"}:
            raise LauncherError("Поддерживаются только PNG, JPEG и WebP")
        try:
            content = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as exc:
            raise LauncherError("Изображение профиля повреждено") from exc
        if len(content) > limit or not _valid_image(content, suffix):
            raise LauncherError("Изображение профиля имеет неверный формат или слишком большое")
        assets.mkdir(mode=0o700, parents=True, exist_ok=True)
        if is_reparse_point(assets):
            raise LauncherError("Каталог изображений экземпляра является символьной ссылкой")
        _remove_profile_assets(assets, kind)
        destination = assets / f"{kind}{suffix}"
        fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
        return destination.name


def _loader_from_version(version_id: str) -> str:
    lowered = version_id.lower()
    for name in ("neoforge", "forge", "quilt", "fabric"):
        if name in lowered:
            return name
    return "vanilla"


def _valid_image(content: bytes, suffix: str) -> bool:
    if suffix == ".png":
        return content.startswith(b"\x89PNG\r\n\x1a\n")
    if suffix in {".jpg", ".jpeg"}:
        return content.startswith(b"\xff\xd8\xff")
    return len(content) >= 12 and content[:4] == b"RIFF" and content[8:12] == b"WEBP"


def _remove_profile_assets(directory: Path, kind: str) -> None:
    if not directory.exists():
        return
    for path in directory.glob(f"{kind}.*"):
        if is_reparse_point(path):
            raise LauncherError("Изображение профиля является символьной ссылкой")
        if path.is_file():
            path.unlink()


def _image_data_url(path: Path, limit: int) -> str:
    if is_reparse_point(path) or not path.is_file():
        return ""
    try:
        if path.stat().st_size > limit:
            return ""
        content = path.read_bytes()
    except OSError:
        return ""
    suffix = path.suffix.lower()
    if not _valid_image(content, suffix):
        return ""
    mime = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp"}[suffix]
    return f"data:{mime};base64," + base64.b64encode(content).decode("ascii")


def _live_play_seconds(path: Path) -> int:
    state_path = path / ".trial-launcher-live.json"
    try:
        if (
            is_reparse_point(state_path)
            or not state_path.is_file()
            or state_path.stat().st_size > MAX_INSTANCE_LIVE_STATE_JSON_BYTES
        ):
            return 0
        value = json.loads(state_path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            return 0
        if not value.get("running"):
            return 0
        started_at = float(value.get("started_at", 0))
        return max(0, int(time.time() - started_at)) if started_at > 0 else 0
    except (OSError, ValueError, TypeError, RecursionError):
        return 0


def _nonnegative_int(value: Any) -> int:
    try:
        return max(0, int(value or 0))
    except (TypeError, ValueError):
        return 0


def _normalize_tags(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    tags: list[str] = []
    for item in value[:24]:
        tag = str(item).strip().lstrip("#")[:32]
        if tag and tag not in tags:
            tags.append(tag)
    return tags[:12]


def _discover_external_instance_candidates(home: Path | None = None) -> list[ExternalImportCandidate]:
    user_home = (home or Path.home()).expanduser()
    values: list[ExternalImportCandidate] = []
    prism_roots = [
        user_home / ".local/share/PrismLauncher/instances",
        user_home / ".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances",
    ]
    multimc_roots = [
        user_home / ".local/share/MultiMC/instances",
        user_home / ".config/MultiMC/instances",
        user_home / ".config/multimc/instances",
    ]
    curseforge_roots = [user_home / "Documents/curseforge/minecraft/Instances"]
    official_roots = [
        user_home / ".minecraft",
        user_home / "snap/minecraft-launcher/common/.minecraft",
        user_home / ".var/app/com.mojang.Minecraft/data/minecraft",
    ]
    appdata = os.environ.get("APPDATA", "").strip() if os.name == "nt" else ""
    if appdata:
        # Windows: лаунчеры живут в %APPDATA%, а не в домашнем каталоге.
        prism_roots.append(Path(appdata) / "PrismLauncher" / "instances")
        multimc_roots.append(Path(appdata) / "MultiMC" / "instances")
        curseforge_roots.append(user_home / "curseforge" / "minecraft" / "Instances")
        official_roots.insert(0, Path(appdata) / ".minecraft")
    for root in prism_roots:
        values.extend(_discover_prism_family_instances(root, "Prism Launcher"))
    for root in multimc_roots:
        values.extend(_discover_prism_family_instances(root, "MultiMC"))
    for root in curseforge_roots:
        values.extend(_discover_curseforge_instances(root))
    for root in official_roots:
        candidate = _discover_official_launcher_instance(root)
        if candidate is not None:
            values.append(candidate)
    return values


def _discover_prism_family_instances(root: Path, launcher: str) -> list[ExternalImportCandidate]:
    if is_reparse_point(root) or not root.is_dir():
        return []
    values: list[ExternalImportCandidate] = []
    for path in sorted(root.iterdir(), key=lambda item: item.name.lower()):
        if is_reparse_point(path) or not path.is_dir():
            continue
        if not any((path / name).is_file() and not is_reparse_point(path / name) for name in ("instance.cfg", "mmc-pack.json", "version.json")):
            continue
        game_path = path / "minecraft"
        if is_reparse_point(game_path) or not game_path.is_dir():
            game_path = path
        candidate = _prism_family_candidate(launcher, path, game_path)
        if candidate is not None:
            values.append(candidate)
    return values


def _prism_family_candidate(launcher: str, source_path: Path, game_path: Path) -> ExternalImportCandidate | None:
    config = _read_key_value_file(source_path / "instance.cfg")
    pack = _read_json_file(source_path / "mmc-pack.json")
    version_info = _read_json_file(source_path / "version.json")
    display_name = str(config.get("name") or (pack or {}).get("name") or source_path.name).strip()[:80] or source_path.name
    minecraft_version = ""
    loader = "vanilla"
    version_id = str((version_info or {}).get("id") or "").strip()[:120]
    components = (pack or {}).get("components", []) if isinstance(pack, dict) else []
    if isinstance(components, list):
        loader_version = ""
        for item in components:
            if not isinstance(item, dict):
                continue
            uid = str(item.get("uid") or "").strip().lower()
            version = str(item.get("version") or "").strip()
            if uid == "net.minecraft" and version:
                minecraft_version = version[:80]
            elif "fabric-loader" in uid and version:
                loader, loader_version = "fabric", version
            elif "quilt-loader" in uid and version:
                loader, loader_version = "quilt", version
            elif "neoforge" in uid and version:
                loader, loader_version = "neoforge", version
            elif ("minecraftforge" in uid or uid.endswith("forge")) and version and loader == "vanilla":
                loader, loader_version = "forge", version
        if not version_id:
            if loader != "vanilla" and loader_version and minecraft_version:
                version_id = f"{loader}-{loader_version}-{minecraft_version}"[:120]
            elif minecraft_version:
                version_id = minecraft_version
    if isinstance(version_info, dict):
        minecraft_version = str(version_info.get("inheritsFrom") or version_info.get("minecraft") or minecraft_version)[:80]
        if not version_id:
            version_id = str(version_info.get("id") or "")[:120]
    return ExternalImportCandidate(
        launcher=launcher,
        source_path=source_path,
        game_path=game_path,
        display_name=display_name,
        suggested_name=display_name,
        version_id=version_id,
        minecraft_version=minecraft_version,
        loader=loader,
        notes=f"Импортировано из {launcher}.",
    )


def _discover_curseforge_instances(root: Path) -> list[ExternalImportCandidate]:
    if is_reparse_point(root) or not root.is_dir():
        return []
    values: list[ExternalImportCandidate] = []
    for path in sorted(root.iterdir(), key=lambda item: item.name.lower()):
        if is_reparse_point(path) or not path.is_dir():
            continue
        if not any((path / name).exists() and not is_reparse_point(path / name) for name in ("minecraftinstance.json", "manifest.json", "minecraft")):
            continue
        game_path = path / "minecraft"
        if is_reparse_point(game_path) or not game_path.is_dir():
            game_path = path
        details = _read_json_file(path / "minecraftinstance.json") or {}
        manifest = _read_json_file(path / "manifest.json") or {}
        display_name = str(details.get("name") or manifest.get("name") or path.name).strip()[:80] or path.name
        minecraft = manifest.get("minecraft") if isinstance(manifest, dict) else {}
        minecraft_version = str((minecraft or {}).get("version") or details.get("gameVersion") or "")[:80]
        base_loader = details.get("baseModLoader")
        if isinstance(base_loader, dict):
            base_loader = base_loader.get("name") or base_loader.get("id")
        mod_loader_token = str(base_loader or "").strip()
        if not mod_loader_token and isinstance(minecraft, dict):
            loaders = minecraft.get("modLoaders", [])
            if isinstance(loaders, list):
                for item in loaders:
                    if isinstance(item, dict) and item.get("id"):
                        mod_loader_token = str(item.get("id"))
                        break
        loader, version_id, minecraft_version = _loader_details_from_token(mod_loader_token, minecraft_version)
        return_candidate = ExternalImportCandidate(
            launcher="CurseForge",
            source_path=path,
            game_path=game_path,
            display_name=display_name,
            suggested_name=display_name,
            version_id=version_id or minecraft_version,
            minecraft_version=minecraft_version,
            loader=loader,
            notes="Импортировано из CurseForge.",
        )
        values.append(return_candidate)
    return values


def _discover_official_launcher_instance(root: Path) -> ExternalImportCandidate | None:
    if is_reparse_point(root) or not root.is_dir():
        return None
    if not any((root / name).exists() and not is_reparse_point(root / name) for name in ("versions", "saves", "mods", "resourcepacks")):
        return None
    version_id = ""
    minecraft_version = ""
    versions = root / "versions"
    if not is_reparse_point(versions) and versions.is_dir():
        json_files = [path for path in versions.glob("*/*.json") if path.is_file() and not is_reparse_point(path)]
        if json_files:
            metadata_path = max(json_files, key=lambda item: item.stat().st_mtime)
            metadata = _read_json_file(metadata_path) or {}
            version_id = str(metadata.get("id") or metadata_path.stem)[:120]
            minecraft_version = str(metadata.get("inheritsFrom") or metadata.get("minecraft") or "")[:80]
            if not minecraft_version:
                matches = re.findall(r"\d+\.\d+(?:\.\d+)?", version_id)
                preferred = [match for match in reversed(matches) if match.startswith(("1.", "2."))]
                minecraft_version = (preferred or matches or [""])[-1][:80]
    return ExternalImportCandidate(
        launcher="Minecraft Launcher",
        source_path=root,
        game_path=root,
        display_name="Official Minecraft",
        suggested_name="official-minecraft",
        version_id=version_id or minecraft_version,
        minecraft_version=minecraft_version,
        loader=_loader_from_version(version_id or minecraft_version),
        notes="Импортировано из официального лаунчера Minecraft.",
    )


def _loader_details_from_token(token: str, minecraft_version: str) -> tuple[str, str, str]:
    lowered = token.strip().lower()
    loader = "vanilla"
    if lowered:
        for name in ("neoforge", "fabric", "quilt", "forge"):
            if name in lowered:
                loader = name
                break
    if not minecraft_version:
        matches = re.findall(r"\d+\.\d+(?:\.\d+)?", token)
        preferred = [match for match in reversed(matches) if match.startswith(("1.", "2."))]
        minecraft_version = (preferred or matches or [""])[-1][:80]
    version_id = token.strip()[:120]
    if not version_id:
        version_id = f"{loader}-{minecraft_version}" if loader != "vanilla" and minecraft_version else minecraft_version
    elif loader != "vanilla" and loader not in version_id.lower():
        version_id = f"{loader}-{version_id}"[:120]
    return loader, version_id, minecraft_version


def _read_json_file(path: Path, limit: int = MAX_INSTANCE_PROFILE_JSON_BYTES) -> dict[str, Any] | None:
    try:
        if is_reparse_point(path) or not path.is_file() or path.stat().st_size > limit:
            return None
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError, RecursionError):
        return None
    return value if isinstance(value, dict) else None


def _read_key_value_file(path: Path, limit: int = 128 * 1024) -> dict[str, str]:
    try:
        if is_reparse_point(path) or not path.is_file() or path.stat().st_size > limit:
            return {}
        values: dict[str, str] = {}
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if "=" not in line or line.lstrip().startswith("#"):
                continue
            key, value = line.split("=", 1)
            key = key.strip()
            if key:
                values[key] = value.strip()
        return values
    except OSError:
        return {}


def _ensure_import_version_metadata(target: Path, candidate: ExternalImportCandidate) -> None:
    versions_dir = target / "versions"
    if not is_reparse_point(versions_dir) and any(versions_dir.glob("*/*.json")):
        return
    version_id = candidate.version_id or candidate.minecraft_version or candidate.suggested_name or target.name
    version_id = re.sub(r"[^A-Za-z0-9._+-]+", "-", version_id).strip(".-")[:120] or target.name
    payload: dict[str, Any] = {"id": version_id}
    if candidate.loader != "vanilla" and candidate.minecraft_version:
        payload["inheritsFrom"] = candidate.minecraft_version
        payload["minecraft"] = candidate.minecraft_version
    elif candidate.minecraft_version:
        payload["id"] = candidate.minecraft_version
    version_dir = versions_dir / str(payload["id"])
    version_dir.mkdir(parents=True, exist_ok=True)
    metadata_path = version_dir / f"{payload['id']}.json"
    metadata_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _desktop_exec_argument(value: str) -> str:
    if any(character in value for character in ("\x00", "\n", "\r")):
        raise LauncherError("Аргумент ярлыка содержит недопустимые символы")
    escaped = (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("`", "\\`")
        .replace("$", "\\$")
        .replace("%", "%%")
    )
    return f'"{escaped}"'


def _launcher_cli_command() -> list[str]:
    if getattr(sys, "frozen", False):
        return [sys.executable, "--cli"]
    return ["nivora-launcher"]


def _assert_no_symlinks(root: Path) -> None:
    for path in root.rglob("*"):
        if is_reparse_point(path):
            raise LauncherError(f"Экземпляр содержит небезопасную символьную ссылку: {path.name}")


def _instance_running(path: Path) -> bool:
    state_path = path / ".trial-launcher-live.json"
    try:
        if (
            is_reparse_point(state_path)
            or not state_path.is_file()
            or state_path.stat().st_size > MAX_INSTANCE_LIVE_STATE_JSON_BYTES
        ):
            return False
        value = json.loads(state_path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            return False
        pid = int(value.get("pid", 0)) if value.get("running") else 0
        if pid > 0:
            return process_exists(pid)
    except (OSError, ValueError, TypeError, RecursionError):
        return False
    return False
