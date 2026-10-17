from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import stat
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator

from .config import ConfigStore
from .errors import LauncherError
from .instances import InstanceInfo, InstanceStore
from .platform_support import is_reparse_point, process_exists
from .portable_fs import atomic_write as atomic_write_path
from .portable_fs import regular_file, remove_tree as remove_tree_path, safe_path
from .process_guard import InstanceProcessLock


MAX_INSTANCE_TEXT_BYTES = 2 * 1024 * 1024
MAX_INSTANCE_PREVIEW_BYTES = 8 * 1024 * 1024
MAX_INSTANCE_UPLOAD_BYTES = 32 * 1024 * 1024
MIN_INSTANCE_LOG_READ_BYTES = 4
MAX_INSTANCE_LOG_READ_BYTES = 131_072
MAX_INSTANCE_LIVE_LOG_BYTES = 2 * 1024 * 1024
MAX_INSTANCE_LOG_LIST_LIMIT = 100
_MAX_LIST_ENTRIES = 1000
_MAX_DELETE_ENTRIES = 100_000
_MAX_DELETE_DEPTH = 128
_DIRECTORY_OPEN_FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
_NOFOLLOW = getattr(os, "O_NOFOLLOW", 0)
_NONBLOCK = getattr(os, "O_NONBLOCK", 0)
_LOG_ID_RE = re.compile(r"[A-Za-z0-9._-]{1,240}\.log\Z")
_LOG_VERSION_TAIL_RE = re.compile(r"[A-Za-z0-9._-]+-\d{1,20}-[0-9a-f]{10}\.log\Z")
_LOG_SUFFIX_RE = re.compile(r"\d{1,20}-[0-9a-f]{10}\.log\Z")
_IMAGE_MIMES = (
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    (b"\xff\xd8\xff", "image/jpeg"),
)


def instance_log_hash(name: str) -> str:
    return hashlib.sha256(name.encode("utf-8")).hexdigest()[:24]


