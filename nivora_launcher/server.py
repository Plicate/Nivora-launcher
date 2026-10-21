from __future__ import annotations

import base64
import json
import os
import re
import signal
import shutil
import stat
import subprocess
import tempfile
import threading
import time
import uuid
from collections import deque
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from .config import (
    ConfigStore,
    _atomic_json_write,
    format_memory_kib,
    java_memory_value,
    parse_memory_kib,
    system_memory_kib,
)
from .constants import MAX_SERVER_CONFIG_JSON_BYTES, version_key
from .errors import LauncherError
from .instances import INSTANCE_METADATA_FILE, InstanceStore, _within
from .metrics import ProcessMetricsTracker, directory_size, read_metrics
from .net import HttpClient, file_hash
from .platform_support import is_reparse_point, process_exists, process_start_options, terminate_process_tree
from .portable_fs import atomic_write as atomic_write_path
from .portable_fs import regular_file, remove_tree as remove_tree_path, safe_path
from .process_guard import InstanceProcessLock
from .runtime import JavaRuntimeManager, java_version, safe_process_environment, validate_java_path
from .server_traffic import ServerTrafficTracker
from .security import load_protected_settings, prepare_process, save_protected_settings

SERVER_METADATA_FILE = ".trial-launcher-server.json"
SERVER_JAR = "server.jar"
SERVER_PROPERTIES_FILE = "server.properties"
MAX_SERVER_PROPERTIES_BYTES = 2 * 1024 * 1024
_UNKNOWN_RAM_SERVER_LIMIT_KIB = 2 * 1024 * 1024
SERVER_SECURITY_KEYS = frozenset({"java_path", "security_profile", "security_permissions"})
SERVER_SECURITY_DEFAULTS: dict[str, Any] = {
    "java_path": "",
    "security_profile": "strict",
    "security_permissions": ["network:game", "files:instance"],
}
_DIRECTORY_OPEN_FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
_NOFOLLOW = getattr(os, "O_NOFOLLOW", 0)
_NONBLOCK = getattr(os, "O_NONBLOCK", 0)
SERVER_CORES: tuple[dict[str, Any], ...] = (
    {"id": "paper", "name": "Paper", "family": "PaperMC", "description": "Быстрое стабильное ядро для Paper API."},
    {"id": "purpur", "name": "Purpur", "family": "PaperMC", "description": "Paper-совместимое ядро с расширенными настройками."},
    {"id": "folia", "name": "Folia", "family": "PaperMC", "description": "Региональный параллельный серверный runtime."},
    {"id": "spigot", "name": "Spigot", "family": "Bukkit", "description": "Классическое Bukkit-совместимое ядро."},
    {"id": "leaf", "name": "Leaf", "family": "Paper", "description": "Paper fork с дополнительными оптимизациями."},
    {"id": "pufferfish", "name": "Pufferfish", "family": "Paper", "description": "Оптимизированное Paper-совместимое ядро."},
    {"id": "patina", "name": "Patina", "family": "Paper", "description": "Производительный Paper-based дистрибутив."},
    {"id": "velocity", "name": "Velocity", "family": "Proxy", "description": "Современное прокси-ядро для сети серверов."},
    {"id": "waterfall", "name": "Waterfall", "family": "Proxy", "description": "BungeeCord-совместимое прокси-ядро."},
    {"id": "custom", "name": "Своё ядро", "family": "Custom", "description": "Загрузите любой совместимый server.jar."},
)

_SERVER_SOURCE_HOSTS = {
    "api.github.com",
    "api.leafmc.one",
    "api.papermc.io",
    "api.purpurmc.org",
    "ci.pufferfish.host",
    "fill-data.papermc.io",
    "fill.papermc.io",
    "github.com",
    "hub.spigotmc.org",
    "objects.githubusercontent.com",
    "piston-meta.mojang.com",
    "raw.githubusercontent.com",
    "release-assets.githubusercontent.com",
}

_MOJANG_VERSION_MANIFEST_URL = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"

def core_options() -> list[dict[str, Any]]:
    seen: set[str] = set()
    result: list[dict[str, Any]] = []
    for value in SERVER_CORES:
        if value["id"] in seen:
            continue
        seen.add(value["id"])
        result.append(dict(value))
    return result

def download_server_core(
    http: HttpClient,
    root: Path,
    core: str,
    version: str,
    build: str = "",
    *,
    confirm_unverified: bool = False,
) -> dict[str, Any]:
    if _server_is_running(root):
        raise LauncherError("Остановите сервер перед заменой ядра")
    core_id = str(core).strip().lower()
    minecraft_version = str(version).strip()
    if not minecraft_version or not re.fullmatch(r"[0-9A-Za-z._-]{1,32}", minecraft_version):
        raise LauncherError("Укажите корректную версию Minecraft для ядра")
    expected_sha256: str | None = None
    expected_size: int | None = None
    verified_by = ""
    if core_id == "purpur":
        url = f"https://api.purpurmc.org/v2/purpur/{minecraft_version}/latest/download"
        resolved_build = "latest"
    elif core_id in {"paper", "folia", "velocity", "waterfall"}:
        project = core_id
        builds_value = http.get_json(f"https://fill.papermc.io/v3/projects/{project}/versions/{minecraft_version}/builds")
        if isinstance(builds_value, list):
            builds = builds_value
            v3 = True
        else:
            builds = builds_value.get("builds", []) if isinstance(builds_value, dict) else []
            v3 = False
        if not isinstance(builds, list) or not builds:
            raise LauncherError(f"PaperMC не нашёл сборки {project} для {minecraft_version}")
        selected = next((item for item in builds if str(item.get("id", item.get("build"))) == str(build)), None) if build else None
        selected = selected or max(builds, key=lambda item: int(item.get("id", item.get("build", 0))))
        resolved_build = str(selected.get("id", selected.get("build", "")))
        if not resolved_build.isdigit():
            raise LauncherError("PaperMC вернул некорректный номер сборки")
        if v3:
            download = selected.get("downloads", {}).get("server:default", {}) if isinstance(selected, dict) else {}
            url = str(download.get("url", ""))
            expected_sha256 = str(download.get("checksums", {}).get("sha256", "")).lower() or None
        else:
            file_name = f"{project}-{minecraft_version}-{resolved_build}.jar"
            url = f"https://api.papermc.io/v2/projects/{project}/versions/{minecraft_version}/builds/{resolved_build}/downloads/{file_name}"
            download = selected.get("downloads", {}).get("application", {}) if isinstance(selected, dict) else {}
            expected_sha256 = str(download.get("sha256", "")).lower() or None
        expected_size = int(download.get("size")) if str(download.get("size", "")).isdigit() else None
        verified_by = "papermc-api" if expected_sha256 else ""
        if not url:
            raise LauncherError(f"PaperMC не вернул ссылку на сборку {project}")
    elif core_id == "leaf":
        builds_value = http.get_json(f"https://api.leafmc.one/v2/projects/leaf/versions/{minecraft_version}/builds")
        builds = builds_value.get("builds", []) if isinstance(builds_value, dict) else builds_value
        if not isinstance(builds, list) or not builds:
            raise LauncherError(f"Leaf не нашёл сборки для {minecraft_version}")
        selected = next((item for item in builds if str(item.get("build")) == str(build)), None) if build else None
        selected = selected or max(builds, key=lambda item: int(item.get("build", 0)))
        resolved_build = str(selected.get("build", ""))
        if not resolved_build.isdigit():
            raise LauncherError("Leaf вернул некорректный номер сборки")
        file_name = f"leaf-{minecraft_version}-{resolved_build}.jar"
        url = f"https://api.leafmc.one/v2/projects/leaf/versions/{minecraft_version}/builds/{resolved_build}/downloads/{file_name}"
        download = selected.get("downloads", {}).get("application", {}) if isinstance(selected, dict) else {}
        expected_sha256 = str(download.get("sha256", "")).lower() or None
        expected_size = int(download.get("size")) if str(download.get("size", "")).isdigit() else None
        verified_by = "leaf-api" if expected_sha256 else ""
    elif core_id == "pufferfish":
        branch = ".".join(minecraft_version.split(".")[:2])
        build_info = http.get_json(f"https://ci.pufferfish.host/job/Pufferfish-{branch}/lastSuccessfulBuild/api/json")
        artifacts = build_info.get("artifacts", []) if isinstance(build_info, dict) else []
        jars = [item for item in artifacts if str(item.get("fileName", "")).lower().endswith(".jar")]
        artifact = next(
            (
                item
                for item in jars
                if any(marker in str(item.get("fileName", "")).lower() for marker in ("paperclip", "pufferfish", "server"))
                and not any(marker in str(item.get("fileName", "")).lower() for marker in ("api", "sources", "javadoc"))
            ),
            jars[0] if jars else None,
        )
        build_url = str(build_info.get("url", f"https://ci.pufferfish.host/job/Pufferfish-{branch}/lastSuccessfulBuild/")).rstrip("/")
        relative_path = str((artifact or {}).get("relativePath", "")).strip()
        if not relative_path:
            raise LauncherError(f"Pufferfish не нашёл JAR для {minecraft_version}")
        url = f"{build_url}/artifact/{relative_path.lstrip('/')}"
        resolved_build = str(build_info.get("number", "latest"))
    elif core_id == "patina":
        url, resolved_build = _patina_download(http, minecraft_version, build)
    elif core_id == "spigot":
        return _build_spigot_with_buildtools(http, root, minecraft_version, confirm_unverified=confirm_unverified)
    else:
        raise LauncherError("Для этого ядра загрузка не автоматизирована. Загрузите совместимый server.jar вручную.")
    _validate_server_source_url(url)
    if not expected_sha256 and not confirm_unverified:
        raise LauncherError("[unverified-download] Серверный источник не предоставил доверенный хеш. Подтвердите скачивание явно.")
    destination = root / SERVER_JAR
    http.download(url, destination, sha256=expected_sha256, size=expected_size, redirect_validator=_validate_server_source_url)
    actual_sha256 = file_hash(destination, "sha256")
    return {
        "core": core_id,
        "version": minecraft_version,
        "build": resolved_build,
        "path": str(destination),
        "url": url,
        "sha256": actual_sha256,
        "verified_by": verified_by,
        "trusted_hash": bool(expected_sha256),
    }

