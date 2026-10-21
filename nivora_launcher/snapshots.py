from __future__ import annotations

import json
import os
import re
import stat
import tempfile
import time
import uuid
import zipfile
from pathlib import Path
from typing import Any

from .config import _atomic_json_write
from .errors import LauncherError
from .platform_support import is_reparse_point, set_private_fd
from .portable_fs import regular_file, safe_path

SNAPSHOT_DIR = ".trial-launcher-snapshots"
SNAPSHOT_RETENTION_HOURS = 5
SNAPSHOT_MAX_COUNT = 1
SNAPSHOT_CATEGORIES = {
    "mods": "mods",
    "plugins": "plugins",
    "resourcepacks": "resourcepacks",
    "shaders": "shaderpacks",
    "saves": "saves",
}
SNAPSHOT_ID_RE = re.compile(r"^[0-9]{1,20}-[0-9a-f]{8}$")
SNAPSHOT_MAX_FILE_BYTES = 256 * 1024 * 1024
SNAPSHOT_MAX_UNPACKED_BYTES = 512 * 1024 * 1024
MAX_SNAPSHOT_MANIFEST_BYTES = 64 * 1024
MAX_SNAPSHOT_MANIFESTS_SCANNED = 500
_DIRECTORY_OPEN_FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
_NOFOLLOW = getattr(os, "O_NOFOLLOW", 0)
_NONBLOCK = getattr(os, "O_NONBLOCK", 0)