class InstanceFiles:
    """No-follow file access rooted at a single client profile."""

    def __init__(self, config: ConfigStore, instances: InstanceStore, bedrock_profiles: Any = None) -> None:
        self.config = config
        self.instances = instances
        self.bedrock_profiles = bedrock_profiles

    def list_files(self, name: str, directory: str = "") -> list[dict[str, Any]]:
        info = self._file_instance(name)
        root = self._root_path(info)
        parts = _path_parts(directory, allow_root=True)
        values: list[dict[str, Any]] = []
        if os.name == "nt":
            target = safe_path(root, parts)
            if not target.is_dir():
                raise LauncherError("Папка экземпляра не найдена")
            for item in target.iterdir():
                if _reserved_name(item.name) or is_reparse_point(item):
                    continue
                try:
                    item_stat = item.stat()
                except OSError:
                    continue
                is_file = stat.S_ISREG(item_stat.st_mode)
                is_directory = stat.S_ISDIR(item_stat.st_mode)
                if not is_file and not is_directory:
                    continue
                relative = "/".join((*parts, item.name))
                values.append(
                    {
                        "name": item.name,
                        "path": relative,
                        "kind": "file" if is_file else "directory",
                        "size": item_stat.st_size if is_file else 0,
                        "modified_at": item_stat.st_mtime,
                        "editable": is_file and item_stat.st_size <= MAX_INSTANCE_TEXT_BYTES,
                    }
                )
            values.sort(key=lambda value: (value["kind"] != "directory", value["name"].casefold()))
            return values[:_MAX_LIST_ENTRIES]
        try:
            with _open_directory(root, parts) as directory_fd:
                for item in os.scandir(directory_fd):
                    if _reserved_name(item.name):
                        continue
                    try:
                        item_stat = item.stat(follow_symlinks=False)
                    except OSError:
                        continue
                    is_file = stat.S_ISREG(item_stat.st_mode)
                    is_directory = stat.S_ISDIR(item_stat.st_mode)
                    if not is_file and not is_directory:
                        continue
                    relative = "/".join((*parts, item.name))
                    values.append(
                        {
                            "name": item.name,
                            "path": relative,
                            "kind": "file" if is_file else "directory",
                            "size": item_stat.st_size if is_file else 0,
                            "modified_at": item_stat.st_mtime,
                            "editable": is_file and item_stat.st_size <= MAX_INSTANCE_TEXT_BYTES,
                        }
                    )
        except OSError as exc:
            raise LauncherError("Не удалось прочитать папку экземпляра") from exc
        values.sort(key=lambda value: (value["kind"] != "directory", value["name"].casefold()))
        return values[:_MAX_LIST_ENTRIES]

    def read_file(self, name: str, path: str, encoding: str = "text") -> dict[str, str]:
        if not isinstance(encoding, str) or encoding not in {"text", "base64"}:
            raise LauncherError("Кодировка должна быть text или base64")
        info = self._file_instance(name)
        root = self._root_path(info)
        parts = _path_parts(path)
        limit = MAX_INSTANCE_TEXT_BYTES if encoding == "text" else MAX_INSTANCE_PREVIEW_BYTES
        if os.name == "nt":
            target = safe_path(root, parts)
            regular_file(target, maximum_size=limit)
            try:
                with target.open("rb") as handle:
                    content = handle.read(limit + 1)
            except OSError as exc:
                raise LauncherError("Не удалось безопасно прочитать файл экземпляра") from exc
            if len(content) > limit:
                raise LauncherError("Файл не найден или превышает допустимый размер")
        else:
            try:
                with _open_parent(root, parts) as (parent_fd, file_name):
                    target_stat = os.stat(file_name, dir_fd=parent_fd, follow_symlinks=False)
                    if not stat.S_ISREG(target_stat.st_mode) or target_stat.st_size > limit:
                        raise LauncherError("Файл не найден или превышает допустимый размер")
                    fd = os.open(file_name, os.O_RDONLY | _NOFOLLOW | _NONBLOCK, dir_fd=parent_fd)
                    with os.fdopen(fd, "rb") as handle:
                        file_stat = os.fstat(handle.fileno())
                        if not stat.S_ISREG(file_stat.st_mode) or file_stat.st_size > limit:
                            raise LauncherError("Файл не найден или превышает допустимый размер")
                        content = handle.read(limit + 1)
                        if len(content) > limit:
                            raise LauncherError("Файл не найден или превышает допустимый размер")
            except LauncherError:
                raise
            except OSError as exc:
                raise LauncherError("Не удалось безопасно прочитать файл экземпляра") from exc

        if encoding == "text":
            try:
                text = content.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise LauncherError("Файл не является UTF-8 текстом") from exc
            if "\x00" in text:
                raise LauncherError("Бинарный файл нельзя открыть как текст")
            return {"content": text, "encoding": "text"}

        mime_type = _preview_image_mime(content)
        if not mime_type:
            raise LauncherError("Предпросмотр доступен только для поддерживаемых изображений")
        return {"content": base64.b64encode(content).decode("ascii"), "encoding": "base64", "mime_type": mime_type}

    def write_file(self, name: str, path: str, content: str) -> None:
        parts = _path_parts(path)
        try:
            encoded = content.encode("utf-8")
        except (AttributeError, UnicodeEncodeError) as exc:
            raise LauncherError("Текстовый файл должен содержать UTF-8 текст") from exc
        if len(encoded) > MAX_INSTANCE_TEXT_BYTES:
            raise LauncherError("Текстовый файл слишком большой (максимум 2 MiB)")
        with self.mutation_lock(name) as info:
            root = self._root_path(info)
            try:
                if os.name == "nt":
                    atomic_write_path(root, parts, encoded)
                else:
                    with _open_parent(root, parts) as (parent_fd, file_name):
                        _atomic_write_at(parent_fd, file_name, encoded)
            except LauncherError:
                raise
            except OSError as exc:
                raise LauncherError("Не удалось сохранить файл экземпляра") from exc

    def mkdir(self, name: str, path: str) -> None:
        parts = _path_parts(path)
        with self.mutation_lock(name) as info:
            root = self._root_path(info)
            try:
                if os.name == "nt":
                    target = safe_path(root, parts)
                    target.mkdir()
                else:
                    with _open_parent(root, parts) as (parent_fd, directory_name):
                        os.mkdir(directory_name, mode=0o700, dir_fd=parent_fd)
            except OSError as exc:
                raise LauncherError("Не удалось создать папку экземпляра") from exc

    def upload(self, name: str, path: str, content: bytes) -> None:
        parts = _path_parts(path)
        if len(content) > MAX_INSTANCE_UPLOAD_BYTES:
            raise LauncherError("Файл слишком большой (максимум 32 MiB)")
        with self.mutation_lock(name) as info:
            root = self._root_path(info)
            try:
                if os.name == "nt":
                    atomic_write_path(root, parts, content)
                else:
                    with _open_parent(root, parts) as (parent_fd, file_name):
                        _atomic_write_at(parent_fd, file_name, content)
            except LauncherError:
                raise
            except OSError as exc:
                raise LauncherError("Не удалось загрузить файл в экземпляр") from exc

    def delete(self, name: str, path: str) -> None:
        parts = _path_parts(path)
        with self.mutation_lock(name) as info:
            root = self._root_path(info)
            try:
                if os.name == "nt":
                    remove_tree_path(root, parts, reserved=_reserved_name, max_entries=_MAX_DELETE_ENTRIES, max_depth=_MAX_DELETE_DEPTH)
                else:
                    with _open_parent(root, parts) as (parent_fd, file_name):
                        _preflight_delete_at(parent_fd, file_name, [0], 0)
                        _remove_entry_at(parent_fd, file_name, 0)
            except LauncherError:
                raise
            except OSError as exc:
                raise LauncherError("Не удалось удалить путь экземпляра") from exc

    @contextmanager
    def mutation_lock(self, name: str) -> Iterator[InstanceInfo]:
        info = self._file_instance(name)
        lock = InstanceProcessLock(self.config.paths.data_home, info.name)
        lock.acquire()
        try:
            info = self._file_instance(info.name)
            if _instance_process_running(info.path):
                raise LauncherError("Нельзя изменять файлы запущенного экземпляра")
            yield info
        finally:
            lock.release()

    def list_log_archives(self, name: str, offset: int = 0, limit: int = 50) -> dict[str, Any]:
        info = self._client_instance(name)
        offset, limit = _pagination(offset, limit, maximum=MAX_INSTANCE_LOG_LIST_LIMIT)
        instances = self.instances.list()
        values: list[dict[str, Any]] = []
        if os.name == "nt":
            logs_dir = self.config.paths.logs_dir
            if logs_dir.is_dir() and not is_reparse_point(logs_dir):
                for item in logs_dir.iterdir():
                    if not _LOG_ID_RE.fullmatch(item.name) or is_reparse_point(item):
                        continue
                    try:
                        item_stat = item.stat()
                    except OSError:
                        continue
                    if stat.S_ISREG(item_stat.st_mode) and _log_belongs_to_instance(info, item.name, instances):
                        values.append({"id": item.name, "modified_at": item_stat.st_mtime, "size": item_stat.st_size})
        else:
            try:
                with _open_directory(self.config.paths.logs_dir, ()) as logs_fd:
                    for item in os.scandir(logs_fd):
                        if not _LOG_ID_RE.fullmatch(item.name):
                            continue
                        try:
                            item_stat = item.stat(follow_symlinks=False)
                        except OSError:
                            continue
                        if not stat.S_ISREG(item_stat.st_mode) or not _log_belongs_to_instance(info, item.name, instances):
                            continue
                        values.append({"id": item.name, "modified_at": item_stat.st_mtime, "size": item_stat.st_size})
            except FileNotFoundError:
                values = []
            except OSError as exc:
                raise LauncherError("Не удалось прочитать архивы логов экземпляра") from exc
        values.sort(key=lambda value: (-value["modified_at"], value["id"]))
        page = values[offset : offset + limit]
        next_offset = offset + len(page) if offset + len(page) < len(values) else None
        return {"logs": page, "next_offset": next_offset}

    def read_log_archive(self, name: str, log_id: str, offset: int = 0, limit: int = MAX_INSTANCE_LOG_READ_BYTES) -> dict[str, Any]:
        info = self._client_instance(name)
        if not isinstance(log_id, str) or not _LOG_ID_RE.fullmatch(log_id):
            raise LauncherError("Некорректный идентификатор архива лога")
        offset, limit = _log_read_range(offset, limit)
        instances = self.instances.list()
        if not _log_belongs_to_instance(info, log_id, instances):
            raise LauncherError("Архив лога не относится к этому экземпляру")
        if os.name == "nt":
            target = safe_path(self.config.paths.logs_dir, (log_id,))
            regular_file(target)
            try:
                with target.open("rb") as handle:
                    file_size = target.stat().st_size
                    offset = min(offset, file_size)
                    handle.seek(offset)
                    content = handle.read(limit)
            except OSError as exc:
                raise LauncherError("Не удалось безопасно прочитать архив лога") from exc
        else:
            try:
                with _open_directory(self.config.paths.logs_dir, ()) as logs_fd:
                    target_stat = os.stat(log_id, dir_fd=logs_fd, follow_symlinks=False)
                    if not stat.S_ISREG(target_stat.st_mode):
                        raise LauncherError("Архив лога недоступен")
                    fd = os.open(log_id, os.O_RDONLY | _NOFOLLOW | _NONBLOCK, dir_fd=logs_fd)
                    with os.fdopen(fd, "rb") as handle:
                        file_stat = os.fstat(handle.fileno())
                        if not stat.S_ISREG(file_stat.st_mode):
                            raise LauncherError("Архив лога недоступен")
                        offset = min(offset, file_stat.st_size)
                        content = os.pread(handle.fileno(), limit, offset)
                        file_size = os.fstat(handle.fileno()).st_size
            except LauncherError:
                raise
            except OSError as exc:
                raise LauncherError("Не удалось безопасно прочитать архив лога") from exc

        next_offset = offset + len(content)
        eof = next_offset >= file_size
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError as exc:
            if exc.reason == "unexpected end of data" and exc.end == len(content) and not eof:
                if exc.start:
                    content = content[: exc.start]
                    next_offset = offset + len(content)
                    text = content.decode("utf-8", errors="replace")
                    eof = next_offset >= file_size
                else:
                    text = ""
                    next_offset = offset
            else:
                text = content.decode("utf-8", errors="replace")
        return {"content": text, "next_offset": next_offset, "eof": eof}

    def read_log_tail(self, name: str, log_id: str, line_limit: int = 500) -> list[str]:
        info = self._client_instance(name)
        if not isinstance(log_id, str) or not _LOG_ID_RE.fullmatch(log_id):
            raise LauncherError("Некорректный идентификатор архива лога")
        if not 1 <= line_limit <= 1000:
            raise LauncherError("Размер хвоста лога должен быть от 1 до 1000 строк")
        if not _log_belongs_to_instance(info, log_id, self.instances.list()):
            raise LauncherError("Архив лога не относится к этому экземпляру")
        if os.name == "nt":
            target = safe_path(self.config.paths.logs_dir, (log_id,))
            regular_file(target)
            try:
                with target.open("rb") as handle:
                    file_size = target.stat().st_size
                    offset = max(0, file_size - MAX_INSTANCE_LIVE_LOG_BYTES)
                    handle.seek(offset)
                    content = handle.read(MAX_INSTANCE_LIVE_LOG_BYTES)
            except OSError as exc:
                raise LauncherError("Не удалось безопасно прочитать текущий лог") from exc
        else:
            try:
                with _open_directory(self.config.paths.logs_dir, ()) as logs_fd:
                    target_stat = os.stat(log_id, dir_fd=logs_fd, follow_symlinks=False)
                    if not stat.S_ISREG(target_stat.st_mode):
                        raise LauncherError("Архив лога недоступен")
                    fd = os.open(log_id, os.O_RDONLY | _NOFOLLOW | _NONBLOCK, dir_fd=logs_fd)
                    with os.fdopen(fd, "rb") as handle:
                        file_stat = os.fstat(handle.fileno())
                        if not stat.S_ISREG(file_stat.st_mode):
                            raise LauncherError("Архив лога недоступен")
                        offset = max(0, file_stat.st_size - MAX_INSTANCE_LIVE_LOG_BYTES)
                        content = os.pread(handle.fileno(), file_stat.st_size - offset, offset)
            except LauncherError:
                raise
            except OSError as exc:
                raise LauncherError("Не удалось безопасно прочитать текущий лог") from exc

        text = content.decode("utf-8", errors="replace").replace("\r", "")
        if offset:
            first_newline = text.find("\n")
            text = text[first_newline + 1 :] if first_newline >= 0 else ""
        return text.splitlines()[-line_limit:]

    def _client_instance(self, name: str) -> InstanceInfo:
        info = self.instances.get(name)
        # Логи/консоль доступны и Bedrock-экземплярам: их сессии пишет
        # Rust-движок тем же SessionLog (префикс <имя>-<hash>-bedrock-…).
        if info.name != name or getattr(info, "kind", "client") != "client" or info.game_edition not in ("java", "bedrock"):
            raise LauncherError("Файловый доступ доступен только для игровых экземпляров")
        return info

    def _file_instance(self, name: str) -> InstanceInfo:
        info = self.instances.get(name)
        if info.name != name or getattr(info, "kind", "client") != "client":
            raise LauncherError("Файловый доступ доступен только для игровых экземпляров")
        if info.game_edition == "bedrock":
            if self.bedrock_profiles is None:
                raise LauncherError("Профиль Bedrock недоступен")
            self.bedrock_profiles.profile_path(info)
            return info
        if info.game_edition != "java":
            raise LauncherError("Неизвестное издание игрового экземпляра")
        return info

    def _root_path(self, info: InstanceInfo) -> Path:
        if info.game_edition == "bedrock":
            if self.bedrock_profiles is None:
                raise LauncherError("Профиль Bedrock недоступен")
            return self.bedrock_profiles.profile_path(info)
        return info.path


