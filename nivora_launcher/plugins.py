from __future__ import annotations

import io
import hashlib
import hmac
import ipaddress
import json
import math
import os
import re
import shutil
import sqlite3
import stat
import tempfile
import threading
import time
import zipfile
from pathlib import Path
from typing import Any

from .config import ConfigStore, _atomic_json_write
from .errors import LauncherError
from .platform_support import set_private_fd
from .plugin_security import PluginTrustStore, verify_manifest

PLUGIN_MANIFEST = "plugin.yml"
PLUGIN_V2_MANIFEST = "plugin.json"
PLUGIN_MAX_MANIFEST_BYTES = 64 * 1024
PLUGIN_MAX_ARCHIVE_BYTES = 64 * 1024 * 1024
PLUGIN_MAX_UNPACKED_BYTES = 128 * 1024 * 1024
PLUGIN_MAX_FILES = 4096
PLUGIN_RUNTIME_MAX_SOURCE_BYTES = 4 * 1024 * 1024
PLUGIN_ID_RE = re.compile(r"^[a-z][a-z0-9._-]{1,63}$")
PLUGIN_DATABASE_MAX_BYTES = 64 * 1024 * 1024
_PLUGIN_PERMISSION_FILE = "plugin-permissions.json"
_PLUGIN_PERMISSION_SCHEMA = 1
_PLUGIN_V2_DATA_DIRECTORIES = {"storage", "files", "config", "database"}
_PLUGIN_ACCESS_LOCK = threading.RLock()
_PLUGIN_LEGACY_PERMISSION_NAMES = {
    "links:open",
    "network:fetch",
    "network:dns",
    "network:ping",
    "network:listen",
    "launcher:servers",
    "launcher:dialogs",
    "storage:workspace",
    "storage:database",
    "files:instance",
    "files:plugin",
    "ui:compose",
    "events:launcher",
}
PLUGIN_V2_PERMISSION_NAMES = _PLUGIN_LEGACY_PERMISSION_NAMES - {"network:listen"}
PLUGIN_V3_PERMISSION_NAMES = PLUGIN_V2_PERMISSION_NAMES | {
    "runtime:observe",
    "lifecycle:observe",
    "process:observe",
    "files:launcher-root-read",
    "files:launcher-config-projected",
    "network:http",
    "network:https",
    "network:websocket",
    "network:tcp-client",
    "network:udp-client",
    "network:proxy-control",
    "network:traffic-observe",
    "network:traffic-intercept",
    "network:tunnel",
    "os:open-url",
    "os:open-file",
    "os:reveal-in-file-manager",
    "os:clipboard-read",
    "os:clipboard-write",
    "os:spawn-external",
    "os:browser-session",
    "auth:oauth",
    "auth:token-use",
    "secrets:store",
    "secrets:use",
    "ai:invoke-cloud",
    "ai:invoke-local",
    "downloads:manage",
    "updates:manage",
    "dependencies:resolve",
    "integrity:scan",
    "metrics:emit",
    "logs:write",
    "presence:discord",
    "scheduler:jobs",
    "commands:register",
    "shortcuts:register",
    "instances:read",
    "instances:mutate",
    "mods:read",
    "mods:install",
    "packs:manage",
    "accounts:projected-read",
    "backup:export",
    "backup:restore",
    "sync:cloud",
    "ipc:sidecar",
    "policy:read",
    "crypto:sign",
    "crypto:verify",
}
PLUGIN_PERMISSION_NAMES = _PLUGIN_LEGACY_PERMISSION_NAMES | PLUGIN_V3_PERMISSION_NAMES

PLUGIN_RUNTIME_CAPABILITIES = {
    "runtime.capabilityNegotiation",
    "runtime.quotaTelemetry",
    "files.launcherRootReadonly",
    "config.projectedLauncherRead",
    "network.http",
    "network.proxyPolicies",
    "network.trafficPolicies",
    "ui.composition.v2",
    "ui.composition.extended",
    "ui.fragments",
    "ui.dataBindings",
    "runtime.operationLimits",
    "runtime.operationAudit",
    "events.listenerDebug",
    "lifecycle.processMetrics",
}

def discover_plugins(root: Path) -> list[dict[str, Any]]:
    trust_store = PluginTrustStore(root.parent / "plugin-trust")
    permission_state = _read_permission_state(root.parent / _PLUGIN_PERMISSION_FILE)
    return _discover_plugins(root, trust_store, permission_state)

def _discover_plugins(
    root: Path,
    trust_store: PluginTrustStore,
    permission_state: dict[str, Any],
) -> list[dict[str, Any]]:
    if root.is_symlink():
        raise LauncherError("Каталог плагинов является символьной ссылкой")
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    result: list[dict[str, Any]] = []
    for directory in sorted(root.iterdir(), key=lambda item: item.name.lower())[:256]:
        if directory.is_symlink() or not directory.is_dir():
            continue
        legacy_path = directory / PLUGIN_MANIFEST
        v2_path = directory / PLUGIN_V2_MANIFEST
        has_legacy = legacy_path.is_file() and not legacy_path.is_symlink()
        has_v2 = v2_path.is_file() and not v2_path.is_symlink()
        static_json = _static_json_manifest(directory) if has_v2 else None
        if static_json is not None and static_json["id"] == directory.name:
            result.append({**static_json})
            continue
        if has_v2 or v2_path.exists() or v2_path.is_symlink():
            result.append(_v2_plugin_status(directory, trust_store, permission_state))
            continue
        if not has_legacy:
            continue
        try:
            if legacy_path.stat().st_size > PLUGIN_MAX_MANIFEST_BYTES:
                continue
            manifest = validate_manifest(parse_plugin_yml(legacy_path.read_text(encoding="utf-8")))
        except (OSError, UnicodeError, ValueError, LauncherError):
            continue
        if manifest["id"] != directory.name:
            continue
        if has_legacy and (directory / PLUGIN_V2_MANIFEST).exists():
            continue
        result.append(
            {
                **manifest,
                "trust": "local",
                "installed": True,
                "enabled": _enabled_state(directory),
                "revoked": trust_store.is_plugin_revoked(manifest["id"]),
                "revocation_reason": trust_store.plugin_revocation_reason(manifest["id"]),
            }
        )
    return result

