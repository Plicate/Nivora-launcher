from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import re
import secrets
import shutil
import subprocess
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath
from typing import Any, BinaryIO, Iterable
from urllib.error import HTTPError, URLError
from urllib.parse import quote, unquote, urljoin, urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

try:
    from defusedxml import ElementTree
except ImportError:  # pragma: no cover - the package is a release dependency
    from xml.etree import ElementTree

from .config import ConfigStore, _atomic_json_write
from .errors import LauncherError
from .platform_support import set_private_fd
from .security import CELL_DIRECTORY, SERVICE_JSON_FILES


class CloudStorageError(LauncherError):
    """A user-facing cloud storage error."""


@dataclass(frozen=True)
class CloudFile:
    key: str
    size: int
    modified_at: float

    def as_dict(self) -> dict[str, Any]:
        return {"key": self.key, "size": self.size, "modified_at": self.modified_at}


_DAV = "{DAV:}"
_MANIFEST_NAME = ".trial-launcher-manifest.json"
_MANIFEST_KEY_FILE = "cloud-manifest.key"
_MAX_REMOTE_KEY = 2048
_MAX_MANIFEST_BYTES = 8 * 1024 * 1024
_MAX_SETTINGS_BYTES = 1 * 1024 * 1024
_MAX_FILE_BYTES = 512 * 1024 * 1024
_MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
_MAX_REMOTE_ENTRIES = 10_000
_EXCLUDED_TREE_NAMES = {"logs", ".trial-cell", "plugins"}
_SERVER_METADATA_NAMES = {
    ".trial-launcher-server.json",
    ".trial-launcher-live.json",
    ".trial-launcher-metrics.json",
}
_CONTROL_FILE_NAMES = frozenset(SERVICE_JSON_FILES) | {_MANIFEST_NAME}
_EXCLUDED_FILE_NAMES = _CONTROL_FILE_NAMES | {"accounts.json", _MANIFEST_KEY_FILE}

# Settings which can affect only the local UI or resource cache. In particular,
# paths, credentials, OAuth endpoints and catalog definitions never come from DAV.
_RESTORABLE_SETTINGS = frozenset(
    {
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
        "keep_logs_days",
        "telemetry",
        "keyboard_shortcuts",
        "graphics_mode",
        "theme",
        "active_theme_id",
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
        "density",
        "radius_tokens",
        "page_themes",
        "page_layouts",
        "widget_visibility",
        "color_tokens",
        "instance_groups",
        "pinned_instance_groups",
    }
)