def _path_parts(raw: str, *, allow_root: bool = False) -> tuple[str, ...]:
    if not isinstance(raw, str):
        raise LauncherError("Путь должен быть текстом")
    if raw == "" and allow_root:
        return ()
    if (
        not raw
        or len(raw) > 4096
        or raw.startswith("/")
        or "\\" in raw
        or "\x00" in raw
        or re.match(r"^[A-Za-z]:", raw)
        or any(ord(char) < 32 for char in raw)
    ):
        raise LauncherError("Путь выходит за пределы экземпляра")
    parts = tuple(raw.split("/"))
    if not parts or any(not part or part in {".", ".."} or len(part) > 255 or _reserved_name(part) for part in parts):
        raise LauncherError("Путь содержит недопустимый компонент")
    return parts


def _reserved_name(name: str) -> bool:
    lowered = name.lower()
    return lowered in {
        ".trial-cell",
        ".launch.lock",
        ".xodus-keyring.ron",
        "msa",
        "profile.json",
        "xodus-home",
    } or lowered.startswith(".trial-launcher")


@contextmanager
def _open_directory(root: Path, parts: tuple[str, ...]) -> Iterator[int]:
    root_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
    directory_fd = -1
    try:
        directory_fd = _open_directory_chain(root_fd, parts)
        yield directory_fd
    finally:
        if directory_fd >= 0:
            os.close(directory_fd)
        os.close(root_fd)