class PluginStore:
    def __init__(self, config: ConfigStore) -> None:
        self._config = config
        self.data_home = config.paths.data_home
        self.root = config.paths.data_home / "plugins"
        if self.root.is_symlink():
            raise LauncherError("Каталог плагинов является символьной ссылкой")
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.trust_store = PluginTrustStore(self.data_home / "plugin-trust")
        self.permission_path = self.data_home / _PLUGIN_PERMISSION_FILE
        _read_permission_state(self.permission_path)

    def list(self) -> list[dict[str, Any]]:
        with _PLUGIN_ACCESS_LOCK:
            state = _read_permission_state(self.permission_path)
        return _discover_plugins(self.root, self.trust_store, state)

    def install_archive(self, content: bytes, *, enable: bool = False, replace: bool = False) -> dict[str, Any]:
        if not content or len(content) > PLUGIN_MAX_ARCHIVE_BYTES:
            raise LauncherError("Архив плагина пустой или слишком большой")
        staging = Path(tempfile.mkdtemp(prefix=".install-", dir=self.root))
        try:
            with zipfile.ZipFile(io.BytesIO(content)) as archive:
                members = archive.infolist()
                if len(members) > PLUGIN_MAX_FILES:
                    raise LauncherError("Архив плагина содержит слишком много файлов")
                total_size = 0
                seen_members: set[str] = set()
                for member in members:
                    path = _archive_member_path(staging, member.filename)
                    normalized_name = "/".join(path.relative_to(staging).parts)
                    if normalized_name in seen_members:
                        raise LauncherError("Архив плагина содержит повторяющийся путь")
                    seen_members.add(normalized_name)
                    if _archive_member_is_symlink(member):
                        raise LauncherError("Архив плагина содержит символьную ссылку")
                    if member.is_dir():
                        path.mkdir(mode=0o700, parents=True, exist_ok=True)
                        continue
                    total_size += member.file_size
                    if member.file_size > PLUGIN_MAX_UNPACKED_BYTES or total_size > PLUGIN_MAX_UNPACKED_BYTES:
                        raise LauncherError("Распакованный плагин слишком большой")
                    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                    with archive.open(member, "r") as source, path.open("wb") as target:
                        shutil.copyfileobj(source, target, length=1024 * 1024)
                    path.chmod(0o600)
            legacy_path = staging / PLUGIN_MANIFEST
            v2_path = staging / PLUGIN_V2_MANIFEST
            if legacy_path.exists() == v2_path.exists():
                raise LauncherError("Архив должен содержать ровно один корневой plugin.yml или plugin.json")
            is_v2 = v2_path.exists()
            if is_v2:
                static_json = _static_json_manifest(staging)
                if static_json is not None:
                    manifest = static_json
                    entrypoint = _safe_child(staging, manifest["entrypoint"])
                    if not entrypoint.is_file():
                        raise LauncherError("entrypoint плагина отсутствует в архиве")
                else:
                    _reject_v2_runtime_paths(staging)
                    package = _read_v2_package(staging)
                    manifest = package["manifest"]
            else:
                if not legacy_path.is_file() or legacy_path.is_symlink() or legacy_path.stat().st_size > PLUGIN_MAX_MANIFEST_BYTES:
                    raise LauncherError("В архиве отсутствует корректный plugin.yml")
                manifest = validate_manifest(parse_plugin_yml(legacy_path.read_text(encoding="utf-8")))
                entrypoint = _safe_child(staging, manifest["entrypoint"])
                if not entrypoint.is_file():
                    raise LauncherError("entrypoint плагина отсутствует в архиве")
            plugin_id = manifest["id"]
            target = self.root / plugin_id
            if target.exists() or target.is_symlink():
                if not replace:
                    raise LauncherError(f"Плагин уже установлен: {plugin_id}")
                if target.is_symlink():
                    raise LauncherError("Каталог существующего плагина является символьной ссылкой")
                self._reconcile_replacement_permissions(plugin_id, target, package if is_v2 else None)
                for name in ("storage", "files", "config", "database", ".enabled"):
                    old = target / name
                    if old.exists() or old.is_symlink():
                        shutil.move(str(old), str(staging / name))
                shutil.rmtree(target)
            staging.rename(target)
            target.chmod(0o700)
            enabled_marker = target / ".enabled"
            if enable:
                if not is_v2 or self._can_execute(package, plugin_id):
                    enabled_marker.touch(mode=0o600, exist_ok=True)
                elif enabled_marker.exists() or enabled_marker.is_symlink():
                    enabled_marker.unlink()
            elif enabled_marker.exists():
                enabled_marker.unlink()
            self.namespace(plugin_id)
            return self._plugin_status(plugin_id)
        except (LauncherError, OSError, UnicodeError, ValueError, zipfile.BadZipFile) as exc:
            if isinstance(exc, LauncherError):
                raise
            raise LauncherError(f"Не удалось установить плагин: {exc}") from exc
        finally:
            if staging.exists():
                shutil.rmtree(staging, ignore_errors=True)

    def set_enabled(self, plugin_id: str, enabled: bool) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        marker = directory / ".enabled"
        if enabled:
            if self.trust_store.is_plugin_revoked(plugin_id):
                raise LauncherError("Плагин отозван локально или списком отзыва")
            if (directory / PLUGIN_V2_MANIFEST).exists():
                package = _read_v2_package(directory, expected_id=plugin_id)
                self._require_executable(package, plugin_id, require_enabled=False)
            marker.touch(mode=0o600, exist_ok=True)
        elif marker.exists() or marker.is_symlink():
            if marker.is_symlink():
                raise LauncherError("Маркер состояния плагина является символьной ссылкой")
            marker.unlink()
        return self._plugin_status(plugin_id)

    def is_enabled(self, plugin_id: str) -> bool:
        return _enabled_state(self._existing_plugin_directory(plugin_id))

    def uninstall(self, plugin_id: str) -> None:
        directory = self._existing_plugin_directory(plugin_id)
        shutil.rmtree(directory)
        self._update_permission_record(plugin_id, None)

    def namespace(self, plugin_id: str) -> Path:
        directory = self._plugin_directory(plugin_id)
        for name in ("storage", "files", "config", "database"):
            path = directory / name
            if path.is_symlink():
                raise LauncherError(f"Каталог плагина является символьной ссылкой: {name}")
            path.mkdir(mode=0o700, exist_ok=True)
        return directory

    def manifest(self, plugin_id: str) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        if (directory / PLUGIN_V2_MANIFEST).exists():
            static_json = _static_json_manifest(directory)
            if static_json is not None:
                if static_json["id"] != plugin_id:
                    raise LauncherError("id plugin.json не совпадает с каталогом")
                return static_json
            return _read_v2_package(directory, expected_id=plugin_id)["manifest"]
        if (directory / PLUGIN_V2_MANIFEST).is_symlink():
            raise LauncherError("plugin.json является символьной ссылкой")
        path = directory / PLUGIN_MANIFEST
        if path.is_symlink() or not path.is_file() or path.stat().st_size > PLUGIN_MAX_MANIFEST_BYTES:
            raise LauncherError("plugin.yml не найден")
        value = validate_manifest(parse_plugin_yml(path.read_text(encoding="utf-8")))
        if value["id"] != plugin_id:
            raise LauncherError("id plugin.yml не совпадает с каталогом")
        if (directory / PLUGIN_V2_MANIFEST).exists():
            raise LauncherError("Каталог содержит конфликтующие plugin.yml и plugin.json")
        entrypoint = _safe_child(directory, value["entrypoint"])
        if not entrypoint.is_file():
            raise LauncherError("entrypoint плагина отсутствует")
        return value

    def authorize_operation(self, plugin_id: str, permission: str) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        if (directory / PLUGIN_V2_MANIFEST).exists():
            package = _read_v2_package(directory, expected_id=plugin_id)
            self._require_executable(package, plugin_id)
            manifest = package["manifest"]
            if permission not in manifest["permissions"]:
                raise LauncherError(f"Плагину не запрошено разрешение {permission}")
            granted = self._granted_permissions(plugin_id, manifest["permissions"])
            if permission not in granted:
                raise LauncherError(f"Пользователь не выдал плагину разрешение {permission}")
            return manifest

        manifest = self.manifest(plugin_id)
        if not self.is_enabled(plugin_id):
            raise LauncherError("Плагин выключен")
        if self.trust_store.is_plugin_revoked(plugin_id):
            raise LauncherError("Плагин отозван локально или списком отзыва")
        if permission not in manifest.get("permissions", []):
            raise LauncherError(f"Плагину не выдано разрешение {permission}")
        return manifest

    def set_permission_grants(self, plugin_id: str, permissions: Any) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        if not (directory / PLUGIN_V2_MANIFEST).exists():
            raise LauncherError("Постоянные grants поддерживаются только для plugin.json")
        manifest = _read_v2_package(directory, expected_id=plugin_id)["manifest"]
        if type(permissions) is not list or any(type(item) is not str for item in permissions):
            raise LauncherError("Список grants должен содержать строки")
        requested = set(manifest["permissions"])
        granted = set(permissions)
        if len(granted) != len(permissions) or not granted.issubset(requested):
            raise LauncherError("Можно выдать только уникальные permissions из plugin.json")
        self._update_permission_record(
            plugin_id,
            {"grants": sorted(granted), "untrusted_execution_hash": self._untrusted_execution_hash(plugin_id)},
        )
        return self._plugin_status(plugin_id)

    def set_local_untrusted_execution(self, plugin_id: str, allowed: bool) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        if type(allowed) is not bool:
            raise LauncherError("allowed должен быть boolean")
        if not (directory / PLUGIN_V2_MANIFEST).exists():
            raise LauncherError("Локальный запуск поддерживается только для plugin.json")
        package = _read_v2_package(directory, expected_id=plugin_id)
        if package["signed"]:
            raise LauncherError("Локальный untrusted-запуск доступен только для unsigned plugin.json")
        self._update_permission_record(
            plugin_id,
            {
                "grants": self._stored_grants(plugin_id),
                "untrusted_execution_hash": package["package_digest"] if allowed else "",
            },
        )
        return self._plugin_status(plugin_id)

    def pin_publisher(self, key_id: str, public_key: str, *, revocation_authority: bool = False) -> dict[str, Any]:
        self.trust_store.pin_publisher_key(key_id, public_key, revocation_authority=revocation_authority)
        publisher = self.trust_store.publisher_status(key_id, public_key)
        return {**self.security_status(), "publisher": {**publisher, "public_key": public_key}}

    def set_publisher_revocation_authority(self, key_id: str, enabled: bool) -> dict[str, Any]:
        self.trust_store.set_revocation_authority(key_id, enabled)
        return self.security_status()

    def import_revocations_json(self, document_json: Any) -> dict[str, Any]:
        return self.trust_store.import_revocations_json(document_json)

    def unpin_publisher(self, key_id: str) -> dict[str, Any]:
        removed = self.trust_store.unpin_publisher_key(key_id)
        return {**self.security_status(), "key_id": key_id, "unpinned": removed}

    def revoke_plugin(self, plugin_id: str, reason: str = "") -> dict[str, Any]:
        self.trust_store.revoke_plugin(plugin_id, reason)
        return self._plugin_status(plugin_id)

    def unrevoke_plugin(self, plugin_id: str) -> dict[str, Any]:
        self.trust_store.unrevoke_plugin(plugin_id)
        return self._plugin_status(plugin_id)

    def runtime_source(self, plugin_id: str) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        if not (directory / PLUGIN_V2_MANIFEST).exists():
            raise LauncherError("Выполняемый runtime доступен только для plugin.json")
        package = _read_v2_package(directory, expected_id=plugin_id)
        self._require_executable(package, plugin_id)
        entrypoint = package["manifest"]["entrypoint"]
        source_bytes = package["files"].get(entrypoint)
        if source_bytes is None or len(source_bytes) > PLUGIN_RUNTIME_MAX_SOURCE_BYTES:
            raise LauncherError("entrypoint плагина отсутствует или превышает лимит runtime")
        try:
            source = source_bytes.decode("utf-8")
        except UnicodeError as exc:
            raise LauncherError("entrypoint плагина должен быть UTF-8") from exc
        return {
            "manifest": package["manifest"],
            "entrypoint": entrypoint,
            "source": source,
            "package_sha256": package["package_digest"],
        }

    def runtime_info(self, plugin_id: str) -> dict[str, Any]:
        directory = self._existing_plugin_directory(plugin_id)
        if not (directory / PLUGIN_V2_MANIFEST).exists():
            raise LauncherError("Выполняемый runtime доступен только для plugin.json")
        package = _read_v2_package(directory, expected_id=plugin_id)
        manifest = package["manifest"]
        capabilities = _runtime_capabilities_for_manifest(manifest)
        return {
            "plugin_id": plugin_id,
            "api_version": manifest["api_version"],
            "permissions": list(manifest.get("permissions", [])),
            "capabilities": capabilities,
            "sandbox_profile": str(manifest.get("sandbox_profile", "strict" if manifest.get("api_version") == "3" else "quickjs-legacy")),
            "package_sha256": package["package_digest"],
        }

    def permission_summary(self, plugin_id: str) -> dict[str, Any]:
        plugin = self._plugin_status(plugin_id)
        return {
            "plugin_id": plugin_id,
            "requested_permissions": list(plugin.get("requested_permissions", [])),
            "granted_permissions": list(plugin.get("granted_permissions", [])),
            "execution_allowed": bool(plugin.get("execution_allowed", False)),
            "api_version": plugin.get("api_version", "1"),
        }

    def launcher_root(self) -> Path:
        return Path(__file__).resolve().parent.parent

    def list_launcher_files(self, directory: str = "") -> list[dict[str, Any]]:
        root = self.launcher_root()
        target = root if not directory else _safe_child(root, directory)
        if target.is_symlink() or not target.exists():
            return []
        if not target.is_dir():
            raise LauncherError("Каталог launcher-root недоступен")
        files: list[dict[str, Any]] = []
        for path in sorted(target.rglob("*"), key=lambda item: str(item).lower()):
            if path.is_symlink() or not path.is_file():
                continue
            metadata = path.stat()
            files.append({
                "path": str(path.relative_to(root)),
                "size": metadata.st_size,
                "modified_at": metadata.st_mtime,
            })
            if len(files) >= 2000:
                break
        return files

    def read_launcher_file(self, relative_path: str) -> str:
        path = _safe_child(self.launcher_root(), relative_path)
        if not path.is_file() or path.stat().st_size > 2 * 1024 * 1024:
            raise LauncherError("Файл launcher-root не найден или слишком большой")
        try:
            return path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            raise LauncherError(f"Не удалось прочитать файл launcher-root: {exc}") from exc

    def projected_launcher_config(self, schema_id: str) -> dict[str, Any]:
        settings = getattr(self._config, "settings", None)
        projections = {
            "launcher-public-config": [
                "theme", "active_theme_id", "accent_color", "sidebar_position", "sidebar_mode", "sidebar_width_mode",
                "sidebar_width", "sidebar_floating", "sidebar_visibility", "animations", "density", "ui_scale",
                "graphics_mode", "launcher_version", "startup_view", "last_view", "remember_last_view",
                "default_version_filter", "default_loader", "max_downloads", "download_timeout_seconds", "download_retries",
                "show_activity_panel", "show_toasts", "font_family", "keep_logs_days", "developer_mode",
            ],
            "network-diagnostics": [
                "max_downloads", "download_timeout_seconds", "download_retries", "telemetry", "offline_debug_limit_seconds",
            ],
        }
        removed = {
            "launcher-public-config": ["client_id", "telemetry_endpoint", "http_proxy", "cloud_username", "cloud_url", "cloud_remote_path"],
            "network-diagnostics": ["http_proxy", "client_id", "cloud_username", "cloud_url"],
        }
        allowed = projections.get(schema_id)
        if allowed is None:
            raise LauncherError("Неизвестная схема projected config")
        data = {
            key: getattr(settings, key)
            for key in allowed
            if settings is not None and hasattr(settings, key)
        }
        return {
            "schema_id": schema_id,
            "data": data,
            "removed_fields": removed.get(schema_id, []),
        }

    def quota_usage(self, plugin_id: str) -> dict[str, Any]:
        namespace = self.namespace(plugin_id)
        buckets = {
            "storage": namespace / "storage",
            "config": namespace / "config",
            "files": namespace / "files",
            "database": namespace / "database",
        }

        def _directory_usage(path: Path) -> dict[str, Any]:
            total_bytes = 0
            file_count = 0
            if not path.exists():
                return {"bytes": 0, "files": 0}
            if path.is_symlink():
                raise LauncherError("Каталог данных плагина является символьной ссылкой")
            for item in path.rglob("*"):
                if item.is_symlink() or not item.is_file():
                    continue
                metadata = item.stat()
                total_bytes += metadata.st_size
                file_count += 1
            return {"bytes": total_bytes, "files": file_count}

        directories = {name: _directory_usage(path) for name, path in buckets.items()}
        return {
            "plugin_id": plugin_id,
            "totals": {
                "bytes": sum(int(item["bytes"]) for item in directories.values()),
                "files": sum(int(item["files"]) for item in directories.values()),
            },
            "directories": directories,
        }

    def security_status(self) -> dict[str, Any]:
        plugins = self.list()
        publishers: dict[str, dict[str, Any]] = {}
        for plugin in plugins:
            key_id = plugin.get("publisher_key_id")
            public_key = plugin.get("publisher_public_key")
            if not isinstance(key_id, str) or not isinstance(public_key, str):
                continue
            try:
                status = self.trust_store.publisher_status(key_id, public_key)
            except LauncherError:
                continue
            publishers[key_id] = {
                **status,
                "public_key": public_key,
                "fingerprint": plugin.get("publisher_fingerprint"),
            }
        return {"plugins": plugins, "publishers": list(publishers.values())}

    def _require_executable(
        self,
        package: dict[str, Any],
        plugin_id: str,
        *,
        require_enabled: bool = True,
    ) -> None:
        if self.trust_store.is_plugin_revoked(plugin_id):
            raise LauncherError("Плагин отозван локально или списком отзыва")
        if package["signed"]:
            if not self.trust_store.is_trusted_publisher(package["key_id"], package["public_key"]):
                raise LauncherError("Издатель не закреплён в локальном trust store или отозван")
        elif self._untrusted_execution_hash(plugin_id) != package["package_digest"]:
            raise LauncherError("Для unsigned-плагина требуется явное согласие на локальный untrusted-запуск")
        if require_enabled and not _enabled_state(self._existing_plugin_directory(plugin_id)):
            raise LauncherError("Плагин выключен")

    def _can_execute(self, package: dict[str, Any], plugin_id: str) -> bool:
        if self.trust_store.is_plugin_revoked(plugin_id):
            return False
        if package["signed"]:
            return self.trust_store.is_trusted_publisher(package["key_id"], package["public_key"])
        return self._untrusted_execution_hash(plugin_id) == package["package_digest"]

    def _plugin_status(self, plugin_id: str) -> dict[str, Any]:
        for plugin in self.list():
            if plugin.get("id") == plugin_id:
                return plugin
        raise LauncherError("Плагин не установлен")

    def _permission_record(self, plugin_id: str) -> dict[str, Any]:
        with _PLUGIN_ACCESS_LOCK:
            state = _read_permission_state(self.permission_path)
        return state["plugins"].get(plugin_id, {"grants": [], "untrusted_execution_hash": ""})

    def _stored_grants(self, plugin_id: str) -> list[str]:
        return list(self._permission_record(plugin_id)["grants"])

    def _untrusted_execution_hash(self, plugin_id: str) -> str:
        return str(self._permission_record(plugin_id)["untrusted_execution_hash"])

    def _granted_permissions(self, plugin_id: str, declared: list[str]) -> list[str]:
        return sorted(set(declared) & set(self._stored_grants(plugin_id)))

    def _update_permission_record(self, plugin_id: str, value: dict[str, Any] | None) -> None:
        with _PLUGIN_ACCESS_LOCK:
            state = _read_permission_state(self.permission_path)
            if value is None:
                state["plugins"].pop(plugin_id, None)
            else:
                state["plugins"][plugin_id] = value
            _write_permission_state(self.permission_path, state)

    def _reconcile_replacement_permissions(
        self,
        plugin_id: str,
        directory: Path,
        replacement: dict[str, Any] | None,
    ) -> None:
        previous: dict[str, Any] | None = None
        if (directory / PLUGIN_V2_MANIFEST).exists():
            try:
                previous = _read_v2_package(directory, expected_id=plugin_id)
            except LauncherError:
                pass

        previous_permissions = set(previous["manifest"]["permissions"]) if previous else set()
        next_permissions = set(replacement["manifest"]["permissions"]) if replacement else set()
        record = self._permission_record(plugin_id)
        grants = sorted(set(record["grants"]) & previous_permissions & next_permissions)
        execution_hash = ""
        if previous and replacement and previous["package_digest"] == replacement["package_digest"]:
            execution_hash = record["untrusted_execution_hash"]

        if not grants and not execution_hash:
            self._update_permission_record(plugin_id, None)
        else:
            self._update_permission_record(
                plugin_id,
                {"grants": grants, "untrusted_execution_hash": execution_hash},
            )

    def read_storage(self, plugin_id: str, key: str) -> Any:
        path = self._storage_path(plugin_id, key)
        if not path.exists():
            return None
        if path.is_symlink() or path.stat().st_size > 1 * 1024 * 1024:
            raise LauncherError("Данные плагина повреждены или слишком большие")
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Не удалось прочитать данные плагина: {exc}") from exc

    def read_config(self, plugin_id: str) -> dict[str, Any]:
        path = self._config_path(plugin_id)
        if not path.exists():
            return {}
        if path.is_symlink() or path.stat().st_size > 256 * 1024:
            raise LauncherError("config.yml плагина повреждён или слишком большой")
        try:
            value = parse_plugin_yml(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError) as exc:
            raise LauncherError(f"Не удалось прочитать config.yml плагина: {exc}") from exc
        if not isinstance(value, dict):
            raise LauncherError("config.yml плагина должен содержать объект")
        return value

    def write_config(self, plugin_id: str, value: dict[str, Any]) -> None:
        if not isinstance(value, dict):
            raise LauncherError("Конфигурация плагина должна быть объектом")
        path = self._config_path(plugin_id)
        encoded = _dump_yaml(value)
        if len(encoded.encode("utf-8")) > 256 * 1024:
            raise LauncherError("config.yml плагина слишком большой")
        fd, temporary_name = tempfile.mkstemp(prefix=".config.", dir=path.parent)
        temporary = Path(temporary_name)
        try:
            set_private_fd(fd)
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                handle.write(encoded)
                handle.flush()
                os.fsync(handle.fileno())
            temporary.replace(path)
        finally:
            if temporary.exists():
                temporary.unlink()

    def config_get(self, plugin_id: str, key: str) -> Any:
        _validate_config_key(key)
        return self.read_config(plugin_id).get(key)

    def config_set(self, plugin_id: str, key: str, value: Any) -> None:
        _validate_config_key(key)
        config = self.read_config(plugin_id)
        config[key] = value
        self.write_config(plugin_id, config)

    def config_remove(self, plugin_id: str, key: str) -> None:
        _validate_config_key(key)
        config = self.read_config(plugin_id)
        config.pop(key, None)
        self.write_config(plugin_id, config)

    def write_storage(self, plugin_id: str, key: str, value: Any) -> None:
        path = self._storage_path(plugin_id, key)
        encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        if len(encoded.encode("utf-8")) > 1 * 1024 * 1024:
            raise LauncherError("Данные плагина слишком большие")
        _atomic_json_write(path, value, private=True)

    def remove_storage(self, plugin_id: str, key: str) -> None:
        path = self._storage_path(plugin_id, key)
        if path.is_symlink():
            raise LauncherError("Данные плагина являются символьной ссылкой")
        if path.exists():
            path.unlink()

    def file_path(self, plugin_id: str, relative_path: str) -> Path:
        directory = self.namespace(plugin_id) / "files"
        return _safe_child(directory, relative_path)

    def list_files(self, plugin_id: str, directory: str = "") -> list[str]:
        root = self.namespace(plugin_id) / "files"
        target = root if not directory else _safe_child(root, directory)
        if target.is_symlink() or not target.exists():
            return []
        if not target.is_dir():
            raise LauncherError("Каталог файлов плагина недоступен")
        files: list[str] = []
        for path in sorted(target.rglob("*"), key=lambda item: str(item).lower()):
            if path.is_file() and not path.is_symlink():
                files.append(str(path.relative_to(root)))
                if len(files) >= 2000:
                    break
        return files

    def list_file_info(self, plugin_id: str, directory: str = "") -> list[dict[str, Any]]:
        root = self.namespace(plugin_id) / "files"
        target = root if not directory else _safe_child(root, directory)
        if target.is_symlink() or not target.exists():
            return []
        if not target.is_dir():
            raise LauncherError("Каталог файлов плагина недоступен")
        files: list[dict[str, Any]] = []
        for path in sorted(target.rglob("*"), key=lambda item: str(item).lower()):
            if path.is_symlink() or not path.is_file():
                continue
            metadata = path.stat()
            files.append({
                "path": str(path.relative_to(root)),
                "size": metadata.st_size,
                "modified_at": metadata.st_mtime,
            })
            if len(files) >= 2000:
                break
        return files

    def read_file(self, plugin_id: str, relative_path: str) -> str:
        path = self.file_path(plugin_id, relative_path)
        if not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
            raise LauncherError("Файл плагина не найден или слишком большой")
        try:
            return path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            raise LauncherError(f"Не удалось прочитать файл плагина: {exc}") from exc

    def write_file(self, plugin_id: str, relative_path: str, content: str) -> None:
        path = self.file_path(plugin_id, relative_path)
        encoded = str(content).encode("utf-8")
        if len(encoded) > 8 * 1024 * 1024:
            raise LauncherError("Файл плагина слишком большой")
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        _assert_no_symlink_chain(path.parent, self.namespace(plugin_id) / "files")
        if path.exists() or path.is_symlink():
            if path.is_symlink():
                raise LauncherError("Символьная ссылка в файлах плагина запрещена")
        path.write_bytes(encoded)

    def remove_file(self, plugin_id: str, relative_path: str) -> None:
        path = self.file_path(plugin_id, relative_path)
        if path.is_symlink():
            raise LauncherError("Символьная ссылка в файлах плагина запрещена")
        if path.exists():
            if not path.is_file():
                raise LauncherError("Можно удалить только файл плагина")
            path.unlink()

    def database(self, plugin_id: str, *, read_only: bool = False) -> sqlite3.Connection:
        directory = self.namespace(plugin_id) / "database"
        path = directory / "plugin.sqlite3"
        if path.is_symlink():
            raise LauncherError("База данных плагина является символьной ссылкой")
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        if path.exists() and path.stat().st_size > PLUGIN_DATABASE_MAX_BYTES:
            raise LauncherError("База данных плагина слишком большая")
        connection = sqlite3.connect(path)
        connection.execute("PRAGMA trusted_schema=OFF")
        connection.execute("PRAGMA foreign_keys=ON")
        connection.execute("PRAGMA max_page_count=16384")
        connection.execute("PRAGMA busy_timeout=1000")
        connection.set_authorizer(_sqlite_authorizer(read_only=read_only))
        return connection

    def _plugin_directory(self, plugin_id: str) -> Path:
        if not PLUGIN_ID_RE.fullmatch(plugin_id):
            raise LauncherError("Небезопасный идентификатор плагина")
        directory = self.root / plugin_id
        if directory.is_symlink():
            raise LauncherError("Каталог плагина является символьной ссылкой")
        directory.mkdir(mode=0o700, exist_ok=True)
        return directory

    def _existing_plugin_directory(self, plugin_id: str) -> Path:
        if not PLUGIN_ID_RE.fullmatch(plugin_id):
            raise LauncherError("Небезопасный идентификатор плагина")
        directory = self.root / plugin_id
        if directory.is_symlink():
            raise LauncherError("Каталог плагина является символьной ссылкой")
        has_legacy = (directory / PLUGIN_MANIFEST).exists() or (directory / PLUGIN_MANIFEST).is_symlink()
        has_v2 = (directory / PLUGIN_V2_MANIFEST).exists() or (directory / PLUGIN_V2_MANIFEST).is_symlink()
        if not directory.is_dir() or not (has_legacy or has_v2):
            raise LauncherError("Плагин не установлен")
        return directory

    def _storage_path(self, plugin_id: str, key: str) -> Path:
        normalized = str(key).strip()
        if not re.fullmatch(r"[A-Za-z0-9._-]{1,120}", normalized):
            raise LauncherError("Небезопасный ключ storage")
        return self.namespace(plugin_id) / "storage" / f"{normalized}.json"

    def _config_path(self, plugin_id: str) -> Path:
        path = self.namespace(plugin_id) / "config" / "config.yml"
        if path.is_symlink():
            raise LauncherError("config.yml плагина является символьной ссылкой")
        return path