def server_core_versions(http: HttpClient, core: str) -> list[str]:
    core_id = str(core).strip().lower()
    if core_id in {"paper", "folia", "velocity", "waterfall"}:
        value = http.get_json(f"https://fill.papermc.io/v3/projects/{core_id}")
    elif core_id == "purpur":
        value = http.get_json("https://api.purpurmc.org/v2/purpur")
    elif core_id == "leaf":
        value = http.get_json("https://api.leafmc.one/v2/projects/leaf")
    elif core_id == "pufferfish":
        try:
            branches = http.get_json("https://api.github.com/repos/pufferfish-gg/Pufferfish/branches?per_page=100")
            supported_majors = _github_branch_versions(branches, "ver/")
            if supported_majors:
                return [
                    version
                    for version in _mojang_release_versions(http)
                    if any(version == major or version.startswith(f"{major}.") for major in supported_majors)
                ]
        except LauncherError:
            pass
        return _mojang_release_versions(http)
    elif core_id == "patina":
        try:
            branches = http.get_json("https://api.github.com/repos/PatinaMC/Patina/branches?per_page=100")
            versions = _github_branch_versions(branches, "ver/")
            if versions:
                return _unique_versions(versions)
        except LauncherError:
            pass
        try:
            value = http.get_json("https://api.github.com/repos/PatinaMC/Patina/releases?per_page=30")
            versions = _patina_versions(value)
            if versions:
                return _unique_versions(versions)
        except LauncherError:
            pass
        return _mojang_release_versions(http)
    elif core_id == "spigot":
        return _mojang_release_versions(http)
    else:
        return []
    raw_versions = value.get("versions", []) if isinstance(value, dict) else []
    if isinstance(raw_versions, dict):
        raw_versions = [item for group in raw_versions.values() if isinstance(group, list) for item in group]
    return _unique_versions(raw_versions)

def _unique_versions(values: Any) -> list[str]:
    result: list[str] = []
    seen: set[str] = set()
    if not isinstance(values, list):
        return result
    for value in values:
        version = str(value).strip()
        if version and version not in seen:
            seen.add(version)
            result.append(version)
    # порядок каталога ненадёжен (Purpur отдаёт от старых к новым), а вместе с
    plain_release = re.compile(r"[0-9]+(?:\.[0-9]+)*")
    return sorted(
        (version for version in result if plain_release.fullmatch(version)),
        key=version_key,
        reverse=True,
    )[:200]

def _mojang_release_versions(http: HttpClient) -> list[str]:
    value = http.get_json(_MOJANG_VERSION_MANIFEST_URL)
    raw_versions = value.get("versions", []) if isinstance(value, dict) else []
    return _unique_versions(
        [item.get("id") for item in raw_versions if isinstance(item, dict) and item.get("type") == "release"]
    )

def _github_branch_versions(value: Any, prefix: str) -> list[str]:
    branches = value if isinstance(value, list) else []
    versions = []
    for branch in branches:
        name = str(branch.get("name", "")) if isinstance(branch, dict) else ""
        match = re.fullmatch(rf"{re.escape(prefix)}(\d+(?:\.\d+){{1,2}})", name)
        if match:
            versions.append(match.group(1))
    return _unique_versions(versions)

def _patina_versions(value: Any) -> list[str]:
    releases = value if isinstance(value, list) else []
    versions: list[str] = []
    for release in releases:
        if not isinstance(release, dict):
            continue
        text = f"{release.get('tag_name', '')} {release.get('name', '')}"
        versions.extend(re.findall(r"\b\d+\.\d+(?:\.\d+)?\b", text))
    return _unique_versions(versions)

def _patina_download(http: HttpClient, version: str, build: str) -> tuple[str, str]:
    repository = http.get_json("https://api.github.com/repos/PatinaMC/Patina")
    branch = str(repository.get("default_branch", "ver/1.21.1")) if isinstance(repository, dict) else "ver/1.21.1"
    if not re.fullmatch(r"[A-Za-z0-9._/-]{1,80}", branch):
        branch = "ver/1.21.1"
    readme_url = f"https://raw.githubusercontent.com/PatinaMC/Patina/{branch}/README.md"
    raw = http.request(readme_url, redirect_validator=_validate_server_source_url).decode("utf-8", errors="replace")
    links = re.findall(r"\]\((https://github\.com/PatinaMC/Patina/raw/[^)\s]+)\)", raw)
    major = ".".join(version.split(".")[:2])
    selected = next((link for link in links if f"/{version}/" in link), "")
    selected = selected or next((link for link in links if f"/{major}/" in link), "")
    if not selected:
        raise LauncherError(f"Patina не опубликовала официальный JAR для {version}")
    return selected, build or version

def _validate_server_source_url(url: str) -> None:
    from urllib.parse import urlsplit

    host = (urlsplit(url).hostname or "").lower().rstrip(".")
    if host not in _SERVER_SOURCE_HOSTS:
        raise LauncherError(f"Источник ядра не входит в официальный allowlist: {host or 'неизвестный хост'}")