@contextmanager
def _open_parent(root: Path, parts: tuple[str, ...]) -> Iterator[tuple[int, str]]:
    if not parts:
        raise LauncherError("Укажите файл или папку")
    root_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
    parent_fd = -1
    try:
        parent_fd = _open_directory_chain(root_fd, parts[:-1])
        yield parent_fd, parts[-1]
    finally:
        if parent_fd >= 0:
            os.close(parent_fd)
        os.close(root_fd)


def _open_directory_chain(root_fd: int, parts: tuple[str, ...]) -> int:
    current_fd = os.dup(root_fd)
    try:
        for part in parts:
            next_fd = os.open(part, _DIRECTORY_OPEN_FLAGS, dir_fd=current_fd)
            os.close(current_fd)
            current_fd = next_fd
        return current_fd
    except BaseException:
        os.close(current_fd)
        raise


def _atomic_write_at(parent_fd: int, file_name: str, content: bytes) -> None:
    _ensure_regular_destination(parent_fd, file_name)
    temporary_name = f".trial-launcher-workspace-{uuid.uuid4().hex}.part"
    fd = os.open(temporary_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW, 0o600, dir_fd=parent_fd)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        _ensure_regular_destination(parent_fd, file_name)
        os.replace(temporary_name, file_name, src_dir_fd=parent_fd, dst_dir_fd=parent_fd)
        try:
            os.fsync(parent_fd)
        except OSError:
            pass
    finally:
        try:
            os.unlink(temporary_name, dir_fd=parent_fd)
        except FileNotFoundError:
            pass