def parse_plugin_yml(text: str) -> dict[str, Any]:
    if len(text.encode("utf-8")) > PLUGIN_MAX_MANIFEST_BYTES:
        raise LauncherError("plugin.yml слишком большой")
    lines = text.splitlines()
    root: dict[str, Any] = {}
    stack: list[tuple[int, Any]] = [(-1, root)]
    for index, raw_line in enumerate(lines, start=1):
        if "\t" in raw_line:
            raise LauncherError(f"plugin.yml содержит tab на строке {index}")
        line = _strip_yaml_comment(raw_line).rstrip()
        if not line.strip() or line.lstrip().startswith("---"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        content = line.strip()
        while stack and indent <= stack[-1][0]:
            stack.pop()
        if not stack:
            raise LauncherError(f"Некорректный отступ в plugin.yml на строке {index}")
        parent = stack[-1][1]
        if content.startswith("- "):
            if not isinstance(parent, list):
                raise LauncherError(f"Список plugin.yml находится не под ключом на строке {index}")
            parent.append(_yaml_scalar(content[2:].strip()))
            continue
        if ":" not in content:
            raise LauncherError(f"Некорректная строка plugin.yml: {index}")
        key, raw_value = content.split(":", 1)
        key = key.strip()
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_.-]{0,63}", key):
            raise LauncherError(f"Небезопасный ключ plugin.yml: {key}")
        value = raw_value.strip()
        if value:
            if not isinstance(parent, dict):
                raise LauncherError(f"Объект plugin.yml ожидается на строке {index}")
            parent[key] = _yaml_scalar(value)
            continue
        if not isinstance(parent, dict):
            raise LauncherError(f"Объект plugin.yml ожидается на строке {index}")
        child: dict[str, Any] | list[Any] = [] if _next_is_list(lines, index) else {}
        parent[key] = child
        stack.append((indent, child))
    return root