def _build_spigot_with_buildtools(http: HttpClient, root: Path, version: str, *, confirm_unverified: bool) -> dict[str, Any]:
    if not confirm_unverified:
        raise LauncherError("[unverified-download] Spigot собирается официальным BuildTools без доступного SHA-256. Подтвердите скачивание явно.")
    java = shutil.which("java")
    if not java:
        raise LauncherError("Для сборки Spigot нужен Java executable")
    buildtools_url = "https://hub.spigotmc.org/jenkins/job/BuildTools/lastSuccessfulBuild/artifact/target/BuildTools.jar"
    _validate_server_source_url(buildtools_url)
    with tempfile.TemporaryDirectory(prefix="nivora-launcher-spigot-", dir=root) as directory:
        build_root = Path(directory)
        buildtools = build_root / "BuildTools.jar"
        http.download(buildtools_url, buildtools, redirect_validator=_validate_server_source_url)
        command, cwd, environment, warning = prepare_process(
            [java, "-jar", "BuildTools.jar", "--rev", version],
            build_root,
            safe_process_environment(compatibility=False),
            {"security_profile": "strict", "security_permissions": ["network:game", "files:instance"]},
        )
        try:
            completed = subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True, timeout=1800, check=False)
        except (OSError, subprocess.SubprocessError) as exc:
            raise LauncherError("Не удалось запустить официальный Spigot BuildTools") from exc
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout or "").strip()[-500:]
            raise LauncherError(f"BuildTools не собрал Spigot{f': {detail}' if detail else ''}")
        source = build_root / f"spigot-{version}.jar"
        if not source.is_file() or source.is_symlink():
            raise LauncherError("BuildTools завершился без spigot JAR")
        destination = root / SERVER_JAR
        if destination.is_symlink():
            raise LauncherError("Файл server.jar является символьной ссылкой")
        temporary = root / f".{SERVER_JAR}.{uuid.uuid4().hex}.part"
        temporary.write_bytes(source.read_bytes())
        temporary.replace(destination)
    return {
        "core": "spigot",
        "version": version,
        "build": "buildtools",
        "path": str(root / SERVER_JAR),
        "url": buildtools_url,
        "sha256": file_hash(root / SERVER_JAR, "sha256"),
        "verified_by": "spigot-buildtools" if warning is None else "",
        "trusted_hash": False,
    }

def _server_public(info: Any, config: dict[str, Any] | None = None) -> dict[str, Any]:
    value = config or {}
    return {
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
        "kind": "server",
        "server_core": str(value.get("core", getattr(info, "server_core", "custom"))),
        "server_core_version": str(value.get("version", getattr(info, "server_core_version", ""))),
        "server_port": int(value.get("port", getattr(info, "server_port", 25565)) or 25565),
        "server_eula": bool(value.get("eula_accepted", False)),
    }

class ServerInstanceStore:
    def __init__(self, config: ConfigStore, instances: InstanceStore) -> None:
        self.config_store = config
        self.instances = instances

    def get(self, name: str) -> Any:
        info = self.instances.get(name)
        if getattr(info, "kind", "client") != "server":
            raise LauncherError(f"Экземпляр «{name}» не является сервером")
        return info

    def config(self, name: str) -> dict[str, Any]:
        info = self.get(name)
        path = info.path / SERVER_METADATA_FILE
        if path.is_symlink():
            raise LauncherError("Настройки сервера являются символьной ссылкой")
        values = {
            "core": "custom",
            "version": info.minecraft_version or "",
            "build": "",
            "port": 25565,
            "max_players": 20,
            "motd": info.display_name or info.name,
            "memory": "4GiB",
            "java_path": "",
            "security_profile": "strict",
            "security_permissions": ["network:game", "files:instance"],
            "eula_accepted": False,
            "auto_restart": False,
            "restart_schedule": {"enabled": False, "time": "04:00", "warn_minutes": 1, "last_run": ""},
            "snapshot_retention_hours": 5,
            "snapshot_max_count": 1,
        }
        values.update(load_protected_settings(self.config_store, info.name, SERVER_SECURITY_DEFAULTS))
        rewrite = not path.exists()
        if path.exists():
            try:
                if path.stat().st_size > MAX_SERVER_CONFIG_JSON_BYTES:
                    raise LauncherError("Настройки сервера слишком большие")
                raw = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(raw, dict):
                    values.update({key: raw[key] for key in values if key not in SERVER_SECURITY_KEYS and key in raw})
                    rewrite = bool(SERVER_SECURITY_KEYS.intersection(raw))
            except (OSError, ValueError, TypeError, RecursionError) as exc:
                raise LauncherError(f"Не удалось прочитать настройки сервера: {exc}") from exc
        original_memory = str(values.get("memory", "4GiB")).strip()
        normalized = _normalize_server_config(values, clamp_memory=True)
        if normalized["memory"] != original_memory:
            rewrite = True
        save_protected_settings(self.config_store, info.name, {key: normalized[key] for key in SERVER_SECURITY_DEFAULTS})
        if rewrite:
            _write_server_config(path, normalized)
        return normalized

    def save_config(self, name: str, values: dict[str, Any]) -> dict[str, Any]:
        info = self.get(name)
        current = self.config(name)
        current.update({key: value for key, value in values.items() if key in current})
        normalized = _normalize_server_config(current)
        _write_eula_file(info.path, normalized["eula_accepted"])
        _write_server_config(info.path / SERVER_METADATA_FILE, normalized)
        save_protected_settings(self.config_store, info.name, {key: normalized[key] for key in SERVER_SECURITY_DEFAULTS})
        profile_path = info.path / INSTANCE_METADATA_FILE
        profile = self.instances._read_profile(info.path)
        profile.update(
            {
                "kind": "server",
                "server_core": normalized["core"],
                "server_core_version": normalized["version"],
                "server_port": normalized["port"],
                "server_eula": normalized["eula_accepted"],
            }
        )
        _atomic_json_write(profile_path, profile, private=True)
        return normalized

    def create(
        self,
        name: str,
        *,
        display_name: str,
        core: str,
        version: str,
        port: int,
        memory: str,
        eula_accepted: bool,
        custom_jar_name: str = "",
        custom_jar_data: bytes | None = None,
        replace_existing: bool = False,
    ) -> Any:
        safe_name = _safe_server_name(name)
        target = self.instances.root / safe_name
        if target.exists() or target.is_symlink():
            if target.is_symlink():
                raise LauncherError("Каталог сервера является символьной ссылкой")
            if replace_existing:
                raise LauncherError(
                    "Нельзя безопасно заменить сервер во время создания: загрузка ядра выполняется отдельно. "
                    "Выберите другое имя или удалите сервер отдельно после резервного копирования его мира."
                )
            raise LauncherError(f"Экземпляр уже существует: {safe_name}")
        target.mkdir(mode=0o700, parents=True)
        for directory in ("plugins", "mods", "config", "world", "logs"):
            (target / directory).mkdir(mode=0o700)
        profile = {
            "display_name": display_name.strip()[:80] or safe_name,
            "description": "Серверный экземпляр",
            "group": "",
            "kind": "server",
            "played_seconds": 0,
            "server_core": core,
            "server_core_version": version,
            "server_port": port,
            "server_eula": eula_accepted,
        }
        _atomic_json_write(target / INSTANCE_METADATA_FILE, profile, private=True)
        server_config = _normalize_server_config(
            {
                "core": core,
                "version": version,
                "port": port,
                "memory": memory,
                "motd": display_name or safe_name,
                "eula_accepted": eula_accepted,
                "security_profile": "strict",
                "security_permissions": ["network:game", "files:instance"],
            },
            clamp_memory=True,
        )
        _write_eula_file(target, eula_accepted)
        _write_server_config(target / SERVER_METADATA_FILE, server_config)
        save_protected_settings(self.config_store, safe_name, {key: server_config[key] for key in SERVER_SECURITY_DEFAULTS})
        if custom_jar_data is not None:
            self.write_core(target, custom_jar_name or SERVER_JAR, custom_jar_data)
        return self.get(safe_name)

    def write_core(self, root: Path, file_name: str, content: bytes) -> Path:
        with self._core_lock(root):
            if _server_is_running(root):
                raise LauncherError("Остановите сервер перед заменой ядра")
            if len(content) > 512 * 1024 * 1024:
                raise LauncherError("Файл ядра слишком большой (максимум 512 MiB)")
            safe = Path(file_name.replace("\\", "/")).name
            if safe != file_name.replace("\\", "/") or not safe.lower().endswith(".jar"):
                raise LauncherError("Ядро должно быть JAR-файлом без пути")
            if root.is_symlink() or not _within(root.resolve(), self.instances.root.resolve()):
                raise LauncherError("Каталог ядра находится вне экземпляров")
            destination = root / (SERVER_JAR if safe != SERVER_JAR else safe)
            if destination.is_symlink():
                raise LauncherError("Файл ядра является символьной ссылкой")
            temporary = root / f".{destination.name}.{uuid.uuid4().hex}.part"
            temporary.write_bytes(content)
            temporary.replace(destination)
            return destination

    @contextmanager
    def _core_lock(self, root: Path):
        lock = InstanceProcessLock(self.config_store.paths.data_home, root.name)
        lock.acquire()
        try:
            yield
        finally:
            lock.release()

    def public(self, name: str) -> dict[str, Any]:
        info = self.get(name)
        return _server_public(info, self.config(name))