def _ensure_regular_destination(parent_fd: int, file_name: str) -> None:
    try:
        value = os.stat(file_name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError:
        return
    if not stat.S_ISREG(value.st_mode):
        raise LauncherError("Файл является ссылкой или специальным объектом")


def _preflight_delete_at(parent_fd: int, name: str, count: list[int], depth: int) -> None:
    if depth > _MAX_DELETE_DEPTH:
        raise LauncherError("Папка содержит слишком много уровней вложенности")
    value = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISREG(value.st_mode):
        return
    if not stat.S_ISDIR(value.st_mode):
        raise LauncherError("Можно удалить только файл или папку")
    directory_fd = os.open(name, _DIRECTORY_OPEN_FLAGS, dir_fd=parent_fd)
    try:
        for item in os.scandir(directory_fd):
            if _reserved_name(item.name):
                raise LauncherError("Папка содержит служебные данные лаунчера")
            count[0] += 1
            if count[0] > _MAX_DELETE_ENTRIES:
                raise LauncherError("Папка содержит слишком много файлов для удаления")
            _preflight_delete_at(directory_fd, item.name, count, depth + 1)
    finally:
        os.close(directory_fd)


def _remove_entry_at(parent_fd: int, name: str, depth: int) -> None:
    if depth > _MAX_DELETE_DEPTH:
        raise LauncherError("Папка содержит слишком много уровней вложенности")
    value = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISREG(value.st_mode):
        os.unlink(name, dir_fd=parent_fd)
        return
    if not stat.S_ISDIR(value.st_mode):
        raise LauncherError("Можно удалить только файл или папку")
    directory_fd = os.open(name, _DIRECTORY_OPEN_FLAGS, dir_fd=parent_fd)
    try:
        for item in os.scandir(directory_fd):
            if _reserved_name(item.name):
                raise LauncherError("Папка содержит служебные данные лаунчера")
            _remove_entry_at(directory_fd, item.name, depth + 1)
    finally:
        os.close(directory_fd)
    os.rmdir(name, dir_fd=parent_fd)


def _preview_image_mime(content: bytes) -> str | None:
    for signature, mime_type in _IMAGE_MIMES:
        if content.startswith(signature):
            return mime_type
    if len(content) >= 12 and content[:4] == b"RIFF" and content[8:12] == b"WEBP":
        return "image/webp"
    return None


def _instance_process_running(root: Path) -> bool:
    if os.name == "nt":
        state_path = safe_path(root, (".trial-launcher-live.json",))
        if is_reparse_point(state_path) or not state_path.is_file() or state_path.stat().st_size > 64 * 1024:
            return False
        try:
            value = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, ValueError, TypeError) as exc:
            raise LauncherError("Не удалось проверить состояние запуска экземпляра") from exc
        if not isinstance(value, dict) or not value.get("running"):
            return False
        try:
            pid = int(value.get("pid", 0))
        except (TypeError, ValueError) as exc:
            raise LauncherError("Состояние запуска экземпляра повреждено") from exc
        return pid <= 0 or process_exists(pid)
    flags = os.O_RDONLY | _NOFOLLOW | _NONBLOCK
    try:
        fd = os.open(root / ".trial-launcher-live.json", flags)
    except FileNotFoundError:
        return False
    except OSError as exc:
        raise LauncherError("Не удалось проверить состояние запуска экземпляра") from exc
    try:
        value_stat = os.fstat(fd)
        if not stat.S_ISREG(value_stat.st_mode) or value_stat.st_size > 64 * 1024:
            raise LauncherError("Состояние запуска экземпляра повреждено")
        with os.fdopen(fd, "rb", closefd=False) as handle:
            raw = handle.read(64 * 1024 + 1)
    finally:
        os.close(fd)
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, TypeError) as exc:
        raise LauncherError("Не удалось проверить состояние запуска экземпляра") from exc
    if not isinstance(value, dict) or not value.get("running"):
        return False
    try:
        pid = int(value.get("pid", 0))
    except (TypeError, ValueError) as exc:
        raise LauncherError("Состояние запуска экземпляра повреждено") from exc
    if pid <= 0:
        return True
    return process_exists(pid)