def _static_json_manifest(root: Path) -> dict[str, Any] | None:
    """plugin.json с api_version 1 — тот же статический контракт, что и plugin.yml."""
    path = root / PLUGIN_V2_MANIFEST
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > PLUGIN_MAX_MANIFEST_BYTES:
            return None
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, UnicodeError):
        return None
    if not isinstance(raw, dict) or str(raw.get("api_version", "1")).strip() != "1":
        return None
    return validate_manifest(raw)

def validate_manifest(value: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise LauncherError("plugin.yml должен содержать объект")
    plugin_id = str(value.get("id", "")).strip().lower()
    name = str(value.get("name", "")).strip()
    version = str(value.get("version", "")).strip()
    entrypoint = str(value.get("entrypoint", "")).strip().replace("\\", "/")
    if not PLUGIN_ID_RE.fullmatch(plugin_id):
        raise LauncherError("plugin.yml содержит небезопасный id")
    if not name or len(name) > 120 or not version:
        raise LauncherError("plugin.yml требует name и version")
    if not entrypoint or entrypoint.startswith("/") or ".." in Path(entrypoint).parts or not entrypoint.endswith(".js"):
        raise LauncherError("entrypoint должен быть относительным .js-файлом")
    permissions = value.get("permissions", [])
    if not isinstance(permissions, list):
        raise LauncherError("permissions в plugin.yml должен быть списком")
    normalized_permissions = [str(item).strip().lower() for item in permissions]
    if any(item not in PLUGIN_PERMISSION_NAMES for item in normalized_permissions):
        raise LauncherError("plugin.yml содержит неизвестное permission")
    network_hosts = value.get("network_hosts", [])
    if not isinstance(network_hosts, list):
        raise LauncherError("network_hosts в plugin.yml должен быть списком")
    hosts = [str(item).strip().lower().rstrip(".") for item in network_hosts if str(item).strip()]
    if any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+", host) for host in hosts):
        raise LauncherError("network_hosts должен содержать только имена хостов")
    for host in hosts:
        try:
            ipaddress.ip_address(host)
        except ValueError:
            continue
        raise LauncherError("network_hosts не может содержать IP-адреса")
    raw_sidebar_tab = value.get("sidebar_tab")
    sidebar_tab: dict[str, str] | None = None
    if raw_sidebar_tab is not None:
        if not isinstance(raw_sidebar_tab, dict) or set(raw_sidebar_tab) != {"label"}:
            raise LauncherError("sidebar_tab должен содержать только поле label")
        raw_label = raw_sidebar_tab.get("label")
        if not isinstance(raw_label, str):
            raise LauncherError("sidebar_tab.label должен быть строкой")
        label = raw_label.strip()
        if not label or len(label) > 40 or not label.isprintable():
            raise LauncherError("sidebar_tab.label должен содержать от 1 до 40 печатных символов")
        if "ui:compose" not in normalized_permissions:
            raise LauncherError("sidebar_tab требует permission ui:compose")
        sidebar_tab = {"label": label}

    manifest = {
        "id": plugin_id,
        "name": name,
        "version": version[:64],
        "author": str(value.get("author", "Local developer"))[:120],
        "description": str(value.get("description", ""))[:500],
        "icon": str(value.get("icon", "plug"))[:64],
        "category": str(value.get("category", "Local"))[:64],
        "api_version": str(value.get("api_version", "1"))[:16],
        "entrypoint": entrypoint,
        "permissions": list(dict.fromkeys(normalized_permissions)),
        "network_hosts": list(dict.fromkeys(hosts)),
        "featured": bool(value.get("featured", False)),
    }
    if sidebar_tab is not None:
        manifest["sidebar_tab"] = sidebar_tab
    return manifest

_PLUGIN_V2_FIELDS = {
    "id", "name", "version", "api_version", "entrypoint", "author", "description", "icon", "category",
    "permissions", "network_hosts", "config", "ui", "i18n", "theme", "actions", "events", "commands",
    "shortcuts", "integrity", "signature",
}
_PLUGIN_V2_EVENTS = {
    "launcher.ready", "instance.selected", "instance.beforeLaunch", "instance.afterExit", "instance.crash",
    "log.line", "mods.beforeInstall", "snapshot.created", "settings.changed", "account.changed", "plugin.enabled",
}
_PLUGIN_V2_UI_SLOTS = {"sidebar_tab", "instance_tab", "settings_section", "home_widget"}
_PLUGIN_V2_UI_TONES = {"default", "muted", "success", "warning", "danger"}
_PLUGIN_V2_UI_DIRECTIONS = {"vertical", "horizontal"}
_PLUGIN_V3_FIELDS = _PLUGIN_V2_FIELDS | {
    "capabilities",
    "consent_scopes",
    "extension_points",
    "dependency_graph",
    "update_channel",
    "sandbox_profile",
    "compatibility_matrix",
    "host_requirements",
}
_PLUGIN_V3_EVENTS = _PLUGIN_V2_EVENTS | {
    "launcher.shuttingDown",
    "instance.launch.requested",
    "instance.preflight.started",
    "instance.preflight.completed",
    "instance.starting",
    "instance.ready",
    "instance.hung",
    "process.memory.thresholdExceeded",
    "network.proxy.connected",
    "network.proxy.failed",
    "files.changed",
}

def validate_v2_manifest(value: dict[str, Any]) -> dict[str, Any]:
    if type(value) is not dict:
        raise LauncherError("plugin.json must contain a JSON object")
    unknown = set(value) - _PLUGIN_V2_FIELDS
    if unknown:
        raise LauncherError(f"plugin.json contains unsupported field(s): {', '.join(sorted(unknown))}")
    required = {"id", "name", "version", "api_version", "entrypoint", "permissions", "integrity"}
    if not required.issubset(value):
        raise LauncherError("plugin.json is missing a required v2 field")
    for field, limit, allow_empty in (
        ("id", 64, False), ("name", 120, False), ("version", 64, False),
        ("api_version", 16, False), ("entrypoint", 256, False),
    ):
        _schema_text(value[field], f"plugin.json.{field}", limit, allow_empty=allow_empty)
    _v2_package_path(value["entrypoint"])
    if value["api_version"] != "2":
        raise LauncherError("plugin.json api_version must be \"2\"")
    if type(value["permissions"]) is not list or any(type(item) is not str for item in value["permissions"]):
        raise LauncherError("plugin.json permissions must be an array of strings")
    if any(item not in PLUGIN_V2_PERMISSION_NAMES for item in value["permissions"]):
        raise LauncherError("plugin.json contains an unknown permission")
    if "network_hosts" in value and (
        type(value["network_hosts"]) is not list or any(type(item) is not str for item in value["network_hosts"])
    ):
        raise LauncherError("plugin.json network_hosts must be an array of strings")
    for field, limit in (("author", 120), ("description", 500), ("icon", 64), ("category", 64)):
        if field in value:
            _schema_text(value[field], f"plugin.json.{field}", limit, allow_empty=(field == "description"))
    if "integrity" not in value:
        raise LauncherError("plugin.json integrity metadata is required")
    _schema_object(value["integrity"], "plugin.json.integrity", {"files"}, {"files"})
    if type(value["integrity"]["files"]) is not dict:
        raise LauncherError("plugin.json integrity.files must be an object")
    if "signature" in value and type(value["signature"]) is not dict:
        raise LauncherError("plugin.json signature must be an object")

    validated = validate_manifest(value)
    if value["id"] != validated["id"] or value["entrypoint"] != validated["entrypoint"]:
        raise LauncherError("plugin.json id and entrypoint must use their normalized forms")
    if "config" in value:
        _validate_v2_config(value["config"])
    if "ui" in value:
        _validate_v2_ui(value["ui"])
    if "i18n" in value:
        _validate_v2_i18n(value["i18n"])
    if "theme" in value:
        _validate_v2_theme(value["theme"])
    if "actions" in value:
        _validate_v2_actions(value["actions"])
    if "events" in value:
        events = _schema_array(value["events"], "plugin.json.events", 64)
        if any(type(item) is not str or item not in _PLUGIN_V2_EVENTS for item in events):
            raise LauncherError("plugin.json.events contains an unsupported launcher event")
    if "commands" in value:
        _validate_v2_commands(value["commands"])
    if "shortcuts" in value:
        _validate_v2_shortcuts(value["shortcuts"])

    result = dict(value)
    result.update({
        "id": validated["id"],
        "name": validated["name"],
        "version": validated["version"],
        "author": validated["author"],
        "description": validated["description"],
        "icon": validated["icon"],
        "category": validated["category"],
        "api_version": "2",
        "entrypoint": validated["entrypoint"],
        "permissions": validated["permissions"],
        "network_hosts": validated["network_hosts"],
    })
    return result

def validate_v3_manifest(value: dict[str, Any]) -> dict[str, Any]:
    if type(value) is not dict:
        raise LauncherError("plugin.json must contain a JSON object")
    unknown = set(value) - _PLUGIN_V3_FIELDS
    if unknown:
        raise LauncherError(f"plugin.json contains unsupported field(s): {', '.join(sorted(unknown))}")
    required = {"id", "name", "version", "api_version", "entrypoint", "permissions", "integrity"}
    if not required.issubset(value):
        raise LauncherError("plugin.json is missing a required v3 field")
    for field, limit, allow_empty in (
        ("id", 64, False), ("name", 120, False), ("version", 64, False),
        ("api_version", 16, False), ("entrypoint", 256, False),
    ):
        _schema_text(value[field], f"plugin.json.{field}", limit, allow_empty=allow_empty)
    _v2_package_path(value["entrypoint"])
    if value["api_version"] != "3":
        raise LauncherError("plugin.json api_version must be \"3\"")
    if type(value["permissions"]) is not list or any(type(item) is not str for item in value["permissions"]):
        raise LauncherError("plugin.json permissions must be an array of strings")
    if any(item not in PLUGIN_V3_PERMISSION_NAMES for item in value["permissions"]):
        raise LauncherError("plugin.json contains an unknown permission")
    if "network_hosts" in value and (
        type(value["network_hosts"]) is not list or any(type(item) is not str for item in value["network_hosts"])
    ):
        raise LauncherError("plugin.json network_hosts must be an array of strings")
    for field, limit in (("author", 120), ("description", 500), ("icon", 64), ("category", 64), ("update_channel", 64), ("sandbox_profile", 64)):
        if field in value:
            _schema_text(value[field], f"plugin.json.{field}", limit, allow_empty=(field == "description"))
    _schema_object(value["integrity"], "plugin.json.integrity", {"files"}, {"files"})
    if type(value["integrity"]["files"]) is not dict:
        raise LauncherError("plugin.json integrity.files must be an object")
    if "signature" in value and type(value["signature"]) is not dict:
        raise LauncherError("plugin.json signature must be an object")

    validated = validate_manifest(value)
    if value["id"] != validated["id"] or value["entrypoint"] != validated["entrypoint"]:
        raise LauncherError("plugin.json id and entrypoint must use their normalized forms")
    if "config" in value:
        _validate_v2_config(value["config"])
    if "ui" in value:
        _validate_v2_ui(value["ui"])
    if "i18n" in value:
        _validate_v2_i18n(value["i18n"])
    if "theme" in value:
        _validate_v2_theme(value["theme"])
    if "actions" in value:
        _validate_v2_actions(value["actions"])
    if "events" in value:
        events = _schema_array(value["events"], "plugin.json.events", 128)
        if any(type(item) is not str or item not in _PLUGIN_V3_EVENTS for item in events):
            raise LauncherError("plugin.json.events contains an unsupported launcher event")
    if "commands" in value:
        _validate_v2_commands(value["commands"])
    if "shortcuts" in value:
        _validate_v2_shortcuts(value["shortcuts"])
    for field, maximum in (
        ("capabilities", 64 * 1024),
        ("consent_scopes", 16 * 1024),
        ("extension_points", 16 * 1024),
        ("dependency_graph", 64 * 1024),
        ("compatibility_matrix", 64 * 1024),
        ("host_requirements", 32 * 1024),
    ):
        if field in value:
            _validate_bounded_json(value[field], f"plugin.json.{field}", maximum)

    result = dict(value)
    result.update({
        "id": validated["id"],
        "name": validated["name"],
        "version": validated["version"],
        "author": validated["author"],
        "description": validated["description"],
        "icon": validated["icon"],
        "category": validated["category"],
        "api_version": "3",
        "entrypoint": validated["entrypoint"],
        "permissions": validated["permissions"],
        "network_hosts": validated["network_hosts"],
    })
    return result

def validate_plugin_json_manifest(value: dict[str, Any]) -> dict[str, Any]:
    if type(value) is not dict:
        raise LauncherError("plugin.json must contain a JSON object")
    api_version = value.get("api_version")
    if api_version == "2":
        return validate_v2_manifest(value)
    if api_version == "3":
        return validate_v3_manifest(value)
    raise LauncherError("plugin.json api_version must be \"2\" or \"3\"")

def _schema_text(value: Any, label: str, maximum: int, *, allow_empty: bool = False) -> str:
    if type(value) is not str or len(value) > maximum or (not allow_empty and not value.strip()) or "\x00" in value:
        raise LauncherError(f"{label} must be text of at most {maximum} characters")
    try:
        value.encode("utf-8")
    except UnicodeEncodeError as exc:
        raise LauncherError(f"{label} must be valid UTF-8") from exc
    return value

def _schema_object(value: Any, label: str, allowed: set[str], required: set[str]) -> dict[str, Any]:
    if type(value) is not dict or not required.issubset(value) or set(value) - allowed:
        raise LauncherError(f"{label} has an invalid object shape")
    return value

def _schema_array(value: Any, label: str, maximum: int = 256) -> list[Any]:
    if type(value) is not list or len(value) > maximum:
        raise LauncherError(f"{label} must be an array of at most {maximum} items")
    return value

def _schema_identifier(value: Any, label: str) -> str:
    text = _schema_text(value, label, 80)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}", text):
        raise LauncherError(f"{label} is not a safe identifier")
    return text