class SnapshotStore:
    def __init__(self, instance_root: Path, categories: dict[str, str] | None = None) -> None:
        self.root = instance_root
        # bedrock снапшотирует контент com.mojang (resource_packs/
        self.categories = categories if categories is not None else SNAPSHOT_CATEGORIES
        if is_reparse_point(self.root) or not self.root.is_dir():
            raise LauncherError("Каталог экземпляра недоступен для снапшотов")

    @property
    def directory(self) -> Path:
        path = self.root / SNAPSHOT_DIR
        if is_reparse_point(path):
            raise LauncherError("Каталог снапшотов является ссылкой или junction")
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        return path

    def list(self, category: str) -> list[dict[str, Any]]:
        target = self._target(category)
        directory = self.directory
        if os.name == "nt":
            return _list_windows(directory, target, category)
        directory_fd = os.open(directory, _DIRECTORY_OPEN_FLAGS)
        values: list[dict[str, Any]] = []
        try:
            candidates: list[tuple[float, str]] = []
            scanned = 0
            with os.scandir(directory_fd) as entries:
                for entry in entries:
                    if not entry.name.startswith(f"{category}-") or not entry.name.endswith(".json"):
                        continue
                    scanned += 1
                    if scanned > MAX_SNAPSHOT_MANIFESTS_SCANNED:
                        break
                    try:
                        item_stat = entry.stat(follow_symlinks=False)
                    except OSError:
                        continue
                    if not stat.S_ISREG(item_stat.st_mode) or item_stat.st_size > MAX_SNAPSHOT_MANIFEST_BYTES:
                        continue
                    candidates.append((item_stat.st_mtime, entry.name))
            for _, manifest_name in sorted(candidates, key=lambda item: item[0], reverse=True):
                try:
                    fd = os.open(manifest_name, os.O_RDONLY | _NOFOLLOW | _NONBLOCK, dir_fd=directory_fd)
                    with os.fdopen(fd, "rb") as handle:
                        item_stat = os.fstat(handle.fileno())
                        if not stat.S_ISREG(item_stat.st_mode) or item_stat.st_size > MAX_SNAPSHOT_MANIFEST_BYTES:
                            continue
                        raw = handle.read(MAX_SNAPSHOT_MANIFEST_BYTES + 1)
                        if len(raw) > MAX_SNAPSHOT_MANIFEST_BYTES:
                            continue
                    value = json.loads(raw.decode("utf-8"))
                    if not isinstance(value, dict):
                        continue
                    snapshot_id = str(value.get("id", ""))
                    archive_name = str(value.get("archive", ""))
                    if value.get("category") != category or not SNAPSHOT_ID_RE.fullmatch(snapshot_id) or archive_name != f"{category}-{snapshot_id}.zip":
                        continue
                    archive = directory / archive_name
                    if archive.parent != directory or archive.is_symlink() or not archive.is_file():
                        continue
                    values.append({**value, "archive": archive.name, "category": category, "target": target.name})
                except (OSError, UnicodeError, ValueError, TypeError, RecursionError):
                    continue
        finally:
            os.close(directory_fd)
        return values[:50]

    def create(self, category: str, *, retention_hours: int = SNAPSHOT_RETENTION_HOURS, max_count: int = SNAPSHOT_MAX_COUNT) -> dict[str, Any]:
        target = self._target(category)
        snapshot_id = f"{int(time.time())}-{uuid.uuid4().hex[:8]}"
        archive_name = f"{category}-{snapshot_id}.zip"
        archive_path = self.directory / archive_name
        if is_reparse_point(archive_path):
            raise LauncherError("Путь снапшота является ссылкой или junction")
        file_count = 0
        total_size = 0
        try:
            with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
                if is_reparse_point(target):
                    raise LauncherError("Каталог снапшота является ссылкой или junction")
                if target.exists():
                    if os.name == "nt":
                        file_count, total_size = _write_directory_archive_windows(target, archive)
                    else:
                        file_count, total_size = _write_directory_archive(target, archive)
        except (LauncherError, OSError) as exc:
            archive_path.unlink(missing_ok=True)
            if isinstance(exc, LauncherError):
                raise
            raise LauncherError(f"Не удалось создать снапшот: {exc}") from exc
        manifest = {
            "id": snapshot_id,
            "category": category,
            "archive": archive_name,
            "created_at": time.time(),
            "file_count": file_count,
            "source_bytes": total_size,
            "retention_hours": max(1, min(720, int(retention_hours))),
        }
        _atomic_json_write(self.directory / f"{category}-{snapshot_id}.json", manifest, private=True)
        self._cleanup(category, manifest["retention_hours"], max_count)
        return manifest

    def restore(self, category: str, snapshot_id: str) -> dict[str, Any]:
        value = next((item for item in self.list(category) if item.get("id") == snapshot_id), None)
        if not value:
            raise LauncherError("Снапшот не найден")
        archive_path = self._archive_path(category, value)
        target = self._target(category)
        temporary = self.root / f".{category}-restore-{uuid.uuid4().hex}.part"
        try:
            temporary.mkdir(mode=0o700)
            _extract_archive(archive_path, temporary)
            _replace_directory_contents(self.root, target, temporary)
            return {**value, "restored_at": time.time()}
        finally:
            if temporary.exists():
                _remove_tree(temporary)

    def delete(self, category: str, snapshot_id: str) -> bool:
        value = next((item for item in self.list(category) if item.get("id") == snapshot_id), None)
        if not value:
            return False
        archive_path = self._archive_path(category, value)
        for name in (archive_path.name, f"{category}-{snapshot_id}.json"):
            path = self.directory / name
            if path.is_symlink():
                raise LauncherError("Файл снапшота является символьной ссылкой")
            path.unlink(missing_ok=True)
        return True

    def latest(self, category: str) -> dict[str, Any] | None:
        values = self.list(category)
        return values[0] if values else None

    def _target(self, category: str) -> Path:
        relative = self.categories.get(category)
        if not relative:
            raise LauncherError("Неизвестная категория снапшота")
        target = self.root / relative
        if is_reparse_point(target):
            raise LauncherError("Каталог снапшота является ссылкой или junction")
        return target

    def _archive_path(self, category: str, value: dict[str, Any]) -> Path:
        snapshot_id = str(value.get("id", ""))
        archive_name = str(value.get("archive", ""))
        expected = f"{category}-{snapshot_id}.zip"
        if value.get("category") != category or not SNAPSHOT_ID_RE.fullmatch(snapshot_id) or archive_name != expected:
            raise LauncherError("Метаданные снапшота повреждены")
        archive = self.directory / archive_name
        if archive.parent != self.directory or is_reparse_point(archive) or not archive.is_file():
            raise LauncherError("Архив снапшота недоступен")
        return archive

    def _cleanup(self, category: str, retention_hours: int, max_count: int) -> None:
        now = time.time()
        values = self.list(category)
        limit = max(1, min(50, int(max_count)))
        for index, value in enumerate(values):
            created = float(value.get("created_at", 0))
            expired = created > 0 and now - created > max(1, retention_hours) * 3600
            if expired or index >= limit:
                self.delete(category, str(value.get("id", "")))