def _log_component(value: str) -> str:
    normalized = re.sub(r"[^A-Za-z0-9._-]+", "_", str(value)).strip("._-")
    return normalized[:80] or "launcher"


def _known_versions(info: InstanceInfo) -> set[str]:
    values = {info.version_id} if info.version_id else set()
    if os.name == "nt":
        try:
            versions_dir = safe_path(info.path, ("versions",))
            if versions_dir.is_dir():
                for item in versions_dir.iterdir():
                    if item.is_dir() and not is_reparse_point(item) and not _reserved_name(item.name):
                        values.add(item.name)
        except (OSError, LauncherError):
            pass
        return {value for value in values if re.fullmatch(r"[A-Za-z0-9._-]{1,128}", value)}
    try:
        with _open_directory(info.path, ("versions",)) as versions_fd:
            for item in os.scandir(versions_fd):
                try:
                    value = item.stat(follow_symlinks=False)
                except OSError:
                    continue
                if stat.S_ISDIR(value.st_mode) and not _reserved_name(item.name):
                    values.add(item.name)
    except OSError:
        pass
    return {value for value in values if re.fullmatch(r"[A-Za-z0-9._-]{1,128}", value)}


def _log_belongs_to_instance(info: InstanceInfo, log_id: str, instances: list[InstanceInfo]) -> bool:
    modern_prefix = f"{_log_component(info.name)}-{instance_log_hash(info.name)}-"
    if log_id.startswith(modern_prefix):
        return bool(_LOG_VERSION_TAIL_RE.fullmatch(log_id[len(modern_prefix) :]))

    matches: set[str] = set()
    for candidate in instances:
        if getattr(candidate, "kind", "client") != "client":
            continue
        for version in _known_versions(candidate):
            prefix = f"{_log_component(candidate.name)}-{_log_component(version)}-"
            if log_id.startswith(prefix) and _LOG_SUFFIX_RE.fullmatch(log_id[len(prefix) :]):
                matches.add(candidate.name)
                break
    return matches == {info.name}


def _pagination(offset: Any, limit: Any, *, maximum: int) -> tuple[int, int]:
    try:
        normalized_offset, normalized_limit = int(offset), int(limit)
    except (TypeError, ValueError) as exc:
        raise LauncherError("Смещение и размер страницы должны быть числами") from exc
    if normalized_offset < 0 or not 1 <= normalized_limit <= maximum:
        raise LauncherError(f"Размер страницы должен быть от 1 до {maximum}, смещение не может быть отрицательным")
    return normalized_offset, normalized_limit


def _log_read_range(offset: Any, limit: Any) -> tuple[int, int]:
    try:
        normalized_offset, normalized_limit = int(offset), int(limit)
    except (TypeError, ValueError) as exc:
        raise LauncherError("Смещение и размер чтения должны быть числами") from exc
    if normalized_offset < 0 or not MIN_INSTANCE_LOG_READ_BYTES <= normalized_limit <= MAX_INSTANCE_LOG_READ_BYTES:
        raise LauncherError(
            f"Размер чтения должен быть от {MIN_INSTANCE_LOG_READ_BYTES} до {MAX_INSTANCE_LOG_READ_BYTES}"
        )
    return normalized_offset, normalized_limit