def _schema_optional_text(record: dict[str, Any], key: str, label: str, maximum: int) -> None:
    if key in record:
        _schema_text(record[key], f"{label}.{key}", maximum, allow_empty=True)

def _schema_bool(record: dict[str, Any], key: str, label: str) -> None:
    if key in record and type(record[key]) is not bool:
        raise LauncherError(f"{label}.{key} must be a boolean")

def _schema_number(record: dict[str, Any], key: str, label: str) -> None:
    value = record.get(key)
    if key in record and not _finite_number(value):
        raise LauncherError(f"{label}.{key} must be a finite number")

def _finite_number(value: Any) -> bool:
    if type(value) not in (int, float):
        return False
    try:
        return math.isfinite(value)
    except OverflowError:
        return False

def _validate_bounded_json(value: Any, label: str, max_bytes: int, *, max_depth: int = 12) -> None:
    def _walk(node: Any, depth: int) -> None:
        if depth > max_depth:
            raise LauncherError(f"{label} exceeds the supported nesting depth")
        if node is None or type(node) in {str, bool, int, float}:
            if type(node) in {int, float} and not _finite_number(node):
                raise LauncherError(f"{label} contains a non-finite number")
            return
        if type(node) is list:
            if len(node) > 1024:
                raise LauncherError(f"{label} contains too many array items")
            for item in node:
                _walk(item, depth + 1)
            return
        if type(node) is dict:
            if len(node) > 1024:
                raise LauncherError(f"{label} contains too many object fields")
            for key, item in node.items():
                if type(key) is not str or len(key) > 256:
                    raise LauncherError(f"{label} contains an unsafe object key")
                _walk(item, depth + 1)
            return
        raise LauncherError(f"{label} contains an unsupported JSON value")

    _walk(value, 0)
    try:
        encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")
    except (TypeError, ValueError, UnicodeEncodeError, RecursionError) as exc:
        raise LauncherError(f"{label} must be serializable as bounded JSON") from exc
    if len(encoded) > max_bytes:
        raise LauncherError(f"{label} exceeds its bounded JSON size limit")

def _validate_v2_option(value: Any, label: str) -> None:
    option = _schema_object(value, label, {"value", "label", "description", "disabled"}, {"value", "label"})
    _schema_text(option["value"], f"{label}.value", 256, allow_empty=True)
    _schema_text(option["label"], f"{label}.label", 256)
    _schema_optional_text(option, "description", label, 1000)
    _schema_bool(option, "disabled", label)

def _validate_v2_config(value: Any) -> None:
    if type(value) is not dict or len(value) > 256:
        raise LauncherError("plugin.json.config must be a bounded object")
    for key, raw_field in value.items():
        if not re.fullmatch(r"[A-Za-z0-9._-]{1,120}", key):
            raise LauncherError("plugin.json.config contains an unsafe key")
        label = f"plugin.json.config.{key}"
        field = _schema_object(
            raw_field,
            label,
            {"type", "label", "description", "default", "minimum", "maximum", "options"},
            {"type", "label"},
        )
        if field["type"] not in {"string", "number", "boolean", "select"}:
            raise LauncherError(f"{label}.type is unsupported")
        _schema_text(field["label"], f"{label}.label", 256)
        _schema_optional_text(field, "description", label, 1000)
        if "default" in field:
            default = field["default"]
            if field["type"] == "string" and type(default) is not str:
                raise LauncherError(f"{label}.default must be text")
            if field["type"] == "number" and not _finite_number(default):
                raise LauncherError(f"{label}.default must be a finite number")
            if field["type"] == "boolean" and type(default) is not bool:
                raise LauncherError(f"{label}.default must be a boolean")
            if field["type"] == "select" and type(default) is not str:
                raise LauncherError(f"{label}.default must be text")
        for bound in ("minimum", "maximum"):
            _schema_number(field, bound, label)
        if "minimum" in field and "maximum" in field and field["minimum"] > field["maximum"]:
            raise LauncherError(f"{label}.minimum must not exceed maximum")
        if "options" in field:
            options = _schema_array(field["options"], f"{label}.options", 128)
            seen: set[str] = set()
            for option in options:
                _validate_v2_option(option, f"{label}.options[]")
                if option["value"] in seen:
                    raise LauncherError(f"{label}.options contains duplicate values")
                seen.add(option["value"])

def _validate_v2_ui(value: Any) -> None:
    ui = _schema_object(value, "plugin.json.ui", {"views"}, {"views"})
    views = _schema_array(ui["views"], "plugin.json.ui.views", 128)
    seen: set[str] = set()
    count = [0]
    for raw_view in views:
        view = _schema_object(
            raw_view,
            "plugin.json.ui.views[]",
            {"id", "slot", "title", "description", "icon", "order", "root"},
            {"id", "slot", "title", "root"},
        )
        view_id = _schema_identifier(view["id"], "plugin.json.ui.views[].id")
        if view_id in seen:
            raise LauncherError("plugin.json.ui.views contains duplicate ids")
        seen.add(view_id)
        if type(view["slot"]) is not str or view["slot"] not in _PLUGIN_V2_UI_SLOTS:
            raise LauncherError("plugin.json.ui.views[].slot is unsupported")
        _schema_text(view["title"], "plugin.json.ui.views[].title", 160)
        _schema_optional_text(view, "description", "plugin.json.ui.views[]", 2000)
        _schema_optional_text(view, "icon", "plugin.json.ui.views[]", 64)
        _schema_number(view, "order", "plugin.json.ui.views[]")
        _validate_v2_ui_node(view["root"], "plugin.json.ui.views[].root", 0, [0, 0])