class ServerFiles:
    def __init__(self, store: ServerInstanceStore) -> None:
        self.store = store

    @contextmanager
    def _mutation_lock(self, name: str):
        config_store = getattr(self.store, "config_store", None)
        if config_store is None:
            yield
            return
        info = self.store.get(name)
        lock = InstanceProcessLock(config_store.paths.data_home, info.name)
        lock.acquire()
        try:
            yield
        finally:
            lock.release()

    def list(self, name: str, directory: str = "") -> list[dict[str, Any]]:
        root = self.store.get(name).path
        parts = self._parts(directory, allow_root=True)
        path = root.joinpath(*parts)
        values: list[dict[str, Any]] = []
        if os.name == "nt":
            path = safe_path(root, parts)
            if not path.is_dir():
                raise LauncherError("Папка сервера не найдена")
            for item in path.iterdir():
                if item.name.startswith(".trial-launcher") or is_reparse_point(item):
                    continue
                try:
                    item_stat = item.stat()
                except OSError:
                    continue
                is_file = stat.S_ISREG(item_stat.st_mode)
                is_directory = stat.S_ISDIR(item_stat.st_mode)
                if not is_file and not is_directory:
                    continue
                relative = Path(*parts, item.name)
                values.append(
                    {
                        "name": item.name,
                        "path": str(relative),
                        "kind": "directory" if is_directory else "file",
                        "size": item_stat.st_size if is_file else _directory_size_nofollow(item),
                        "modified_at": item_stat.st_mtime,
                        "editable": is_file and item_stat.st_size <= 2 * 1024 * 1024,
                    }
                )
            return values
        try:
            with self._open_directory(root, parts, create=True) as directory_fd:
                entries = list(os.scandir(directory_fd))
                entries.sort(key=lambda value: (not value.is_dir(follow_symlinks=False), value.name.lower()))
                for item in entries[:1000]:
                    if item.name in {".trial-launcher-instance.json", ".trial-launcher-server.json", ".trial-launcher-live.json", ".trial-launcher-metrics.json"}:
                        continue
                    try:
                        item_stat = item.stat(follow_symlinks=False)
                    except OSError:
                        continue
                    if stat.S_ISLNK(item_stat.st_mode) or not (stat.S_ISDIR(item_stat.st_mode) or stat.S_ISREG(item_stat.st_mode)):
                        continue
                    relative = Path(*parts, item.name)
                    is_file = stat.S_ISREG(item_stat.st_mode)
                    values.append({"name": item.name, "path": str(relative), "kind": "directory" if not is_file else "file", "size": item_stat.st_size if is_file else _directory_size_nofollow(path / item.name), "modified_at": item_stat.st_mtime, "editable": is_file and item_stat.st_size <= 2 * 1024 * 1024})
        except OSError as exc:
            raise LauncherError(f"Не удалось прочитать папку сервера: {exc}") from exc
        return values

    def read_text(self, name: str, path: str) -> str:
        root = self.store.get(name).path
        parts = self._parts(path)
        if parts[-1].startswith(".trial-launcher"):
            raise LauncherError("Служебные файлы лаунчера нельзя читать через редактор")
        if os.name == "nt":
            target = safe_path(root, parts)
            regular_file(target, maximum_size=2 * 1024 * 1024)
            try:
                with target.open("rb") as handle:
                    content = handle.read(2 * 1024 * 1024 + 1)
            except OSError as exc:
                raise LauncherError("Текстовый файл не найден или недоступен") from exc
            if len(content) > 2 * 1024 * 1024:
                raise LauncherError("Текстовый файл не найден или слишком большой")
            try:
                return content.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise LauncherError("Файл не является UTF-8 текстом") from exc
        try:
            with self._open_parent(root, parts) as (parent_fd, file_name):
                with os.fdopen(os.open(file_name, os.O_RDONLY | _NOFOLLOW, dir_fd=parent_fd), "rb") as handle:
                    file_stat = os.fstat(handle.fileno())
                    if not stat.S_ISREG(file_stat.st_mode) or file_stat.st_size > 2 * 1024 * 1024:
                        raise LauncherError("Текстовый файл не найден или слишком большой")
                    content = handle.read(2 * 1024 * 1024 + 1)
                    if len(content) > 2 * 1024 * 1024:
                        raise LauncherError("Текстовый файл не найден или слишком большой")
            return content.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise LauncherError("Файл не является UTF-8 текстом") from exc
        except OSError as exc:
            raise LauncherError("Текстовый файл не найден или недоступен") from exc

    def write_text(self, name: str, path: str, content: str) -> None:
        with self._mutation_lock(name):
            root = self.store.get(name).path
            encoded = content.encode("utf-8")
            if len(encoded) > 2 * 1024 * 1024:
                raise LauncherError("Текстовый файл слишком большой")
            parts = self._parts(path)
            if parts[-1].startswith(".trial-launcher"):
                raise LauncherError("Служебные файлы лаунчера нельзя изменять")
            try:
                if os.name == "nt":
                    parent = safe_path(root, parts[:-1])
                    if not parent.is_dir():
                        raise LauncherError("Родительская папка не найдена")
                    atomic_write_path(root, parts, encoded)
                else:
                    with self._open_parent(root, parts, create=True) as (parent_fd, file_name):
                        fd = os.open(file_name, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | _NOFOLLOW, 0o600, dir_fd=parent_fd)
                        with os.fdopen(fd, "wb") as handle:
                            handle.write(encoded)
            except OSError as exc:
                raise LauncherError("Не удалось сохранить файл сервера") from exc

    def mkdir(self, name: str, path: str) -> None:
        with self._mutation_lock(name):
            root = self.store.get(name).path
            parts = self._parts(path)
            if parts[-1].startswith(".trial-launcher"):
                raise LauncherError("Служебные каталоги лаунчера нельзя создавать")
            try:
                if os.name == "nt":
                    safe_path(root, parts).mkdir()
                else:
                    with self._open_parent(root, parts, create=True) as (parent_fd, directory_name):
                        os.mkdir(directory_name, mode=0o700, dir_fd=parent_fd)
            except OSError as exc:
                raise LauncherError("Не удалось создать папку сервера") from exc

    def upload(self, name: str, path: str, content: bytes) -> None:
        with self._mutation_lock(name):
            root = self.store.get(name).path
            if len(content) > 256 * 1024 * 1024:
                raise LauncherError("Файл сервера слишком большой (максимум 256 MiB)")
            parts = self._parts(path)
            if parts[-1].startswith(".trial-launcher"):
                raise LauncherError("Служебные файлы лаунчера нельзя заменять")
            temporary_name = f".{parts[-1]}.{uuid.uuid4().hex}.part"
            try:
                if os.name == "nt":
                    parent = safe_path(root, parts[:-1])
                    if not parent.is_dir():
                        raise LauncherError("Родительская папка не найдена")
                    atomic_write_path(root, parts, content)
                else:
                    with self._open_parent(root, parts, create=True) as (parent_fd, file_name):
                        try:
                            existing = os.stat(file_name, dir_fd=parent_fd, follow_symlinks=False)
                        except FileNotFoundError:
                            existing = None
                        if existing is not None and stat.S_ISLNK(existing.st_mode):
                            raise LauncherError("Файл является символьной ссылкой")
                        fd = os.open(temporary_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW, 0o600, dir_fd=parent_fd)
                        try:
                            with os.fdopen(fd, "wb") as handle:
                                handle.write(content)
                            os.replace(temporary_name, file_name, src_dir_fd=parent_fd, dst_dir_fd=parent_fd)
                        finally:
                            try:
                                os.unlink(temporary_name, dir_fd=parent_fd)
                            except FileNotFoundError:
                                pass
            except OSError as exc:
                raise LauncherError("Не удалось загрузить файл сервера") from exc

    def delete(self, name: str, path: str) -> None:
        with self._mutation_lock(name):
            root = self.store.get(name).path
            parts = self._parts(path)
            if not parts or parts[-1].startswith(".trial-launcher"):
                raise LauncherError("Служебный путь нельзя удалить")
            try:
                if os.name == "nt":
                    remove_tree_path(root, parts, reserved=lambda value: value.startswith(".trial-launcher"))
                else:
                    with self._open_parent(root, parts) as (parent_fd, file_name):
                        _remove_entry_at(parent_fd, file_name)
            except FileNotFoundError:
                return
            except OSError as exc:
                raise LauncherError("Не удалось удалить путь сервера") from exc

    @staticmethod
    def _parts(raw: str, *, allow_root: bool = False) -> tuple[str, ...]:
        value = str(raw or "").replace("\\", "/")
        if "\x00" in value:
            raise LauncherError("Путь содержит недопустимый символ")
        if not value and allow_root:
            return ()
        path = Path(value)
        parts = tuple(path.parts)
        if path.is_absolute() or not parts or any(part in {"", ".", ".."} for part in parts):
            raise LauncherError("Путь выходит за пределы сервера")
        return parts

    @classmethod
    @contextmanager
    def _open_directory(cls, root: Path, parts: tuple[str, ...], *, create: bool = False):
        root_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
        directory_fd = -1
        try:
            directory_fd = cls._open_directory_chain(root_fd, parts, create=create)
            yield directory_fd
        finally:
            if directory_fd >= 0:
                os.close(directory_fd)
            os.close(root_fd)

    @classmethod
    @contextmanager
    def _open_parent(cls, root: Path, parts: tuple[str, ...], *, create: bool = False):
        if not parts:
            raise LauncherError("Укажите файл или папку")
        root_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
        parent_fd = -1
        try:
            parent_fd = cls._open_directory_chain(root_fd, parts[:-1], create=create)
            yield parent_fd, parts[-1]
        finally:
            if parent_fd >= 0:
                os.close(parent_fd)
            os.close(root_fd)

    @staticmethod
    def _open_directory_chain(root_fd: int, parts: tuple[str, ...], *, create: bool) -> int:
        current_fd = os.dup(root_fd)
        try:
            for part in parts:
                try:
                    next_fd = os.open(part, _DIRECTORY_OPEN_FLAGS, dir_fd=current_fd)
                except FileNotFoundError:
                    if not create:
                        raise
                    try:
                        os.mkdir(part, mode=0o700, dir_fd=current_fd)
                    except FileExistsError:
                        pass
                    next_fd = os.open(part, _DIRECTORY_OPEN_FLAGS, dir_fd=current_fd)
                os.close(current_fd)
                current_fd = next_fd
            return current_fd
        except BaseException:
            os.close(current_fd)
            raise

    @staticmethod
    def _path(root: Path, raw: str, *, allow_root: bool = False) -> Path:
        parts = ServerFiles._parts(raw, allow_root=allow_root)
        return root.joinpath(*parts)