class _SameOriginRedirectHandler(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        target = urljoin(request.full_url, newurl)
        source = urlsplit(request.full_url)
        destination = urlsplit(target)
        source_origin = (source.scheme.lower(), source.hostname, source.port or 443)
        destination_origin = (destination.scheme.lower(), destination.hostname, destination.port or 443)
        if source_origin != destination_origin:
            raise CloudStorageError("WebDAV перенаправил запрос на другой сервер")
        if destination.scheme.lower() != "https":
            raise CloudStorageError("WebDAV перенаправил запрос на небезопасный адрес")
        return super().redirect_request(request, fp, code, msg, headers, target)


def _safe_key(value: str) -> str:
    key = str(value).replace("\\", "/").strip("/")
    path = PurePosixPath(key)
    if not key or len(key) > _MAX_REMOTE_KEY or "\x00" in key or path.is_absolute() or ".." in path.parts:
        raise CloudStorageError("Недопустимый путь файла в облачном хранилище")
    return "/".join(part for part in path.parts if part not in {"", "."})


def _safe_remote_path(value: str) -> str:
    raw = str(value or "trial-launcher").strip().replace("\\", "/")
    parts = [part for part in raw.split("/") if part and part != "."]
    if any(part == ".." or "\x00" in part for part in parts):
        raise CloudStorageError("Путь облачной папки содержит недопустимые сегменты")
    return "/".join(parts)[:512]


def _is_protected_local_path(config: ConfigStore, path: Path) -> bool:
    try:
        resolved = path.resolve()
        protected_files = {
            config.paths.settings_file.resolve(),
            config.paths.accounts_file.resolve(),
            (config.paths.config_home / _MANIFEST_KEY_FILE).resolve(),
        }
        if resolved in protected_files:
            return True
        security_dir = config.paths.instance_security_dir.resolve()
        return resolved == security_dir or security_dir in resolved.parents
    except OSError:
        return True


def _format_http_error(error: HTTPError) -> str:
    if error.code in {401, 403}:
        return "WebDAV отклонил авторизацию. Проверьте логин и пароль или токен."
    if error.code == 404:
        return "WebDAV endpoint или удалённая папка не найдены."
    if error.code == 405:
        return "WebDAV-сервис не поддерживает нужную операцию."
    return f"WebDAV вернул HTTP {error.code}."


class WebDavStorage:
    def __init__(self, config: ConfigStore) -> None:
        self.config = config
        self.url = self._validated_url(config.settings.cloud_url)
        self.username = str(config.settings.cloud_username).strip()
        self.remote_path = _safe_remote_path(config.settings.cloud_remote_path)
        self.password = self._load_password()
        if not self.url or not self.username or not self.password:
            raise CloudStorageError("Сначала настройте WebDAV endpoint, логин и пароль или токен")
        self.root_url = self._root_url()

    @staticmethod
    def _validated_url(value: str) -> str:
        url = str(value or "").strip().rstrip("/")
        parsed = urlsplit(url)
        if not url:
            return ""
        if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
            raise CloudStorageError("WebDAV endpoint должен иметь вид https://host/remote.php/dav/files/user")
        return urlunsplit((parsed.scheme, parsed.netloc, parsed.path.rstrip("/"), "", ""))

    def _root_url(self) -> str:
        suffix = "/".join(quote(part, safe="") for part in self.remote_path.split("/"))
        return f"{self.url}/{suffix}/" if suffix else f"{self.url}/"

    def _load_password(self) -> str:
        return _secret_get(self.config.settings.cloud_url, self.username)

    def _headers(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        token = base64.b64encode(f"{self.username}:{self.password}".encode("utf-8")).decode("ascii")
        headers = {"Authorization": f"Basic {token}", "User-Agent": "TrialLauncher/0.1"}
        if extra:
            headers.update(extra)
        return headers

    def _request(
        self,
        method: str,
        url: str,
        data: bytes | BinaryIO | None = None,
        headers: dict[str, str] | None = None,
        accepted: set[int] | None = None,
        max_bytes: int = _MAX_FILE_BYTES,
    ) -> bytes:
        request_headers = self._headers(headers)
        if isinstance(data, bytes):
            request_headers.setdefault("Content-Length", str(len(data)))
        request = Request(url, data=data, headers=request_headers, method=method)
        try:
            opener = build_opener(ProxyHandler({}), _SameOriginRedirectHandler())
            with opener.open(request, timeout=max(5, int(self.config.settings.download_timeout_seconds))) as response:
                content_length = response.headers.get("Content-Length")
                try:
                    declared_length = int(content_length) if content_length else None
                except ValueError:
                    declared_length = None
                if declared_length is not None and declared_length > max_bytes:
                    raise CloudStorageError("Ответ WebDAV слишком большой")
                body = response.read(max_bytes + 1)
                if len(body) > max_bytes:
                    raise CloudStorageError("Ответ WebDAV слишком большой")
                return body
        except HTTPError as exc:
            if accepted and exc.code in accepted:
                return b""
            raise CloudStorageError(_format_http_error(exc)) from exc
        except (OSError, URLError, TimeoutError) as exc:
            raise CloudStorageError(f"Не удалось подключиться к WebDAV: {exc}") from exc

    def ensure_root(self) -> None:
        self._make_collection(self.root_url)

    def _make_collection(self, url: str) -> None:
        parts = urlsplit(url).path.rstrip("/").split("/")
        base = urlunsplit((urlsplit(url).scheme, urlsplit(url).netloc, "", "", ""))
        path = ""
        for part in parts:
            if not part:
                continue
            path += f"/{part}"
            collection = f"{base}{path}/"
            self._request("MKCOL", collection, accepted={405, 409})

    def _file_url(self, key: str) -> str:
        safe = _safe_key(key)
        encoded = "/".join(quote(part, safe="") for part in safe.split("/"))
        return urljoin(self.root_url, encoded)

    def put(self, key: str, content: bytes) -> None:
        safe = _safe_key(key)
        parent = "/".join(safe.split("/")[:-1])
        if parent:
            self._make_collection(urljoin(self.root_url, "/".join(quote(part, safe="") for part in parent.split("/")) + "/"))
        self._request("PUT", self._file_url(safe), data=content, headers={"Content-Type": "application/octet-stream"})

    def put_file(self, key: str, path: Path) -> None:
        safe = _safe_key(key)
        size = path.stat().st_size
        if size > _MAX_FILE_BYTES:
            raise CloudStorageError(f"Файл слишком большой для синхронизации: {safe}")
        parent = "/".join(safe.split("/")[:-1])
        if parent:
            self._make_collection(urljoin(self.root_url, "/".join(quote(part, safe="") for part in parent.split("/")) + "/"))
        with path.open("rb") as handle:
            self._request(
                "PUT",
                self._file_url(safe),
                data=handle,
                headers={"Content-Type": "application/octet-stream", "Content-Length": str(size)},
            )

    def get(self, key: str) -> bytes:
        return self._request("GET", self._file_url(key), max_bytes=_MAX_FILE_BYTES)

    def delete(self, key: str) -> None:
        self._request("DELETE", self._file_url(key), accepted={404})

    def list_files(self) -> list[CloudFile]:
        self.ensure_root()
        base_path = unquote(urlsplit(self.root_url).path).rstrip("/") + "/"
        values: list[CloudFile] = []
        pending = [self.root_url]
        visited: set[str] = set()
        while pending:
            collection_url = pending.pop()
            if collection_url in visited:
                continue
            visited.add(collection_url)
            if len(visited) > _MAX_REMOTE_ENTRIES:
                raise CloudStorageError("В WebDAV слишком много каталогов")
            body = self._request(
                "PROPFIND",
                collection_url,
                headers={"Depth": "1", "Content-Type": "application/xml"},
                max_bytes=_MAX_MANIFEST_BYTES,
            )
            if b"<!DOCTYPE" in body.upper() or b"<!ENTITY" in body.upper():
                raise CloudStorageError("WebDAV вернул XML с запрещёнными декларациями")
            try:
                root = ElementTree.fromstring(body)
            except ElementTree.ParseError as exc:
                raise CloudStorageError("WebDAV вернул некорректный список файлов") from exc
            for response in root.findall(f"{_DAV}response"):
                href = response.findtext(f"{_DAV}href")
                if not href:
                    continue
                absolute = urljoin(collection_url, href)
                parsed_absolute = urlsplit(absolute)
                root_parts = (parsed_absolute.scheme.lower(), parsed_absolute.hostname, parsed_absolute.port or 443)
                expected_parts = urlsplit(self.root_url)
                expected_origin = (expected_parts.scheme.lower(), expected_parts.hostname, expected_parts.port or 443)
                if root_parts != expected_origin:
                    continue
                remote_path = unquote(parsed_absolute.path)
                if not remote_path.startswith(base_path):
                    continue
                key = remote_path[len(base_path) :].strip("/")
                if not key:
                    continue
                prop = response.find(f"{_DAV}propstat/{_DAV}prop")
                if prop is None:
                    continue
                is_collection = prop.find(f"{_DAV}resourcetype/{_DAV}collection") is not None
                if is_collection:
                    pending.append(absolute.rstrip("/") + "/")
                    continue
                if len(values) >= _MAX_REMOTE_ENTRIES:
                    raise CloudStorageError("В WebDAV слишком много файлов")
                try:
                    size = int(prop.findtext(f"{_DAV}getcontentlength") or 0)
                except ValueError:
                    size = 0
                modified = prop.findtext(f"{_DAV}getlastmodified") or ""
                modified_at = 0.0
                if modified:
                    try:
                        from email.utils import parsedate_to_datetime

                        modified_at = parsedate_to_datetime(modified).timestamp()
                    except (TypeError, ValueError, OverflowError):
                        modified_at = 0.0
                try:
                    values.append(CloudFile(_safe_key(key), size, modified_at))
                except CloudStorageError:
                    continue
        return sorted(values, key=lambda item: item.key.lower())


def save_cloud_password(config: ConfigStore, password: str) -> None:
    value = str(password)
    if not value:
        _secret_clear(config.settings.cloud_url, config.settings.cloud_username)
        return
    _secret_set(config.settings.cloud_url, config.settings.cloud_username, value)


def cloud_has_password(config: ConfigStore) -> bool:
    try:
        return bool(_secret_get(config.settings.cloud_url, config.settings.cloud_username, soft=True))
    except CloudStorageError:
        return False


def cloud_settings_public(config: ConfigStore) -> dict[str, Any]:
    settings = config.settings
    return {
        "provider": "webdav",
        "configured": bool(settings.cloud_url and settings.cloud_username and cloud_has_password(config)),
        "endpoint": settings.cloud_url,
        "username": settings.cloud_username,
        "remote_path": settings.cloud_remote_path,
        "include_accounts": settings.cloud_include_accounts,
        "last_sync": settings.cloud_last_sync,
    }


def sync_sources(config: ConfigStore, include_accounts: bool) -> Iterable[tuple[str, Path]]:
    roots: list[tuple[str, Path]] = [("data", config.paths.data_home)]
    game_dir = config.game_dir.resolve()
    data_home = config.paths.data_home.resolve()
    if not game_dir.is_relative_to(data_home):
        roots.append(("game", game_dir))
    roots.append(("config/settings.json", config.paths.settings_file))
    for prefix, root in roots:
        if root.is_symlink() or not root.exists():
            continue
        if root.is_file():
            if root.name.endswith(".part"):
                continue
            yield prefix, root
            continue
        for path in sorted(root.rglob("*")):
            if path.is_symlink() or not path.is_file() or path.name.endswith(".part"):
                continue
            try:
                relative = path.relative_to(root)
            except ValueError:
                continue
            if _is_protected_local_path(config, path):
                continue
            if relative.parts and any(part in _EXCLUDED_TREE_NAMES for part in relative.parts):
                continue
            if path.name in _SERVER_METADATA_NAMES or path.name in _EXCLUDED_FILE_NAMES:
                continue
            yield f"{prefix}/{relative.as_posix()}", path


def _local_target(config: ConfigStore, key: str, include_accounts: bool) -> Path | None:
    safe = _safe_key(key)
    if safe == "config/settings.json":
        return config.paths.settings_file
    # Account tokens are never restored from DAV, even when the upload opt-in is set.
    if safe == "config/accounts.json":
        return None
    if safe.startswith("data/"):
        relative = safe.removeprefix("data/")
        parts = PurePosixPath(relative).parts
        if any(part in _EXCLUDED_TREE_NAMES for part in parts) or any(part in _EXCLUDED_FILE_NAMES for part in parts):
            return None
        target = config.paths.data_home / relative
        return None if _is_protected_local_path(config, target) else target
    if safe.startswith("game/"):
        relative = safe.removeprefix("game/")
        parts = PurePosixPath(relative).parts
        if (
            any(part in _EXCLUDED_TREE_NAMES for part in parts)
            or any(part in _SERVER_METADATA_NAMES for part in parts)
            or any(part in _EXCLUDED_FILE_NAMES for part in parts)
        ):
            return None
        target = config.game_dir / relative
        return None if _is_protected_local_path(config, target) else target
    return None


def _atomic_file_write(path: Path, content: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp_path = Path(temp_name)
    try:
        set_private_fd(fd)
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        temp_path.replace(path)
    finally:
        if temp_path.exists():
            temp_path.unlink()


def _secret_command() -> str:
    command = shutil.which("secret-tool")
    if not command:
        raise CloudStorageError("Для облачного пароля установите libsecret и secret-tool")
    return command


def _secret_attributes(url: str, username: str) -> list[str]:
    return ["trial-launcher-webdav", "user", username, "url", url]


def _secret_get(url: str, username: str, *, soft: bool = False) -> str:
    if not url or not username:
        return ""
    if os.name == "nt":
        try:
            password = _windows_keyring().get_password("trial-launcher-webdav", _webdav_keyring_username(url, username))
        except Exception as exc:
            if soft:
                return ""
            raise CloudStorageError("Не удалось прочитать пароль WebDAV из диспетчера учётных данных Windows") from exc
        if password is None and not soft:
            raise CloudStorageError("Пароль WebDAV не найден в диспетчере учётных данных Windows")
        return password or ""
    try:
        result = subprocess.run(
            [_secret_command(), "lookup", *_secret_attributes(url, username)],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        if soft:
            return ""
        raise CloudStorageError("Не удалось прочитать пароль WebDAV из libsecret") from exc
    if result.returncode != 0:
        if soft:
            return ""
        raise CloudStorageError("Пароль WebDAV не найден в libsecret")
    return result.stdout.rstrip("\r\n")


def _secret_set(url: str, username: str, password: str) -> None:
    if os.name == "nt":
        try:
            _windows_keyring().set_password("trial-launcher-webdav", _webdav_keyring_username(url, username), password)
        except Exception as exc:
            raise CloudStorageError("Не удалось сохранить пароль WebDAV в диспетчере учётных данных Windows") from exc
        return
    try:
        result = subprocess.run(
            [_secret_command(), "store", "--label", "Nivora Launcher WebDAV", *_secret_attributes(url, username)],
            input=password,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise CloudStorageError("Не удалось сохранить пароль WebDAV в libsecret") from exc
    if result.returncode != 0:
        raise CloudStorageError("libsecret не сохранил пароль WebDAV")


def _secret_clear(url: str, username: str) -> None:
    if not url or not username:
        return
    if os.name == "nt":
        try:
            keyring = _windows_keyring()
            identity = _webdav_keyring_username(url, username)
            if keyring.get_password("trial-launcher-webdav", identity) is not None:
                keyring.delete_password("trial-launcher-webdav", identity)
        except Exception as exc:
            raise CloudStorageError("Не удалось удалить пароль WebDAV из диспетчера учётных данных Windows") from exc
        return
    try:
        result = subprocess.run(
            [_secret_command(), "clear", *_secret_attributes(url, username)],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise CloudStorageError("Не удалось удалить пароль WebDAV из libsecret") from exc
    if result.returncode not in {0, 1}:
        raise CloudStorageError("libsecret не удалил пароль WebDAV")


def _windows_keyring():
    try:
        import keyring
        if keyring.get_keyring().priority <= 0:
            raise RuntimeError("no active keyring backend")
        return keyring
    except Exception as exc:
        raise CloudStorageError("Диспетчер учётных данных Windows недоступен") from exc


def _webdav_keyring_username(url: str, username: str) -> str:
    return f"{url}\n{username}"


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _bytes_sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def _filtered_settings_bytes(config: ConfigStore) -> bytes:
    values = asdict(config.settings)
    filtered = {key: values[key] for key in sorted(_RESTORABLE_SETTINGS) if key in values}
    content = json.dumps(filtered, ensure_ascii=False, sort_keys=True, indent=2).encode("utf-8")
    if len(content) > _MAX_SETTINGS_BYTES:
        raise CloudStorageError("Отфильтрованные настройки слишком большие для синхронизации")
    return content


def _manifest_key(config: ConfigStore) -> bytes:
    path = config.paths.config_home / _MANIFEST_KEY_FILE
    if path.is_symlink():
        raise CloudStorageError("Ключ проверки облачного манифеста является ссылкой")
    try:
        if path.exists():
            value = path.read_bytes()
            if len(value) == 32:
                return value
            raise CloudStorageError("Ключ проверки облачного манифеста повреждён")
        value = secrets.token_bytes(32)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(value)
        return value
    except FileExistsError:
        return _manifest_key(config)
    except OSError as exc:
        raise CloudStorageError("Не удалось сохранить ключ проверки облачного манифеста") from exc


def _manifest_payload(files: list[dict[str, Any]]) -> bytes:
    normalized = [
        {
            "key": str(item.get("key", "")),
            "size": int(item.get("size", 0)),
            "modified_at": float(item.get("modified_at", 0) or 0),
            "sha256": str(item.get("sha256", "")).lower(),
        }
        for item in files
    ]
    return json.dumps({"format": 2, "files": normalized}, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _manifest_signature(config: ConfigStore, files: list[dict[str, Any]]) -> str:
    return hmac.new(_manifest_key(config), _manifest_payload(files), hashlib.sha256).hexdigest()


def _load_manifest(storage: WebDavStorage, config: ConfigStore) -> tuple[dict[str, Any], str]:
    try:
        raw = json.loads(storage._request("GET", storage._file_url(_MANIFEST_NAME), max_bytes=_MAX_MANIFEST_BYTES).decode("utf-8"))
    except (UnicodeDecodeError, ValueError, CloudStorageError) as exc:
        raise CloudStorageError("Облачный манифест отсутствует или повреждён") from exc
    if not isinstance(raw, dict) or raw.get("format") != 2 or not isinstance(raw.get("files"), list) or not isinstance(raw.get("signature"), str):
        raise CloudStorageError("Облачный манифест имеет неподдерживаемый формат")
    files: list[dict[str, Any]] = []
    total = 0
    seen_keys: set[str] = set()
    for item in raw["files"]:
        if not isinstance(item, dict):
            raise CloudStorageError("Облачный манифест содержит некорректный файл")
        try:
            key = _safe_key(str(item["key"]))
            size = int(item["size"])
            digest = str(item["sha256"]).lower()
        except (KeyError, TypeError, ValueError) as exc:
            raise CloudStorageError("Облачный манифест содержит некорректную запись") from exc
        if size < 0 or size > _MAX_FILE_BYTES or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise CloudStorageError("Облачный манифест содержит небезопасный размер или хеш")
        if key in seen_keys:
            raise CloudStorageError("Облачный манифест содержит повторяющийся путь")
        seen_keys.add(key)
        total += size
        if total > _MAX_TOTAL_BYTES:
            raise CloudStorageError("Облачный бэкап превышает общий лимит размера")
        try:
            modified_at = float(item.get("modified_at", 0) or 0)
        except (TypeError, ValueError):
            modified_at = 0.0
        files.append({"key": key, "size": size, "modified_at": modified_at, "sha256": digest})
    expected = _manifest_signature(config, files)
    if not hmac.compare_digest(expected, raw["signature"]):
        raise CloudStorageError("Подпись облачного манифеста не совпадает с локальным ключом")
    payload_digest = hashlib.sha256(_manifest_payload(files)).hexdigest()
    return {"format": 2, "updated_at": float(raw.get("updated_at", 0) or 0), "files": files, "signature": raw["signature"]}, payload_digest


def _restore_plan(config: ConfigStore, storage: WebDavStorage) -> tuple[dict[str, Any], str]:
    manifest, manifest_id = _load_manifest(storage, config)
    changes: list[dict[str, Any]] = []
    settings_preview: dict[str, Any] | None = None
    skipped = 0
    for item in manifest["files"]:
        key = item["key"]
        target = _local_target(config, key, False)
        if target is None:
            skipped += 1
            continue
        if target == config.paths.settings_file:
            if item["size"] > _MAX_SETTINGS_BYTES:
                raise CloudStorageError("Удалённый settings.json слишком большой")
            content = storage.get(key)
            try:
                remote_settings = json.loads(content.decode("utf-8"))
            except (UnicodeDecodeError, ValueError) as exc:
                raise CloudStorageError("Удалённые настройки имеют некорректный формат") from exc
            if not isinstance(remote_settings, dict):
                raise CloudStorageError("Удалённые настройки имеют неожиданный формат")
            settings_preview = {key: remote_settings[key] for key in _RESTORABLE_SETTINGS if key in remote_settings}
            changes.append({"key": key, "action": "merge-settings", "keys": sorted(settings_preview)})
            continue
        _reject_symlink_target(target)
        changes.append({"key": key, "action": "replace" if target.exists() else "create", "size": item["size"]})
    return {
        "preview": True,
        "manifest_id": manifest_id,
        "changes": changes,
        "skipped": skipped,
        "files": len(changes),
        "bytes": sum(int(item.get("size", 0)) for item in manifest["files"] if _local_target(config, item["key"], False) is not None),
        "settings": settings_preview or {},
    }, manifest_id


def _reject_symlink_target(path: Path) -> None:
    current = path
    while current != current.parent:
        if current.is_symlink():
            raise CloudStorageError(f"Локальный путь восстановления является ссылкой: {path}")
        current = current.parent


def sync_cloud(config: ConfigStore) -> dict[str, Any]:
    storage = WebDavStorage(config)
    storage.ensure_root()
    uploaded = 0
    total_bytes = 0
    manifest: list[dict[str, Any]] = []
    sources = [
        (key, path)
        for key, path in sync_sources(config, config.settings.cloud_include_accounts)
        if key != "config/settings.json"
    ]
    settings_path = config.paths.settings_file
    settings_content = _filtered_settings_bytes(config) if settings_path.exists() else b""
    if len(sources) + bool(settings_content) > _MAX_REMOTE_ENTRIES:
        raise CloudStorageError("В облачный бэкап попадает слишком много файлов")
    planned_total = sum(path.stat().st_size for _, path in sources if path.exists()) + len(settings_content)
    if planned_total > _MAX_TOTAL_BYTES:
        raise CloudStorageError("Облачный бэкап превышает общий лимит размера")
    for key, path in sources:
        stat = path.stat()
        if stat.st_size > _MAX_FILE_BYTES:
            raise CloudStorageError(f"Файл слишком большой для синхронизации: {key}")
        storage.put_file(key, path)
        digest = _file_sha256(path)
        manifest.append({"key": key, "size": stat.st_size, "modified_at": stat.st_mtime, "sha256": digest})
        uploaded += 1
        total_bytes += stat.st_size
    if settings_content:
        settings_stat = settings_path.stat()
        storage.put("config/settings.json", settings_content)
        manifest.append(
            {
                "key": "config/settings.json",
                "size": len(settings_content),
                "modified_at": settings_stat.st_mtime,
                "sha256": _bytes_sha256(settings_content),
            }
        )
        uploaded += 1
        total_bytes += len(settings_content)
    manifest_data = {
        "format": 2,
        "updated_at": time.time(),
        "files": manifest,
        "signature": _manifest_signature(config, manifest),
    }
    manifest_content = json.dumps(manifest_data, ensure_ascii=False, indent=2).encode("utf-8")
    if len(manifest_content) > _MAX_MANIFEST_BYTES:
        raise CloudStorageError("Облачный манифест слишком большой")
    storage.put(_MANIFEST_NAME, manifest_content)
    config.set_value("cloud_last_sync", time.time())
    return {"uploaded": uploaded, "bytes": total_bytes}


def restore_cloud(config: ConfigStore, *, confirmed: bool = False, manifest_id: str = "") -> dict[str, Any]:
    storage = WebDavStorage(config)
    plan, current_manifest_id = _restore_plan(config, storage)
    if not confirmed:
        return plan
    if not manifest_id or not hmac.compare_digest(manifest_id, current_manifest_id):
        raise CloudStorageError("Список облачного восстановления изменился. Получите новый preview и подтвердите его снова.")
    restored = 0
    total_bytes = 0
    restored_at = time.time()
    manifest, restored_manifest_id = _load_manifest(storage, config)
    if not hmac.compare_digest(restored_manifest_id, current_manifest_id):
        raise CloudStorageError("Список облачного восстановления изменился. Получите новый preview и подтвердите его снова.")
    remote_settings: dict[str, Any] = {}
    for item in manifest["files"]:
        key = item["key"]
        target = _local_target(config, key, False)
        if target is None:
            continue
        if target == config.paths.settings_file and item["size"] > _MAX_SETTINGS_BYTES:
            raise CloudStorageError("Удалённый settings.json слишком большой")
        content = storage.get(key)
        if len(content) != item["size"] or hashlib.sha256(content).hexdigest() != item["sha256"]:
            raise CloudStorageError(f"Проверка целостности облачного файла не пройдена: {key}")
        if target == config.paths.settings_file:
            try:
                settings = json.loads(content.decode("utf-8"))
            except (UnicodeDecodeError, ValueError) as exc:
                raise CloudStorageError("Удалённый settings.json имеет некорректный формат") from exc
            if not isinstance(settings, dict):
                raise CloudStorageError("Удалённый settings.json имеет неожиданный формат")
            remote_settings.update({key: settings[key] for key in _RESTORABLE_SETTINGS if key in settings})
            restored += 1
            continue
        _reject_symlink_target(target)
        _atomic_file_write(target, content)
        restored += 1
        total_bytes += len(content)
    if remote_settings:
        config.set_values(remote_settings)
    config.settings.cloud_last_sync = restored_at
    config.save_settings()
    return {"preview": False, "restored": restored, "bytes": total_bytes, "manifest_id": current_manifest_id, "skipped": plan["skipped"]}


def delete_cloud_file(config: ConfigStore, key: str) -> None:
    if _safe_key(key) == _MANIFEST_NAME:
        raise CloudStorageError("Служебный manifest нельзя удалить")
    WebDavStorage(config).delete(key)