def _validate_v2_ui_node(value: Any, label: str, depth: int, count: list[int]) -> None:
    if len(count) < 2: count.append(0)
    def _ui_array(value, label, maximum=256):
        result = _schema_array(value, label, maximum)
        count[1] += len(result)
        if count[1] > 512: raise LauncherError("UI collection budget exceeded")
        return result
    count[0] += 1
    if depth > 10 or count[0] > 512:
        raise LauncherError("plugin.json UI tree exceeds its depth or node limit")
    if type(value) is not dict or type(value.get("type")) is not str:
        raise LauncherError(f"{label} must be a plugin UI node")
    kind = value["type"]
    fields: dict[str, tuple[set[str], set[str]]] = {
        "form": ({"type", "id", "fields", "submitAction"}, {"type", "id", "fields", "submitAction"}),
        "tabs": ({"type", "id", "tabs"}, {"type", "id", "tabs"}),
        "accordion": ({"type", "id", "sections"}, {"type", "id", "sections"}),
        "table": ({"type", "id", "columns", "rows"}, {"type", "id", "columns", "rows"}),
        "chart": ({"type", "id", "label", "values"}, {"type", "id", "values"}),
        "include": ({"type", "id", "fragment"}, {"type", "id", "fragment"}),
        "text": ({"type", "id", "text", "tone"}, {"type", "text"}),
        "badge": ({"type", "id", "label", "tone"}, {"type", "label"}),
        "button": ({"type", "id", "label", "action", "disabled", "tone"}, {"type", "id", "label", "action"}),
        "input": ({"type", "id", "label", "value", "placeholder", "config_key", "action", "onChange"}, {"type", "id", "label"}),
        "textarea": ({"type", "id", "label", "value", "placeholder", "rows", "config_key", "action", "onChange"}, {"type", "id", "label"}),
        "number": ({"type", "id", "label", "value", "min", "max", "step", "unit", "config_key", "action", "onChange"}, {"type", "id", "label"}),
        "slider": ({"type", "id", "label", "value", "min", "max", "step", "unit", "config_key", "action", "onChange"}, {"type", "id", "label", "min", "max"}),
        "color": ({"type", "id", "label", "value", "config_key", "onChange"}, {"type", "id", "label"}),
        "datetime": ({"type", "id", "label", "value", "config_key", "action", "onChange"}, {"type", "id", "label"}),
        "select": ({"type", "id", "label", "options", "value", "config_key", "action", "onChange"}, {"type", "id", "label", "options"}),
        "checkbox": ({"type", "id", "label", "checked", "config_key", "action", "onChange"}, {"type", "id", "label"}),
        "switch": ({"type", "id", "label", "checked", "hint", "config_key", "action", "onChange"}, {"type", "id", "label"}),
        "progress": ({"type", "id", "value", "max", "label"}, {"type", "value"}),
        "metric": ({"type", "id", "label", "value", "unit", "tone"}, {"type", "label", "value"}),
        "alert": ({"type", "id", "title", "text", "tone"}, {"type", "text"}),
        "code": ({"type", "id", "text"}, {"type", "text"}),
        "image": ({"type", "id", "url", "alt", "height"}, {"type", "url"}),
        "list": ({"type", "id", "items"}, {"type", "items"}),
        "link": ({"type", "id", "label", "url", "action"}, {"type", "label", "url"}),
        "divider": ({"type", "id"}, {"type"}),
        "group": ({"type", "id", "label", "direction", "children"}, {"type", "children"}),
        "card": ({"type", "id", "label", "tone", "children"}, {"type", "children"}),
        "grid": ({"type", "id", "columns", "children"}, {"type", "children"}),
    }
    shape = fields.get(kind)
    if shape is None:
        raise LauncherError(f"{label}.type is unsupported")
    node = _schema_object(value, label, shape[0], shape[1])
    for field in ("id", "action", "config_key", "onChange"):
        if field in node:
            _schema_identifier(node[field], f"{label}.{field}")
    for field in ("label", "text", "placeholder", "url", "title", "hint", "unit", "alt"):
        if field in node:
            _schema_text(node[field], f"{label}.{field}", 4096, allow_empty=(field in {"value", "placeholder"}))
    if kind in {"input", "select", "textarea", "color", "datetime"} and "value" in node:
        _schema_text(node["value"], f"{label}.value", 4096, allow_empty=True)
    if kind == "color" and "value" in node and not re.fullmatch(r"#[0-9A-Fa-f]{6}", str(node["value"])):
        raise LauncherError(f"{label}.value must be #rrggbb")
    if kind == "image" and "url" in node:
        _schema_text(node["url"], f"{label}.url", 4096)
    if "tone" in node and (type(node["tone"]) is not str or node["tone"] not in _PLUGIN_V2_UI_TONES):
        raise LauncherError(f"{label}.tone is unsupported")
    for field in ("disabled", "checked"):
        _schema_bool(node, field, label)
    for field in ("value", "max"):
        if kind == "progress" and field in node:
            _schema_number(node, field, label)
    if kind in {"number", "slider"}:
        for field in ("value", "min", "max", "step"):
            if field in node:
                _schema_number(node, field, label)
        if "min" in node and "max" in node and float(node["min"]) > float(node["max"]):
            raise LauncherError(f"{label}.min exceeds {label}.max")
    if kind == "textarea" and "rows" in node:
        _schema_number(node, "rows", label)
        if not 1 <= float(node["rows"]) <= 24:
            raise LauncherError(f"{label}.rows is out of range")
    if kind == "image" and "height" in node:
        _schema_number(node, "height", label)
        if not 16 <= float(node["height"]) <= 720:
            raise LauncherError(f"{label}.height is out of range")
    if kind == "grid" and "columns" in node:
        _schema_number(node, "columns", label)
        if float(node["columns"]) != int(node["columns"]) or not 1 <= int(node["columns"]) <= 4:
            raise LauncherError(f"{label}.columns is out of range")
    if kind == "include":
        _schema_identifier(node["fragment"], f"{label}.fragment")
    if kind == "form":
        _schema_identifier(node["submitAction"], f"{label}.submitAction")
        for child in _ui_array(node["fields"], f"{label}.fields", 512):
            _validate_v2_ui_node(child, f"{label}.fields[]", depth + 1, count)
    if kind in {"tabs", "accordion"}:
        key = "tabs" if kind == "tabs" else "sections"
        ids = set()
        for entry in _ui_array(node[key], f"{label}.{key}", 64):
            _schema_object(entry, label, {"id", "label", "content"}, {"id", "label", "content"})
            identifier = _schema_identifier(entry["id"], label)
            if identifier in ids: raise LauncherError("Duplicate UI tab/section id")
            ids.add(identifier)
            _schema_text(entry["label"], label, 256)
            _validate_v2_ui_node(entry["content"], label, depth + 1, count)
    if kind == "table":
        for column in _ui_array(node["columns"], label, 32):
            _schema_object(column, label, {"key", "label"}, {"key", "label"})
            _schema_identifier(column["key"], label)
            _schema_text(column["label"], label, 256)
        for row in _ui_array(node["rows"], label, 512):
            if type(row) is not dict or len(row) > 32: raise LauncherError("Invalid table row")
            for cell in row.values():
                if type(cell) is str: _schema_text(cell, label, 4096, allow_empty=True)
                elif not _finite_number(cell): raise LauncherError("Invalid table cell")
    if kind == "chart":
        if any(not _finite_number(item) for item in _ui_array(node["values"], label, 512)):
            raise LauncherError("Invalid chart value")
    if kind == "select":
        options = _ui_array(node["options"], f"{label}.options", 128)
        if not options:
            raise LauncherError(f"{label}.options must not be empty")
        for option in options:
            _validate_v2_option(option, f"{label}.options[]")
    if kind == "list":
        items = _ui_array(node["items"], f"{label}.items", 512)
        for item in items:
            if type(item) is str:
                _schema_text(item, f"{label}.items[]", 4096, allow_empty=True)
                continue
            record = _schema_object(item, f"{label}.items[]", {"id", "label", "detail", "badge", "action"}, {"id", "label"})
            _schema_identifier(record["id"], f"{label}.items[].id")
            _schema_text(record["label"], f"{label}.items[].label", 256)
            for field in ("detail", "badge", "action"):
                _schema_optional_text(record, field, f"{label}.items[]", 512)
    if kind == "group":
        if "label" in node:
            _schema_text(node["label"], f"{label}.label", 256, allow_empty=True)
        if "direction" in node and (type(node["direction"]) is not str or node["direction"] not in _PLUGIN_V2_UI_DIRECTIONS):
            raise LauncherError(f"{label}.direction is unsupported")
        for child in _ui_array(node["children"], f"{label}.children", 512):
            _validate_v2_ui_node(child, f"{label}.children[]", depth + 1, count)
    if kind in {"card", "grid"}:
        if "label" in node and kind == "card":
            _schema_text(node["label"], f"{label}.label", 256, allow_empty=True)
        for child in _ui_array(node["children"], f"{label}.children", 512):
            _validate_v2_ui_node(child, f"{label}.children[]", depth + 1, count)

def _validate_v2_i18n(value: Any) -> None:
    i18n = _schema_object(value, "plugin.json.i18n", {"default_locale", "messages"}, {"default_locale", "messages"})
    _schema_text(i18n["default_locale"], "plugin.json.i18n.default_locale", 35)
    messages = i18n["messages"]
    if type(messages) is not dict or len(messages) > 32:
        raise LauncherError("plugin.json.i18n.messages must be a bounded object")
    for locale, raw_table in messages.items():
        _schema_text(locale, "plugin.json.i18n.locale", 35)
        if type(raw_table) is not dict or len(raw_table) > 2048:
            raise LauncherError("plugin.json.i18n locale table must be a bounded object")
        for key, text in raw_table.items():
            _schema_text(key, "plugin.json.i18n message key", 256)
            _schema_text(text, "plugin.json.i18n message", 4096, allow_empty=True)

def _validate_v2_theme(value: Any) -> None:
    theme = _schema_object(value, "plugin.json.theme", {"name", "colors", "tokens"}, set())
    if "name" in theme:
        _schema_text(theme["name"], "plugin.json.theme.name", 120)
    for field, allow_number in (("colors", False), ("tokens", True)):
        if field not in theme:
            continue
        mapping = theme[field]
        if type(mapping) is not dict or len(mapping) > 256:
            raise LauncherError(f"plugin.json.theme.{field} must be a bounded object")
        for key, item in mapping.items():
            _schema_identifier(key, f"plugin.json.theme.{field} key")
            if type(item) is str:
                _schema_text(item, f"plugin.json.theme.{field}.{key}", 256, allow_empty=True)
            elif not (allow_number and _finite_number(item)):
                raise LauncherError(f"plugin.json.theme.{field}.{key} has an unsupported value")

def _validate_v2_actions(value: Any) -> None:
    actions = _schema_array(value, "plugin.json.actions", 256)
    _validate_v2_named_records(
        actions,
        "plugin.json.actions[]",
        {"id", "label", "description", "icon", "confirmation"},
        {"id", "label"},
    )

def _validate_v2_named_records(value: list[Any], label: str, allowed: set[str], required: set[str]) -> None:
    seen: set[str] = set()
    for item in value:
        record = _schema_object(item, label, allowed, required)
        identifier = _schema_identifier(record["id"], f"{label}.id")
        if identifier in seen:
            raise LauncherError(f"{label} contains duplicate ids")
        seen.add(identifier)
        _schema_text(record["label"], f"{label}.label", 256)
        for field in required - {"id", "label"}:
            _schema_text(record[field], f"{label}.{field}", 256)
        for field in allowed - required - {"id"}:
            _schema_optional_text(record, field, label, 1000)

def _validate_v2_commands(value: Any) -> None:
    commands = _schema_array(value, "plugin.json.commands", 256)
    seen: set[str] = set()
    for raw_command in commands:
        command = _schema_object(
            raw_command,
            "plugin.json.commands[]",
            {"id", "label", "description", "arguments"},
            {"id", "label"},
        )
        identifier = _schema_identifier(command["id"], "plugin.json.commands[].id")
        if identifier in seen:
            raise LauncherError("plugin.json.commands contains duplicate ids")
        seen.add(identifier)
        _schema_text(command["label"], "plugin.json.commands[].label", 256)
        _schema_optional_text(command, "description", "plugin.json.commands[]", 1000)
        if "arguments" in command:
            args = _schema_array(command["arguments"], "plugin.json.commands[].arguments", 64)
            arg_names: set[str] = set()
            for raw_arg in args:
                arg = _schema_object(
                    raw_arg,
                    "plugin.json.commands[].arguments[]",
                    {"name", "description", "required", "choices"},
                    {"name"},
                )
                name = _schema_identifier(arg["name"], "plugin.json.commands[].arguments[].name")
                if name in arg_names:
                    raise LauncherError("plugin.json command arguments contain duplicate names")
                arg_names.add(name)
                _schema_optional_text(arg, "description", "plugin.json.commands[].arguments[]", 1000)
                _schema_bool(arg, "required", "plugin.json.commands[].arguments[]")
                if "choices" in arg:
                    choices = _schema_array(arg["choices"], "plugin.json.commands[].arguments[].choices", 256)
                    for choice in choices:
                        _schema_text(choice, "plugin.json command choice", 256)