@dataclass
class _ServerProcess:
    name: str
    root: Path
    process: subprocess.Popen[str]
    started_at: float
    logs: deque[tuple[int, str]]
    tracker: ProcessMetricsTracker
    process_lock: InstanceProcessLock
    log_sequence: int = 0
    players: int = 0
    max_players: int = 0
    tps: float = 0.0
    exit_code: int | None = None
    stop_requested: bool = False
    finished: threading.Event = field(default_factory=threading.Event)
    traffic: ServerTrafficTracker | None = None

_PROCESSES: dict[str, _ServerProcess] = {}
_PROCESS_LOCK = threading.RLock()
_LAUNCH_ERRORS: dict[str, str] = {}
_CONSOLE_FILE = ".trial-launcher-server-console.json"

def _saved_console(root: Path) -> dict[str, Any]:
    path = root / _CONSOLE_FILE
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 2 * 1024 * 1024:
            return {}
        value = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(value, dict) and isinstance(value.get("lines"), list):
            value["cursor"] = -max(1, path.stat().st_mtime_ns // 1000)
            return value
        return {}
    except (OSError, ValueError, RecursionError):
        return {}

def _save_console(root: Path, lines: list[str], error: str = "") -> None:
    selected: list[str] = []
    remaining = 256 * 1024
    for line in reversed(lines[-800:]):
        text = str(line)[-2048:]
        size = len(text.encode("utf-8")) + 16
        if size > remaining:
            break
        remaining -= size
        selected.append(text)
    _atomic_json_write(root / _CONSOLE_FILE, {"lines": list(reversed(selected)), "error": error}, private=True)

def _server_java_requirement(version: str) -> tuple[int, str]:
    match = re.fullmatch(r"1\.(\d+)(?:\.(\d+))?", version.strip())
    if not match:
        return 0, ""
    minor, patch = int(match[1]), int(match[2] or 0)
    if minor > 20 or minor == 20 and patch >= 5:
        return 21, "java-runtime-delta"
    if minor >= 18:
        return 17, "java-runtime-gamma"
    if minor == 17:
        return 16, "java-runtime-alpha"
    return 8, "jre-legacy"

def _initial_server_heap(memory: str) -> str:
    match = re.fullmatch(r"(\d+)([kKmMgG])", memory)
    if not match:
        return memory
    amount = int(match[1]) * {"k": 1024, "m": 1024 ** 2, "g": 1024 ** 3}[match[2].lower()]
    return "512M" if amount >= 512 * 1024 ** 2 else memory

class ServerProcessManager:
    def __init__(self, config: ConfigStore, store: ServerInstanceStore) -> None:
        self.config = config
        self.store = store

    def start(self, name: str, *, emit: Callable[[dict[str, Any]], None] | None = None) -> dict[str, Any]:
        info = self.store.get(name)
        try:
            result = self._start(name, emit=emit)
            _LAUNCH_ERRORS.pop(name, None)
            return result
        except LauncherError as exc:
            _LAUNCH_ERRORS[name] = str(exc)
            with _PROCESS_LOCK:
                previous = _PROCESSES.get(name)
                if previous and previous.process.poll() is not None:
                    previous.log_sequence += 1
                    previous.logs.append((previous.log_sequence, f"[launcher] Ошибка запуска: {exc}"))
            try:
                _save_console(info.path, [*_saved_console(info.path).get("lines", []), f"[launcher] Ошибка запуска: {exc}"], str(exc))
            except (OSError, LauncherError):
                pass
            raise

    def _start(self, name: str, *, emit: Callable[[dict[str, Any]], None] | None = None) -> dict[str, Any]:
        info = self.store.get(name)
        with _PROCESS_LOCK:
            current = _PROCESSES.get(name)
            if current and current.process.poll() is None:
                return self.status(name)
            if _server_is_running(info.path):
                raise LauncherError("Сервер уже запущен другим процессом")
        settings = self.store.config(name)
        if not settings["eula_accepted"]:
            raise LauncherError("Сначала примите EULA сервера")
        jar = info.path / SERVER_JAR
        if jar.is_symlink() or not jar.is_file():
            raise LauncherError("Ядро не найдено. Загрузите server.jar или выберите официальное ядро.")
        configured_java = str(settings.get("java_path", "")).strip() or self.config.settings.java_path or "java"
        required, component = _server_java_requirement(str(settings.get("version", "")))
        if configured_java == "java":
            system = shutil.which("java")
            candidate = validate_java_path(system) if system else None
            actual = java_version(candidate) if candidate else None
            if candidate and (not required or actual and actual >= required):
                java = str(candidate)
            else:
                java = str(JavaRuntimeManager(self.config).java_for({"javaVersion": {"majorVersion": required or None, "component": component}}))
        else:
            candidate = validate_java_path(configured_java, allow_directory=True)
            actual = java_version(candidate)
            if required and actual and actual < required:
                raise LauncherError(f"Java {actual} слишком старая для Minecraft {settings['version']}; требуется Java {required}. Выберите подходящую Java в настройках сервера.")
            java = str(candidate)
        memory = java_memory_value(str(settings.get("memory", "4GiB")))
        environment = safe_process_environment(compatibility=False)
        command = [java, f"-Xms{_initial_server_heap(memory)}", f"-Xmx{memory}", "-jar", SERVER_JAR]
        if settings["core"] not in {"velocity", "waterfall"}:
            command.append("--nogui")
        with _PROCESS_LOCK:
            if _server_is_running(info.path):
                raise LauncherError("Сервер уже запущен другим процессом")
            prepared_command, cwd, prepared_environment, warning = prepare_process(command, info.path, environment, settings)
            process_lock = InstanceProcessLock(self.config.paths.data_home, info.name)
            process_lock.acquire()
            try:
                _apply_server_port(info.path, int(settings["port"]))
                process = subprocess.Popen(
                    prepared_command,
                    cwd=cwd,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    bufsize=1,
                    env=prepared_environment,
                    **process_start_options(),
                )
            except OSError as exc:
                process_lock.release()
                raise LauncherError(f"Не удалось запустить сервер: {exc}") from exc
            except BaseException:
                process_lock.release()
                raise
            started = time.time()
            state = _ServerProcess(name, info.path, process, started, deque(maxlen=1000), ProcessMetricsTracker(info.path, int(process.pid), started_at=started), process_lock, max_players=int(settings["max_players"]))
            if current:
                state.log_sequence = current.log_sequence + 1
            state.traffic = ServerTrafficTracker(int(process.pid), int(settings["port"]))
            _PROCESSES[name] = state
        self._write_live(info.path, True, process.pid, started)
        if warning and emit:
            emit({"kind": "status", "event": "server-security-warning", "instance_name": name, "message": warning})
        threading.Thread(target=self._read_output, args=(state, emit), daemon=True).start()
        return self.status(name)

    def _read_output(self, state: _ServerProcess, emit: Callable[[dict[str, Any]], None] | None) -> None:
        process = state.process
        try:
            if process.stdout is None:
                raise LauncherError("Сервер не открыл поток вывода")
            for raw in process.stdout:
                line = raw.rstrip("\r\n")[-2048:]
                state.log_sequence += 1
                state.logs.append((state.log_sequence, line))
                self._parse_line(state, line)
                if emit:
                    emit({"kind": "log", "instance_name": state.name, "line": line})
        finally:
            state.exit_code = process.wait()
            should_restart = False
            if not state.stop_requested:
                try:
                    should_restart = bool(self.store.config(state.name).get("auto_restart", False))
                except (LauncherError, OSError):
                    pass
            try:
                snapshot = state.tracker.finish(exit_code=state.exit_code)
                if state.traffic:
                    snapshot.update(state.traffic.sample(running=False))
                    from .metrics import write_last_snapshot
                    write_last_snapshot(state.root, snapshot)
                with _PROCESS_LOCK:
                    is_current = _PROCESSES.get(state.name) is state
                    # keep stopped console output; discard only oldest stopped entries.
                    stopped = [key for key, item in _PROCESSES.items() if item.process.poll() is not None and item is not state]
                    for key in stopped[:-31]:
                        _PROCESSES.pop(key, None)
                if is_current:
                    self._write_live(state.root, False, 0, 0)
                    _save_console(state.root, [line for _, line in state.logs])
            except (OSError, LauncherError) as exc:
                if emit:
                    emit({"kind": "status", "event": "server-log-save-failed", "instance_name": state.name, "message": str(exc)})
            finally:
                state.process_lock.release()
                state.finished.set()
            if emit:
                emit({"kind": "status", "event": "server-stopped", "instance_name": state.name, "message": f"Сервер остановлен (код {state.exit_code})."})
            if not state.stop_requested:
                if should_restart:
                    if emit:
                        emit({"kind": "status", "event": "server-restarting", "instance_name": state.name, "message": "Сервер завершился неожиданно, повторный запуск через секунду."})
                    time.sleep(1)
                    with _PROCESS_LOCK:
                        if state.stop_requested or _PROCESSES.get(state.name) is not state:
                            return
                    try:
                        self.start(state.name, emit=emit)
                    except LauncherError as exc:
                        if emit:
                            emit({"kind": "status", "event": "server-restart-failed", "instance_name": state.name, "message": str(exc)})

    def stop(self, name: str) -> dict[str, Any]:
        with _PROCESS_LOCK:
            state = _PROCESSES.get(name)
        if not state or state.process.poll() is not None:
            return self.status(name)
        state.stop_requested = True
        if state.process.stdin is not None:
            try:
                state.process.stdin.write("stop\n")
                state.process.stdin.flush()
                state.process.wait(timeout=8)
                if not state.finished.wait(10):
                    raise LauncherError("Сервер завершился, но очистка запуска ещё не закончена; повторите действие")
                return self.status(name)
            except (BrokenPipeError, OSError, subprocess.TimeoutExpired):
                pass
        terminate_process_tree(state.process)
        try:
            state.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            terminate_process_tree(state.process, force=True)
            try:
                state.process.wait(timeout=5)
            except subprocess.TimeoutExpired as exc:
                raise LauncherError("Сервер не завершился после принудительной остановки") from exc
        if not state.finished.wait(10):
            raise LauncherError("Очистка предыдущего запуска ещё не завершена")
        return self.status(name)

    def restart(self, name: str, *, emit: Callable[[dict[str, Any]], None] | None = None) -> dict[str, Any]:
        self.stop(name)
        with _PROCESS_LOCK:
            previous = _PROCESSES.get(name)
        if previous and not previous.finished.wait(10):
            raise LauncherError("Предыдущий запуск ещё завершает очистку")
        return self.start(name, emit=emit)

    def command(self, name: str, command: str) -> dict[str, Any]:
        value = command.strip()
        if not value or len(value) > 2000:
            raise LauncherError("Команда сервера пуста или слишком длинная")
        with _PROCESS_LOCK:
            state = _PROCESSES.get(name)
        if not state or state.process.poll() is not None or state.process.stdin is None:
            raise LauncherError("Сервер не запущен")
        state.process.stdin.write(value + "\n")
        state.process.stdin.flush()
        return self.status(name)

    def log(self, name: str, since: int = 0) -> dict[str, Any]:
        with _PROCESS_LOCK:
            state = _PROCESSES.get(name)
        if not state:
            saved = _saved_console(self.store.get(name).path)
            cursor = int(saved.get("cursor", -1))
            return {"lines": saved.get("lines", []) if since != cursor else [], "running": False, "cursor": cursor,
                    "reset": bool(since and since != cursor), "error": saved.get("error", "")}
        cursor = state.log_sequence
        entries = list(state.logs)
        if since and entries and entries[0][0] - 1 <= since <= cursor:
            return {
                "lines": [line for sequence, line in entries if sequence > since],
                "running": state.process.poll() is None,
                "cursor": cursor,
                "reset": False,
            }
        return {
            "lines": [line for _, line in entries],
            "running": state.process.poll() is None,
            "cursor": cursor,
            "reset": bool(since),
        }

    def status(self, name: str) -> dict[str, Any]:
        with _PROCESS_LOCK:
            state = _PROCESSES.get(name)
        info = self.store.get(name)
        settings = self.store.config(name)
        if state and state.process.poll() is None:
            metrics = state.tracker.sample(players=state.players, max_players=state.max_players, tps=state.tps)
            if state.traffic:
                metrics.update(state.traffic.sample())
            running = True
            pid = state.process.pid
            started_at = state.started_at
        else:
            previous = read_metrics(info.path).get("last", {})
            metrics = {"timestamp": time.time(), "pid": 0, "running": False, "uptime_seconds": 0, "ram_bytes": 0, "cpu_percent": 0.0, "storage_bytes": directory_size(info.path), "players": 0, "max_players": int(settings["max_players"]), "tps": 0.0, "last": previous}
            running = False
            pid = 0
            started_at = 0
            if state and state.traffic:
                metrics.update(state.traffic.sample(running=False))
        saved_error = _LAUNCH_ERRORS.get(name, "") or _saved_console(info.path).get("error", "")
        result = {"running": running, "pid": pid, "started_at": started_at, "start_error": saved_error if not running else "", "exit_code": state.exit_code if state else metrics.get("last", {}).get("exit_code"), "players": metrics.get("players", 0), "max_players": metrics.get("max_players", settings["max_players"]), "tps": metrics.get("tps", 0), "uptime_seconds": metrics.get("uptime_seconds", 0), "metrics": metrics, "last_metrics": read_metrics(info.path).get("last", {})}
        if running:
            self._maybe_scheduled_restart(name, state, settings.get("restart_schedule") or {})
        else:
            exit_code = result.get("exit_code")
            if isinstance(exit_code, int) and exit_code != 0:
                from .crash_reports import analyze_crash

                lines = [line for _, line in state.logs] if state else _saved_console(info.path).get("lines", [])
                result["crash"] = analyze_crash(lines, exit_code, instance_path=info.path, memory=str(settings.get("memory", "")))
        return result

    def _maybe_scheduled_restart(self, name: str, state: "_ServerProcess", schedule: dict[str, Any]) -> None:
        if not schedule.get("enabled") or getattr(state, "scheduled_restart_date", ""):
            return
        match = re.fullmatch(r"(\d{1,2}):(\d{2})", str(schedule.get("time", "04:00")))
        if not match:
            return
        now = time.localtime()
        hour, minute = int(match.group(1)), int(match.group(2))
        if (now.tm_hour, now.tm_min) < (hour, minute):
            return
        today = time.strftime("%Y-%m-%d", now)
        if str(schedule.get("last_run", "")) == today:
            return
        # старт после расчётного времени — это уже перезапуск, повторно не трогаем
        if state.started_at >= time.mktime((now.tm_year, now.tm_mon, now.tm_mday, hour, minute, 0, now.tm_isdst, -1)):
            return
        state.scheduled_restart_date = today
        schedule["last_run"] = today
        try:
            config_values = self.store.config(name)
            config_values["restart_schedule"] = schedule
            self.store.save_config(name, config_values)
        except (LauncherError, OSError):
            pass
        threading.Thread(
            target=self._scheduled_restart_worker,
            args=(name, int(schedule.get("warn_minutes", 1))),
            daemon=True,
        ).start()

    def _scheduled_restart_worker(self, name: str, warn_minutes: int) -> None:
        # сообщение в чат — только ASCII: кодировка консоли Windows не гарантирована
        try:
            self.command(name, f"say Scheduled restart in {max(1, warn_minutes)} min")
        except LauncherError:
            pass
        time.sleep(max(1, min(30, warn_minutes)) * 60)
        with _PROCESS_LOCK:
            state = _PROCESSES.get(name)
        if not state or state.process.poll() is not None:
            return
        try:
            self.restart(name)
        except LauncherError:
            pass

    @staticmethod
    def _parse_line(state: _ServerProcess, line: str) -> None:
        match = re.search(r"There are (\d+) of a max of (\d+) players online", line)
        if match:
            state.players, state.max_players = int(match.group(1)), int(match.group(2))
        tps = re.search(r"(?:TPS|tps)[:= ]+([0-9]+(?:\.[0-9]+)?)", line)
        if tps:
            try:
                state.tps = float(tps.group(1))
            except ValueError:
                pass

    @staticmethod
    def _write_live(root: Path, running: bool, pid: int, started_at: float) -> None:
        _atomic_json_write(root / ".trial-launcher-live.json", {"running": running, "pid": pid, "started_at": started_at}, private=True)

def server_memory_limit_kib(total_kib: int | None = None) -> int:
    total = system_memory_kib() if total_kib is None else int(total_kib)
    if total <= 0:
        return _UNKNOWN_RAM_SERVER_LIMIT_KIB
    return max(1, total * 3 // 4)

def _normalize_server_config(value: dict[str, Any], *, clamp_memory: bool = False) -> dict[str, Any]:
    core = str(value.get("core", "custom")).strip().lower()
    if core == "leaves":
        core = "leaf"
    valid_cores = {item["id"] for item in SERVER_CORES}
    if core not in valid_cores:
        core = "custom"
    try:
        port = min(65535, max(1, int(value.get("port", 25565))))
        max_players = min(100000, max(1, int(value.get("max_players", 20))))
    except (TypeError, ValueError) as exc:
        raise LauncherError("Порт и количество игроков должны быть числами") from exc
    raw_memory = str(value.get("memory", "")).strip()
    memory = raw_memory or "4GiB"
    memory_kib = parse_memory_kib(memory)
    memory_limit_kib = server_memory_limit_kib()
    # дефолтные 4GiB на машине с ≤5,3 ГБ ОЗУ не должны валить создание
    default_memory = memory_kib == 4 * 1024 * 1024
    if memory_kib > memory_limit_kib:
        if not clamp_memory and not default_memory:
            raise LauncherError(
                "Память JVM сервера не может превышать "
                f"{format_memory_kib(memory_limit_kib)} (75% физической памяти; остальное оставлено системе)"
            )
        memory = format_memory_kib(memory_limit_kib)
    else:
        memory = format_memory_kib(memory_kib)
    java_memory_value(memory)
    try:
        retention = min(720, max(1, int(value.get("snapshot_retention_hours", 5))))
        max_count = min(50, max(1, int(value.get("snapshot_max_count", 1))))
    except (TypeError, ValueError) as exc:
        raise LauncherError("Параметры хранения снапшотов должны быть числами") from exc
    security_profile = str(value.get("security_profile", "strict")).strip().lower()
    if security_profile not in {"manual", "strict", "off"}:
        raise LauncherError("Серверный security profile должен быть manual, strict или off")
    raw_permissions = value.get("security_permissions", ["network:game", "files:instance"])
    permissions = [
        permission
        for permission in (raw_permissions if isinstance(raw_permissions, list) else [])
        if str(permission).strip() in {"network:game", "files:instance"}
    ]
    if "network:game" not in permissions:
        permissions.append("network:game")
    if "files:instance" not in permissions:
        permissions.append("files:instance")
    return {
        "core": core,
        "version": str(value.get("version", "")).strip()[:32],
        "build": str(value.get("build", "")).strip()[:32],
        "port": port,
        "max_players": max_players,
        "motd": str(value.get("motd", "")).strip()[:256],
        "memory": memory,
        "java_path": str(value.get("java_path", "")).strip()[:512],
        "security_profile": security_profile,
        "security_permissions": permissions,
        "eula_accepted": bool(value.get("eula_accepted", False)),
        "auto_restart": bool(value.get("auto_restart", False)),
        "restart_schedule": _normalize_restart_schedule(value.get("restart_schedule")),
        "snapshot_retention_hours": retention,
        "snapshot_max_count": max_count,
    }

def _normalize_restart_schedule(value: Any) -> dict[str, Any]:
    raw = value if isinstance(value, dict) else {}
    time_value = str(raw.get("time", "04:00")).strip()
    if not re.fullmatch(r"([01]?\d|2[0-3]):[0-5]\d", time_value):
        time_value = "04:00"
    try:
        warn_minutes = min(30, max(0, int(raw.get("warn_minutes", 1))))
    except (TypeError, ValueError):
        warn_minutes = 1
    last_run = str(raw.get("last_run", ""))[:10]
    return {
        "enabled": bool(raw.get("enabled", False)),
        "time": time_value,
        "warn_minutes": warn_minutes,
        "last_run": last_run if re.fullmatch(r"\d{4}-\d{2}-\d{2}", last_run) else "",
    }

def _write_server_config(path: Path, values: dict[str, Any]) -> None:
    _atomic_json_write(path, {key: value for key, value in values.items() if key not in SERVER_SECURITY_KEYS}, private=True)

def _write_eula_file(root: Path, accepted: bool) -> None:
    temporary_name = f".eula.txt.{uuid.uuid4().hex}.part"
    content = f"# Generated by Nivora Launcher\neula={'true' if accepted else 'false'}\n".encode("utf-8")
    if os.name == "nt":
        target = safe_path(root, ("eula.txt",))
        if os.path.lexists(target):
            regular_file(target)
        atomic_write_path(root, ("eula.txt",), content)
        return
    directory_fd = -1
    try:
        directory_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
        try:
            existing = os.stat("eula.txt", dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            if not stat.S_ISREG(existing.st_mode):
                raise LauncherError("Файл EULA является символьной ссылкой или имеет неверный тип")
        fd = os.open(temporary_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW, 0o600, dir_fd=directory_fd)
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_name, "eula.txt", src_dir_fd=directory_fd, dst_dir_fd=directory_fd)
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError(f"Не удалось безопасно сохранить EULA: {exc}") from exc
    finally:
        if directory_fd >= 0:
            try:
                os.unlink(temporary_name, dir_fd=directory_fd)
            except FileNotFoundError:
                pass
            os.close(directory_fd)

def _apply_server_port(root: Path, port: int, *, public_bind: bool = False) -> None:
    if not 1 <= port <= 65535:
        raise LauncherError("Порт сервера должен быть от 1 до 65535")
    if os.name == "nt":
        target = safe_path(root, (SERVER_PROPERTIES_FILE,))
        if os.path.lexists(target):
            regular_file(target, maximum_size=MAX_SERVER_PROPERTIES_BYTES)
            try:
                content = target.read_bytes()
            except OSError as exc:
                raise LauncherError("Не удалось прочитать server.properties") from exc
        else:
            content = b""
        updated = _server_properties_with_port(content, port, public_bind=public_bind)
        if updated != content:
            atomic_write_path(root, (SERVER_PROPERTIES_FILE,), updated)
        return
    directory_fd = -1
    temporary_name = f".{SERVER_PROPERTIES_FILE}.{uuid.uuid4().hex}.part"
    try:
        directory_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
        try:
            file_stat = os.stat(SERVER_PROPERTIES_FILE, dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            content = b""
        else:
            if not stat.S_ISREG(file_stat.st_mode) or file_stat.st_size > MAX_SERVER_PROPERTIES_BYTES:
                raise LauncherError("Файл server.properties недоступен или слишком большой")
            fd = os.open(SERVER_PROPERTIES_FILE, os.O_RDONLY | _NOFOLLOW | _NONBLOCK, dir_fd=directory_fd)
            with os.fdopen(fd, "rb") as handle:
                opened_stat = os.fstat(handle.fileno())
                if not stat.S_ISREG(opened_stat.st_mode) or opened_stat.st_size > MAX_SERVER_PROPERTIES_BYTES:
                    raise LauncherError("Файл server.properties недоступен или слишком большой")
                content = handle.read(MAX_SERVER_PROPERTIES_BYTES + 1)
            if len(content) > MAX_SERVER_PROPERTIES_BYTES:
                raise LauncherError("Файл server.properties слишком большой")

        next_content = _server_properties_with_port(content, port, public_bind=public_bind)
        if next_content == content:
            return

        fd = os.open(
            temporary_name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW,
            0o600,
            dir_fd=directory_fd,
        )
        with os.fdopen(fd, "wb") as handle:
            handle.write(next_content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(
            temporary_name,
            SERVER_PROPERTIES_FILE,
            src_dir_fd=directory_fd,
            dst_dir_fd=directory_fd,
        )
        os.fsync(directory_fd)
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError(f"Не удалось применить порт сервера: {exc}") from exc
    finally:
        if directory_fd >= 0:
            try:
                os.unlink(temporary_name, dir_fd=directory_fd)
            except FileNotFoundError:
                pass
            os.close(directory_fd)

def _safe_server_name(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9._-]+", "-", str(value).strip()).strip(".-")
    if not cleaned:
        raise LauncherError("Имя сервера не может быть пустым")
    return cleaned[:64]

def _server_properties_with_port(content: bytes, port: int, *, public_bind: bool = False) -> bytes:
    text = content.decode("utf-8", errors="surrogateescape")
    newline = "\r\n" if "\r\n" in text else "\n"
    lines = text.splitlines(keepends=True)
    updated: list[str] = []
    found = False
    for line in lines:
        body = line.rstrip("\r\n")
        ending = line[len(body) :]
        match = re.match(r"^([ \t]*server-port[ \t]*(?:=|:|[ \t]+)[ \t]*).*$", body)
        if match:
            updated.append(f"{match.group(1)}{port}{ending}")
            found = True
        else:
            updated.append(line)
    if not found:
        if updated and not updated[-1].endswith(("\n", "\r")):
            updated[-1] += newline
        updated.append(f"server-port={port}{newline}")
    result = "".join(updated)
    if public_bind:
        # preserve all unrelated properties, including online-mode / whitelist.
        result, matches = re.subn(r"(?m)^[ \t]*server-ip[ \t]*(?:=|:|[ \t]+)[^\r\n]*", "server-ip=", result)
        if not matches:
            if result and not result.endswith(("\n", "\r")):
                result += newline
            result += f"server-ip={newline}"
    return result.encode("utf-8", errors="surrogateescape")

def _server_is_running(root: Path) -> bool:
    state = root / ".trial-launcher-live.json"
    try:
        if state.is_symlink() or not state.is_file():
            return False
        with os.fdopen(os.open(state, os.O_RDONLY | _NOFOLLOW), "r", encoding="utf-8") as handle:
            value = json.load(handle)
        if not isinstance(value, dict):
            return False
        pid = int(value.get("pid", 0)) if value.get("running") else 0
        if pid > 0:
            if not process_exists(pid):
                return False
            if os.name == "nt":
                try:
                    import psutil

                    command_line = " ".join(psutil.Process(pid).cmdline()).lower()
                    if "server.jar" not in command_line:
                        return False
                except ImportError:
                    pass
                except psutil.NoSuchProcess:
                    return False
                except psutil.Error:
                    return True
                return True
            proc = Path(f"/proc/{pid}")
            if proc.is_dir():
                try:
                    command_line = (proc / "cmdline").read_bytes().replace(b"\x00", b" ").decode(errors="ignore")
                    if "server.jar" not in command_line:
                        return False
                except OSError:
                    pass
            return True
    except (OSError, ValueError, TypeError, json.JSONDecodeError):
        pass
    return False

def _remove_entry_at(parent_fd: int, name: str) -> None:
    item_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISLNK(item_stat.st_mode):
        raise LauncherError("Символьные ссылки запрещены")
    if stat.S_ISDIR(item_stat.st_mode):
        directory_fd = os.open(name, _DIRECTORY_OPEN_FLAGS, dir_fd=parent_fd)
        try:
            for entry in os.scandir(directory_fd):
                _remove_entry_at(directory_fd, entry.name)
        finally:
            os.close(directory_fd)
        os.rmdir(name, dir_fd=parent_fd)
        return
    if not stat.S_ISREG(item_stat.st_mode):
        raise LauncherError("Можно удалить только файл или папку")
    os.unlink(name, dir_fd=parent_fd)

def _directory_size_nofollow(root: Path) -> int:
    total = 0
    for current, directories, files in os.walk(root, followlinks=False):
        directories[:] = [name for name in directories if not (Path(current) / name).is_symlink()]
        for name in files:
            path = Path(current) / name
            try:
                item_stat = path.stat()
            except OSError:
                continue
            if not path.is_symlink() and stat.S_ISREG(item_stat.st_mode):
                total += item_stat.st_size
    return total