def _extract_archive(archive_path: Path, temporary: Path) -> None:
    if os.name == "nt":
        _extract_archive_windows(archive_path, temporary)
        return
    root_fd = os.open(temporary, _DIRECTORY_OPEN_FLAGS)
    seen: set[str] = set()
    total_size = 0
    try:
        with zipfile.ZipFile(archive_path) as archive:
            for member in archive.infolist():
                parts = _archive_parts(member.filename)
                normalized = "/".join(parts)
                if normalized in seen:
                    raise LauncherError("Снапшот содержит повторяющийся путь")
                seen.add(normalized)
                if _archive_member_is_symlink(member):
                    raise LauncherError("Снапшот содержит символьную ссылку")
                if member.is_dir():
                    _open_directory_chain(root_fd, parts, create=True)
                    continue
                if member.file_size < 0 or member.file_size > SNAPSHOT_MAX_FILE_BYTES:
                    raise LauncherError("Файл в снапшоте превышает лимит 256 MiB")
                total_size += member.file_size
                if total_size > SNAPSHOT_MAX_UNPACKED_BYTES:
                    raise LauncherError("Распакованный снапшот слишком большой")
                parent_fd = _open_directory_chain(root_fd, parts[:-1], create=True)
                try:
                    fd = os.open(parts[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW, 0o600, dir_fd=parent_fd)
                    with archive.open(member, "r") as source, os.fdopen(fd, "wb") as target:
                        written = 0
                        while True:
                            chunk = source.read(1024 * 1024)
                            if not chunk:
                                break
                            written += len(chunk)
                            if written > SNAPSHOT_MAX_FILE_BYTES:
                                raise LauncherError("Файл в снапшоте превышает лимит 256 MiB")
                            target.write(chunk)
                finally:
                    os.close(parent_fd)
    finally:
        os.close(root_fd)

def _write_directory_archive(target: Path, archive: zipfile.ZipFile) -> tuple[int, int]:
    root_fd = os.open(target, _DIRECTORY_OPEN_FLAGS)
    file_count = 0
    total_size = 0

    def visit(directory_fd: int, prefix: tuple[str, ...]) -> None:
        nonlocal file_count, total_size
        entries = sorted(os.scandir(directory_fd), key=lambda value: value.name)
        for entry in entries:
            item_stat = entry.stat(follow_symlinks=False)
            if stat.S_ISLNK(item_stat.st_mode):
                continue
            relative = (*prefix, entry.name)
            if stat.S_ISDIR(item_stat.st_mode):
                child_fd = os.open(entry.name, _DIRECTORY_OPEN_FLAGS, dir_fd=directory_fd)
                try:
                    visit(child_fd, relative)
                finally:
                    os.close(child_fd)
                continue
            if not stat.S_ISREG(item_stat.st_mode):
                continue
            if item_stat.st_size > SNAPSHOT_MAX_FILE_BYTES:
                raise LauncherError("Файл в снапшоте превышает лимит 256 MiB")
            file_fd = os.open(entry.name, os.O_RDONLY | _NOFOLLOW, dir_fd=directory_fd)
            try:
                with os.fdopen(file_fd, "rb") as source, archive.open("/".join(relative), "w") as destination:
                    written = 0
                    while True:
                        chunk = source.read(1024 * 1024)
                        if not chunk:
                            break
                        written += len(chunk)
                        total_size += len(chunk)
                        if written > SNAPSHOT_MAX_FILE_BYTES or total_size > SNAPSHOT_MAX_UNPACKED_BYTES:
                            raise LauncherError("Распакованный снапшот слишком большой")
                        destination.write(chunk)
            finally:
                file_count += 1

    try:
        visit(root_fd, ())
    finally:
        os.close(root_fd)
    return file_count, total_size

def _write_directory_archive_windows(target: Path, archive: zipfile.ZipFile) -> tuple[int, int]:
    file_count = 0
    total_size = 0

    def visit(directory: Path, prefix: tuple[str, ...]) -> None:
        nonlocal file_count, total_size
        for entry in sorted(directory.iterdir(), key=lambda value: value.name):
            if is_reparse_point(entry):
                continue
            value = entry.stat()
            relative = (*prefix, entry.name)
            if stat.S_ISDIR(value.st_mode):
                visit(entry, relative)
                continue
            if not stat.S_ISREG(value.st_mode):
                continue
            if value.st_size > SNAPSHOT_MAX_FILE_BYTES:
                raise LauncherError("Файл в снапшоте превышает лимит 256 MiB")
            file_size = 0
            with entry.open("rb") as source, archive.open("/".join(relative), "w") as destination:
                while True:
                    chunk = source.read(1024 * 1024)
                    if not chunk:
                        break
                    file_size += len(chunk)
                    total_size += len(chunk)
                    if file_size > SNAPSHOT_MAX_FILE_BYTES or total_size > SNAPSHOT_MAX_UNPACKED_BYTES:
                        raise LauncherError("Распакованный снапшот слишком большой")
                    destination.write(chunk)
            file_count += 1

    visit(target, ())
    return file_count, total_size

def _list_windows(directory: Path, target: Path, category: str) -> list[dict[str, Any]]:
    values: list[dict[str, Any]] = []
    candidates: list[tuple[float, Path]] = []
    scanned = 0
    for path in directory.iterdir():
        if not path.name.startswith(f"{category}-") or not path.name.endswith(".json") or is_reparse_point(path):
            continue
        scanned += 1
        if scanned > MAX_SNAPSHOT_MANIFESTS_SCANNED:
            break
        try:
            item_stat = regular_file(path, maximum_size=MAX_SNAPSHOT_MANIFEST_BYTES)
        except LauncherError:
            continue
        candidates.append((item_stat.st_mtime, path))
    for _, manifest_path in sorted(candidates, key=lambda item: item[0], reverse=True):
        try:
            raw = manifest_path.read_bytes()
            value = json.loads(raw.decode("utf-8"))
            if not isinstance(value, dict):
                continue
            snapshot_id = str(value.get("id", ""))
            archive_name = str(value.get("archive", ""))
            if value.get("category") != category or not SNAPSHOT_ID_RE.fullmatch(snapshot_id) or archive_name != f"{category}-{snapshot_id}.zip":
                continue
            archive = safe_path(directory, (archive_name,))
            regular_file(archive)
            values.append({**value, "archive": archive.name, "category": category, "target": target.name})
        except (OSError, UnicodeError, ValueError, TypeError, RecursionError, LauncherError):
            continue
    return values[:50]

def _extract_archive_windows(archive_path: Path, temporary: Path) -> None:
    seen: set[str] = set()
    total_size = 0
    total_written = 0
    try:
        with zipfile.ZipFile(archive_path) as archive:
            for member in archive.infolist():
                parts = _archive_parts(member.filename)
                normalized = "/".join(parts)
                if normalized in seen:
                    raise LauncherError("Снапшот содержит повторяющийся путь")
                seen.add(normalized)
                if _archive_member_is_symlink(member):
                    raise LauncherError("Снапшот содержит символьную ссылку")
                target = safe_path(temporary, parts)
                if member.is_dir():
                    target.mkdir(mode=0o700, parents=True, exist_ok=True)
                    continue
                if member.file_size < 0 or member.file_size > SNAPSHOT_MAX_FILE_BYTES:
                    raise LauncherError("Файл в снапшоте превышает лимит 256 MiB")
                total_size += member.file_size
                if total_size > SNAPSHOT_MAX_UNPACKED_BYTES:
                    raise LauncherError("Распакованный снапшот слишком большой")
                parent = safe_path(temporary, parts[:-1])
                parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                parent = safe_path(temporary, parts[:-1])
                descriptor, temporary_name = tempfile.mkstemp(prefix=".snapshot-", suffix=".part", dir=parent)
                staged = Path(temporary_name)
                written = 0
                try:
                    set_private_fd(descriptor)
                    with archive.open(member, "r") as source, os.fdopen(descriptor, "wb") as output:
                        while True:
                            chunk = source.read(1024 * 1024)
                            if not chunk:
                                break
                            written += len(chunk)
                            if written > SNAPSHOT_MAX_FILE_BYTES:
                                raise LauncherError("Файл в снапшоте превышает лимит 256 MiB")
                            if total_written + written > SNAPSHOT_MAX_UNPACKED_BYTES:
                                raise LauncherError("Распакованный снапшот слишком большой")
                            output.write(chunk)
                        output.flush()
                        os.fsync(output.fileno())
                    safe_path(temporary, parts)
                    staged.replace(target)
                    total_written += written
                finally:
                    staged.unlink(missing_ok=True)
    except zipfile.BadZipFile as exc:
        raise LauncherError("Архив снапшота повреждён") from exc

def _replace_directory_contents(root: Path, target: Path, temporary: Path) -> None:
    if os.name == "nt":
        relative = tuple(target.relative_to(root).parts)
        safe_target = safe_path(root, relative)
        safe_temporary = safe_path(root, tuple(temporary.relative_to(root).parts))
        safe_target.mkdir(mode=0o700, parents=True, exist_ok=True)
        if not safe_target.is_dir() or not safe_temporary.is_dir():
            raise LauncherError("Каталог восстановления снапшота недоступен")
        for entry in safe_target.iterdir():
            if is_reparse_point(entry):
                raise LauncherError("Целевая папка содержит ссылку или junction")
            if entry.is_dir():
                _remove_tree(entry)
            else:
                regular_file(entry)
                entry.unlink()
        for entry in safe_temporary.iterdir():
            if is_reparse_point(entry):
                raise LauncherError("Снапшот содержит ссылку или junction")
            entry.replace(safe_target / entry.name)
        return
    root_fd = os.open(root, _DIRECTORY_OPEN_FLAGS)
    target_fd = -1
    temporary_fd = -1
    try:
        relative = tuple(target.relative_to(root).parts)
        target_fd = _open_directory_chain(root_fd, relative, create=True)
        temporary_fd = os.open(temporary, _DIRECTORY_OPEN_FLAGS)
        for entry in os.scandir(target_fd):
            _remove_entry_at(target_fd, entry.name)
        for entry in os.scandir(temporary_fd):
            if entry.is_symlink():
                raise LauncherError("Снапшот содержит символьную ссылку")
            os.replace(entry.name, entry.name, src_dir_fd=temporary_fd, dst_dir_fd=target_fd)
    finally:
        if temporary_fd >= 0:
            os.close(temporary_fd)
        if target_fd >= 0:
            os.close(target_fd)
        os.close(root_fd)

def _archive_parts(member_name: str) -> tuple[str, ...]:
    value = str(member_name).replace("\\", "/")
    path = Path(value)
    parts = tuple(path.parts)
    if path.is_absolute() or not parts or any(part in {"", ".", ".."} for part in parts):
        raise LauncherError("Снапшот содержит небезопасный путь")
    return parts

def _archive_member_is_symlink(member: zipfile.ZipInfo) -> bool:
    return stat.S_ISLNK((member.external_attr >> 16) & 0o170000)

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

def _remove_entry_at(parent_fd: int, name: str) -> None:
    item_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISLNK(item_stat.st_mode):
        os.unlink(name, dir_fd=parent_fd)
        return
    if stat.S_ISDIR(item_stat.st_mode):
        directory_fd = os.open(name, _DIRECTORY_OPEN_FLAGS, dir_fd=parent_fd)
        try:
            for entry in os.scandir(directory_fd):
                _remove_entry_at(directory_fd, entry.name)
        finally:
            os.close(directory_fd)
        os.rmdir(name, dir_fd=parent_fd)
        return
    if stat.S_ISREG(item_stat.st_mode):
        os.unlink(name, dir_fd=parent_fd)
        return
    raise LauncherError("Снапшот содержит неподдерживаемый тип файла")

def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False

def _remove_tree(path: Path) -> None:
    if is_reparse_point(path) or path.is_file():
        path.unlink()
        return
    if path.is_dir():
        for child in path.iterdir():
            _remove_tree(child)
        path.rmdir()