def _validate_v2_shortcuts(value: Any) -> None:
    shortcuts = _schema_array(value, "plugin.json.shortcuts", 256)
    _validate_v2_named_records(
        shortcuts,
        "plugin.json.shortcuts[]",
        {"id", "label", "default_key", "description"},
        {"id", "label", "default_key"},
    )

def _read_permission_state(path: Path) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return {"schema_version": _PLUGIN_PERMISSION_SCHEMA, "plugins": {}}
    except OSError as exc:
        raise LauncherError("Could not inspect plugin permission grants") from exc
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 1024 * 1024:
        raise LauncherError("Plugin permission grants must be a bounded regular file")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise LauncherError("Plugin permission grants must be a regular file")
            if hasattr(os, "fchmod"):
                os.fchmod(handle.fileno(), 0o600)
            content = handle.read(1024 * 1024 + 1)
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError("Could not securely read plugin permission grants") from exc
    if len(content) > 1024 * 1024:
        raise LauncherError("Plugin permission grants file is too large")
    try:
        state = json.loads(
            content.decode("utf-8"),
            object_pairs_hook=_unique_json_object,
            parse_constant=_reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise LauncherError("Plugin permission grants contain invalid JSON") from exc
    return _validate_permission_state(state)

def _validate_permission_state(value: Any) -> dict[str, Any]:
    if type(value) is not dict or set(value) != {"schema_version", "plugins"}:
        raise LauncherError("Plugin permission grants have an unsupported format")
    if type(value["schema_version"]) is not int or value["schema_version"] != _PLUGIN_PERMISSION_SCHEMA:
        raise LauncherError("Plugin permission grants have an unsupported schema version")
    plugins = value["plugins"]
    if type(plugins) is not dict:
        raise LauncherError("Plugin permission grants plugins must be an object")
    for plugin_id, record in plugins.items():
        if not PLUGIN_ID_RE.fullmatch(plugin_id):
            raise LauncherError("Plugin permission grants contain an unsafe plugin id")
        if type(record) is not dict or set(record) != {"grants", "untrusted_execution_hash"}:
            raise LauncherError("Plugin permission grants contain an invalid record")
        grants = record["grants"]
        if type(grants) is not list or any(type(item) is not str or item not in PLUGIN_PERMISSION_NAMES for item in grants):
            raise LauncherError("Plugin permission grants contain an invalid permission")
        if len(set(grants)) != len(grants):
            raise LauncherError("Plugin permission grants contain duplicate permissions")
        package_hash = record["untrusted_execution_hash"]
        if type(package_hash) is not str or (package_hash and not re.fullmatch(r"[0-9a-f]{64}", package_hash)):
            raise LauncherError("Plugin permission grants contain an invalid local execution hash")
    return value

def _write_permission_state(path: Path, value: dict[str, Any]) -> None:
    _validate_permission_state(value)
    if path.is_symlink():
        raise LauncherError("Plugin permission grants must not be a symbolic link")
    try:
        _atomic_json_write(path, value, private=True)
    except OSError as exc:
        raise LauncherError("Could not save plugin permission grants") from exc

def _parse_v2_manifest_bytes(content: bytes) -> dict[str, Any]:
    if len(content) > PLUGIN_MAX_MANIFEST_BYTES:
        raise LauncherError("plugin.json exceeds the manifest size limit")
    try:
        value = json.loads(
            content.decode("utf-8"),
            object_pairs_hook=_unique_json_object,
            parse_constant=_reject_json_constant,
        )
    except (UnicodeError, json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise LauncherError(f"plugin.json is not valid UTF-8 JSON: {exc}") from exc
    if type(value) is not dict:
        raise LauncherError("plugin.json must contain an object")
    return value

def _read_v2_package(root: Path, *, expected_id: str | None = None) -> dict[str, Any]:
    if root.is_symlink() or not root.is_dir():
        raise LauncherError("Plugin package root must be a real directory")
    if (root / PLUGIN_MANIFEST).exists() or (root / PLUGIN_MANIFEST).is_symlink():
        raise LauncherError("Plugin package cannot mix plugin.yml and plugin.json")
    file_bytes, manifest_bytes = _collect_v2_files(root)
    raw_manifest = _parse_v2_manifest_bytes(manifest_bytes)
    manifest = validate_plugin_json_manifest(raw_manifest)
    if expected_id is not None and manifest["id"] != expected_id:
        raise LauncherError("plugin.json id does not match its plugin directory")
    entrypoint = manifest["entrypoint"]
    if entrypoint not in file_bytes:
        raise LauncherError("Plugin package is missing its entrypoint")

    signed = "signature" in raw_manifest
    verification: dict[str, Any]
    if signed:
        verification = verify_manifest(raw_manifest, file_bytes)
        if not isinstance(verification, dict) or verification.get("signed") is not True or verification.get("valid") is not True:
            reason = verification.get("error") if isinstance(verification, dict) else None
            raise LauncherError(f"Plugin signature or integrity verification failed: {reason or 'invalid package'}")
    else:
        _verify_unsigned_v2_integrity(raw_manifest, file_bytes)
        verification = {"signed": False, "valid": True, "key_id": None, "public_key": None, "fingerprint": None}

    try:
        canonical = json.dumps(
            raw_manifest,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError, UnicodeEncodeError, RecursionError) as exc:
        raise LauncherError("plugin.json cannot be encoded as canonical JSON") from exc
    return {
        "manifest": manifest,
        "files": file_bytes,
        "signed": signed,
        "key_id": verification.get("key_id"),
        "public_key": verification.get("public_key"),
        "fingerprint": verification.get("fingerprint"),
        "package_digest": hashlib.sha256(canonical).hexdigest(),
    }

def _collect_v2_files(root: Path) -> tuple[dict[str, bytes], bytes]:
    manifest_path = root / PLUGIN_V2_MANIFEST
    try:
        manifest_metadata = manifest_path.lstat()
    except OSError as exc:
        raise LauncherError("Plugin package must contain a root plugin.json") from exc
    if not stat.S_ISREG(manifest_metadata.st_mode) or manifest_metadata.st_size > PLUGIN_MAX_MANIFEST_BYTES:
        raise LauncherError("plugin.json must be a bounded regular file")
    manifest_bytes = _read_regular_package_file(manifest_path, PLUGIN_MAX_MANIFEST_BYTES)

    files: dict[str, bytes] = {}
    total_size = len(manifest_bytes)
    file_count = 1
    try:
        for current, directories, filenames in os.walk(root, topdown=True, followlinks=False):
            current_path = Path(current)
            kept_directories: list[str] = []
            for name in sorted(directories):
                child = current_path / name
                mode = child.lstat().st_mode
                if stat.S_ISLNK(mode):
                    raise LauncherError(f"Plugin package contains a symbolic link: {child.relative_to(root)}")
                if not stat.S_ISDIR(mode):
                    raise LauncherError(f"Plugin package contains a non-directory path: {child.relative_to(root)}")
                relative = _v2_package_path(child.relative_to(root).as_posix())
                if len(Path(relative).parts) == 1 and name in _PLUGIN_V2_DATA_DIRECTORIES:
                    continue
                if relative == ".enabled":
                    raise LauncherError("plugin.json package uses a reserved runtime path")
                kept_directories.append(name)
            directories[:] = kept_directories

            for name in sorted(filenames):
                path = current_path / name
                relative = _v2_package_path(path.relative_to(root).as_posix())
                mode = path.lstat().st_mode
                if stat.S_ISLNK(mode):
                    raise LauncherError(f"Plugin package contains a symbolic link: {relative}")
                if not stat.S_ISREG(mode):
                    raise LauncherError(f"Plugin package contains a non-regular file: {relative}")
                if relative == PLUGIN_V2_MANIFEST:
                    continue
                if relative == ".enabled":
                    if path.stat().st_size != 0:
                        raise LauncherError("Plugin enabled marker is malformed")
                    continue
                if relative == PLUGIN_MANIFEST:
                    raise LauncherError("Plugin package cannot mix plugin.yml and plugin.json")
                remaining = PLUGIN_MAX_UNPACKED_BYTES - total_size
                if path.stat().st_size > remaining:
                    raise LauncherError("Plugin package exceeds the unpacked size limit")
                content = _read_regular_package_file(path, remaining)
                total_size += len(content)
                file_count += 1
                if file_count > PLUGIN_MAX_FILES:
                    raise LauncherError("Plugin package contains too many files")
                files[relative] = content
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError(f"Could not inspect plugin package files: {exc}") from exc
    return files, manifest_bytes

def _read_regular_package_file(path: Path, size_limit: int) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as handle:
            metadata = os.fstat(handle.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > size_limit:
                raise LauncherError("Plugin package file is not a bounded regular file")
            content = handle.read(size_limit + 1)
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError("Could not securely read plugin package file") from exc
    if len(content) > size_limit or len(content) != metadata.st_size:
        raise LauncherError("Plugin package file changed while it was being read")
    return content

def _v2_package_path(value: str) -> str:
    if not value or "\\" in value or "\x00" in value or value.startswith("/") or re.match(r"[A-Za-z]:", value):
        raise LauncherError("Plugin package contains an unsafe path")
    parts = value.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        raise LauncherError("Plugin package contains an unsafe path")
    return value

def _reject_v2_runtime_paths(root: Path) -> None:
    for name in {*_PLUGIN_V2_DATA_DIRECTORIES, ".enabled"}:
        child = root / name
        if child.exists() or child.is_symlink():
            raise LauncherError(f"Plugin package uses a reserved runtime path: {name}")

def _verify_unsigned_v2_integrity(manifest: dict[str, Any], file_bytes: dict[str, bytes]) -> None:
    integrity = manifest.get("integrity")
    if type(integrity) is not dict or set(integrity) != {"files"} or type(integrity["files"]) is not dict:
        raise LauncherError("Unsigned plugin.json must include integrity.files")
    expected = integrity["files"]
    if set(expected) != set(file_bytes):
        raise LauncherError("Plugin integrity hashes must exactly cover every package file except plugin.json")
    for path, content in file_bytes.items():
        digest = expected.get(path)
        if type(digest) is not str or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise LauncherError(f"Plugin integrity hash is invalid for {path}")
        if not hmac.compare_digest(digest, hashlib.sha256(content).hexdigest()):
            raise LauncherError(f"Plugin file hash mismatch: {path}")

def _try_v2_manifest(root: Path) -> dict[str, Any] | None:
    path = root / PLUGIN_V2_MANIFEST
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > PLUGIN_MAX_MANIFEST_BYTES:
            return None
        return validate_plugin_json_manifest(_parse_v2_manifest_bytes(_read_regular_package_file(path, PLUGIN_MAX_MANIFEST_BYTES)))
    except (OSError, LauncherError):
        return None

def _runtime_capabilities_for_manifest(manifest: dict[str, Any]) -> list[str]:
    capabilities = set(PLUGIN_RUNTIME_CAPABILITIES)
    permissions = set(manifest.get("permissions", []))
    if manifest.get("api_version") == "3":
        capabilities.add("runtime.manifest.v3")
    if "files:launcher-root-read" in permissions:
        capabilities.add("files.launcherRootReadonly.read")
    if "files:launcher-config-projected" in permissions:
        capabilities.add("config.projectedLauncherRead.schemas")
    if permissions & {"network:proxy-control", "network:tunnel"}:
        capabilities.add("network.proxyPolicies.attach")
    if "network:traffic-observe" in permissions:
        capabilities.add("network.trafficObserve")
    if "network:traffic-intercept" in permissions:
        capabilities.add("network.trafficIntercept.experimental")
    return sorted(capabilities)

def _v2_plugin_status(
    directory: Path,
    trust_store: PluginTrustStore,
    permission_state: dict[str, Any],
) -> dict[str, Any]:
    plugin_id = directory.name
    fallback = {
        "id": plugin_id,
        "name": plugin_id,
        "version": "",
        "author": "",
        "description": "",
        "icon": "plug",
        "category": "Local",
        "api_version": "2",
        "entrypoint": "",
        "permissions": [],
        "network_hosts": [],
    }
    package: dict[str, Any] | None = None
    package_error = ""
    try:
        if not PLUGIN_ID_RE.fullmatch(plugin_id):
            raise LauncherError("Plugin directory has an unsafe id")
        package = _read_v2_package(directory, expected_id=plugin_id)
        manifest = package["manifest"]
    except LauncherError as exc:
        package_error = str(exc)[:500]
        manifest = _try_v2_manifest(directory) or fallback

    signed = bool(package["signed"]) if package else isinstance(manifest.get("signature"), dict)
    signature = manifest.get("signature") if isinstance(manifest.get("signature"), dict) else {}
    key_id = package.get("key_id") if package else signature.get("key_id")
    public_key = package.get("public_key") if package else signature.get("public_key")
    fingerprint = package.get("fingerprint") if package else None
    publisher_status: dict[str, Any] = {"pinned": False, "trusted": False, "revoked": False, "reason": None}
    if signed and isinstance(key_id, str) and isinstance(public_key, str):
        try:
            publisher_status = trust_store.publisher_status(key_id, public_key)
        except LauncherError:
            publisher_status = {"pinned": False, "trusted": False, "revoked": False, "reason": None}
    package_valid = package is not None
    publisher_trusted = package_valid and bool(publisher_status.get("trusted"))
    plugin_revoked = False
    if PLUGIN_ID_RE.fullmatch(plugin_id):
        plugin_revoked = trust_store.is_plugin_revoked(plugin_id)
    revoked = plugin_revoked or bool(publisher_status.get("revoked"))
    record = permission_state["plugins"].get(plugin_id, {"grants": [], "untrusted_execution_hash": ""})
    declared = manifest.get("permissions", []) if package_valid else []
    granted = sorted(set(declared) & set(record["grants"]))
    local_execution_allowed = bool(
        package_valid
        and not signed
        and record["untrusted_execution_hash"] == package["package_digest"]
    )
    enabled = _enabled_state(directory)
    execution_allowed = bool(
        package_valid
        and enabled
        and not revoked
        and (publisher_trusted if signed else local_execution_allowed)
    )
    if not package_valid:
        trust = "community" if signed else "local"
    elif signed:
        trust = "verified" if publisher_trusted else "community"
    else:
        trust = "local"
    return {
        **manifest,
        "trust": trust,
        "installed": True,
        "enabled": enabled,
        "signed": signed,
        "signature_valid": bool(package_valid and signed),
        "publisher_key_id": key_id if isinstance(key_id, str) else None,
        "publisher_public_key": public_key if isinstance(public_key, str) else None,
        "publisher_fingerprint": fingerprint,
        "publisher_pinned": bool(publisher_status.get("pinned")),
        "publisher_revocation_authority": bool(publisher_status.get("revocation_authority")),
        "publisher_trusted": publisher_trusted,
        "package_valid": package_valid,
        "package_error": package_error or None,
        "revoked": revoked,
        "revocation_reason": trust_store.plugin_revocation_reason(plugin_id) if PLUGIN_ID_RE.fullmatch(plugin_id) and plugin_revoked else publisher_status.get("reason"),
        "requested_permissions": list(manifest.get("permissions", [])),
        "granted_permissions": granted,
        "local_untrusted_execution_allowed": local_execution_allowed,
        "execution_allowed": execution_allowed,
        "execution_blocked_reason": _execution_block_reason(package_valid, enabled, revoked, signed, publisher_trusted, local_execution_allowed),
    }

def _execution_block_reason(
    package_valid: bool,
    enabled: bool,
    revoked: bool,
    signed: bool,
    publisher_trusted: bool,
    local_execution_allowed: bool,
) -> str | None:
    if not package_valid:
        return "invalid-package"
    if revoked:
        return "revoked"
    if signed and not publisher_trusted:
        return "publisher-not-trusted"
    if not signed and not local_execution_allowed:
        return "local-untrusted-execution-not-approved"
    if not enabled:
        return "disabled"
    return None

def _unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, item in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = item
    return result

def _reject_json_constant(value: str) -> None:
    raise ValueError(f"non-finite JSON constant: {value}")

def _enabled_state(directory: Path) -> bool:
    path = directory / ".enabled"
    return path.exists() and not path.is_symlink()

def _safe_child(root: Path, relative_path: str) -> Path:
    raw = Path(str(relative_path).replace("\\", "/"))
    if raw.is_absolute() or not raw.parts or any(part in {"", ".", ".."} for part in raw.parts):
        raise LauncherError("Небезопасный путь файла плагина")
    candidate = root
    for part in raw.parts:
        candidate = candidate / part
        if candidate.is_symlink():
            raise LauncherError("Символьные ссылки в файлах плагина запрещены")
    target = candidate.resolve()
    if not _within(target, root.resolve()):
        raise LauncherError("Путь файла плагина выходит за пределы namespace")
    return target

def _archive_member_path(root: Path, member_name: str) -> Path:
    raw = Path(str(member_name).replace("\\", "/"))
    if raw.is_absolute() or not raw.parts or any(part in {"", ".", ".."} for part in raw.parts):
        raise LauncherError("Архив плагина содержит небезопасный путь")
    candidate = root.joinpath(*raw.parts)
    if not _within(candidate.parent.resolve(), root.resolve()):
        raise LauncherError("Архив плагина выходит за пределы staging-каталога")
    return candidate

def _archive_member_is_symlink(member: zipfile.ZipInfo) -> bool:
    return ((member.external_attr >> 16) & 0o170000) == 0o120000

def _assert_no_symlink_chain(path: Path, root: Path) -> None:
    current = path
    root = root.resolve()
    while True:
        if current.is_symlink():
            raise LauncherError("Символьные ссылки в файлах плагина запрещены")
        if current == root:
            return
        if current == current.parent or not _within(current.resolve(), root):
            raise LauncherError("Путь файла плагина выходит за пределы namespace")
        current = current.parent

def _sqlite_authorizer(*, read_only: bool):
    allowed_read = {
        sqlite3.SQLITE_READ,
        sqlite3.SQLITE_SELECT,
        sqlite3.SQLITE_FUNCTION,
        sqlite3.SQLITE_TRANSACTION,
    }
    allowed_write = {
        sqlite3.SQLITE_CREATE_INDEX,
        sqlite3.SQLITE_CREATE_TABLE,
        sqlite3.SQLITE_CREATE_TEMP_INDEX,
        sqlite3.SQLITE_CREATE_TEMP_TABLE,
        sqlite3.SQLITE_CREATE_TEMP_TRIGGER,
        sqlite3.SQLITE_CREATE_TEMP_VIEW,
        sqlite3.SQLITE_CREATE_TRIGGER,
        sqlite3.SQLITE_CREATE_VIEW,
        sqlite3.SQLITE_DELETE,
        sqlite3.SQLITE_DROP_INDEX,
        sqlite3.SQLITE_DROP_TABLE,
        sqlite3.SQLITE_DROP_TEMP_INDEX,
        sqlite3.SQLITE_DROP_TEMP_TABLE,
        sqlite3.SQLITE_DROP_TEMP_TRIGGER,
        sqlite3.SQLITE_DROP_TEMP_VIEW,
        sqlite3.SQLITE_DROP_TRIGGER,
        sqlite3.SQLITE_DROP_VIEW,
        sqlite3.SQLITE_INSERT,
        sqlite3.SQLITE_UPDATE,
    }

    def authorize(action: int, arg1: str | None, arg2: str | None, _db: str | None, _source: str | None) -> int:
        if action in {sqlite3.SQLITE_ATTACH, sqlite3.SQLITE_DETACH, sqlite3.SQLITE_PRAGMA}:
            return sqlite3.SQLITE_DENY
        if action in allowed_read or (not read_only and action in allowed_write):
            return sqlite3.SQLITE_OK
        return sqlite3.SQLITE_DENY

    return authorize

def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False

def _next_is_list(lines: list[str], index: int) -> bool:
    for raw in lines[index:]:
        value = _strip_yaml_comment(raw).strip()
        if not value:
            continue
        return value.startswith("-")
    return False

def _strip_yaml_comment(value: str) -> str:
    quote = ""
    for index, char in enumerate(value):
        if char in {"'", '"'}:
            quote = "" if quote == char else char if not quote else quote
        elif char == "#" and not quote and (index == 0 or value[index - 1].isspace()):
            return value[:index]
    return value

def _yaml_scalar(value: str) -> Any:
    if value.startswith("!") or value.startswith("&") or value.startswith("*") or value in {"|", ">"}:
        raise LauncherError("plugin.yml не поддерживает YAML tags, anchors и blocks")
    if value.startswith('"') and value.endswith('"'):
        try:
            return json.loads(value)
        except ValueError as exc:
            raise LauncherError("Некорректная строка plugin.yml") from exc
    if value.startswith("'") and value.endswith("'"):
        return value[1:-1].replace("''", "'")
    lowered = value.lower()
    if lowered in {"true", "yes"}:
        return True
    if lowered in {"false", "no"}:
        return False
    if lowered in {"null", "~"}:
        return None
    if re.fullmatch(r"-?\d+", value):
        return int(value)
    if re.fullmatch(r"-?(?:\d+\.\d*|\d*\.\d+)", value):
        return float(value)
    if value.startswith("[") and value.endswith("]"):
        try:
            parsed = json.loads(value)
        except ValueError as exc:
            raise LauncherError("Списки plugin.yml должны быть JSON-подобными") from exc
        if not isinstance(parsed, list):
            raise LauncherError("Некорректный список plugin.yml")
        return parsed
    return value

def _validate_config_key(key: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,120}", str(key).strip()):
        raise LauncherError("Небезопасный ключ конфигурации плагина")

def _dump_yaml(value: dict[str, Any]) -> str:
    lines: list[str] = []
    for key, item in value.items():
        _validate_config_key(str(key))
        _dump_yaml_value(lines, str(key), item, 0)
    return "\n".join(lines) + ("\n" if lines else "")

def _dump_yaml_value(lines: list[str], key: str, value: Any, indent: int) -> None:
    prefix = " " * indent
    if isinstance(value, dict):
        lines.append(f"{prefix}{key}:")
        for child_key, child_value in value.items():
            _validate_config_key(str(child_key))
            _dump_yaml_value(lines, str(child_key), child_value, indent + 2)
        return
    if isinstance(value, list):
        lines.append(f"{prefix}{key}:")
        for item in value:
            lines.append(f"{' ' * (indent + 2)}- {_yaml_dump_scalar(item)}")
        return
    lines.append(f"{prefix}{key}: {_yaml_dump_scalar(value)}")

def _yaml_dump_scalar(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return str(value)
    return json.dumps(str(value), ensure_ascii=False)
