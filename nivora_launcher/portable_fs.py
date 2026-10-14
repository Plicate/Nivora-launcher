from __future__ import annotations

import os
import stat
import tempfile
from pathlib import Path
from typing import Callable

from .errors import LauncherError
from .platform_support import is_reparse_point, set_private_fd


def safe_path(root: Path, parts: tuple[str, ...]) -> Path:
    """Resolve a relative path while rejecting links, junctions, and escapes."""
    if is_reparse_point(root):
        raise LauncherError("Корневой каталог является ссылкой")
    try:
        resolved_root = root.resolve(strict=True)
    except OSError as exc:
        raise LauncherError("Корневой каталог недоступен") from exc
    current = resolved_root
    for part in parts:
        _validate_component(part)
        current = current / part
        if is_reparse_point(current):
            raise LauncherError("Путь содержит символьную ссылку или junction")
        if os.path.lexists(current):
            try:
                resolved = current.resolve(strict=True)
                resolved.relative_to(resolved_root)
            except (OSError, ValueError) as exc:
                raise LauncherError("Путь выходит за пределы корневого каталога") from exc
    return current


def _validate_component(part: str) -> None:
    if (
        not part
        or part in {".", ".."}
        or "/" in part
        or "\\" in part
        or any(char in part for char in '<>:"|?*')
        or "\x00" in part
        or any(ord(char) < 32 for char in part)
        or part.endswith((" ", "."))
    ):
        raise LauncherError("Путь содержит недопустимый компонент")
    device_name = part.split(".", 1)[0].upper()
    if device_name in {"CON", "PRN", "AUX", "NUL", *(f"COM{index}" for index in range(1, 10)), *(f"LPT{index}" for index in range(1, 10))}:
        raise LauncherError("Путь содержит зарезервированное имя Windows")


def atomic_write(root: Path, parts: tuple[str, ...], content: bytes) -> Path:
    if not parts:
        raise LauncherError("Укажите файл")
    target = safe_path(root, parts)
    parent = safe_path(root, parts[:-1])
    if not parent.is_dir():
        raise LauncherError("Родительский каталог недоступен")
    if os.path.lexists(target) and not target.is_file():
        raise LauncherError("Путь файла является ссылкой или имеет неверный тип")
    descriptor, temporary_name = tempfile.mkstemp(prefix=".trial-launcher-", suffix=".part", dir=parent)
    temporary = Path(temporary_name)
    try:
        set_private_fd(descriptor)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        safe_path(root, parts)
        temporary.replace(target)
        return target
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def remove_tree(
    root: Path,
    parts: tuple[str, ...],
    *,
    reserved: Callable[[str], bool] | None = None,
    max_entries: int = 100_000,
    max_depth: int = 128,
) -> None:
    if not parts:
        raise LauncherError("Нельзя удалить корневой каталог")
    target = safe_path(root, parts)
    count = [0]

    def remove(path: Path, relative: tuple[str, ...], depth: int) -> None:
        if depth > max_depth:
            raise LauncherError("Папка содержит слишком много уровней вложенности")
        safe_path(root, relative)
        if is_reparse_point(path):
            raise LauncherError("Нельзя удалять символьные ссылки или junction")
        if path.is_file():
            path.unlink()
            return
        if not path.is_dir():
            raise LauncherError("Можно удалить только обычный файл или папку")
        for child in path.iterdir():
            if reserved and reserved(child.name):
                raise LauncherError("Папка содержит служебные данные лаунчера")
            count[0] += 1
            if count[0] > max_entries:
                raise LauncherError("Папка содержит слишком много файлов для удаления")
            remove(child, (*relative, child.name), depth + 1)
        path.rmdir()

    remove(target, parts, 0)


def regular_file(path: Path, *, maximum_size: int | None = None) -> os.stat_result:
    if is_reparse_point(path):
        raise LauncherError("Файл является ссылкой или junction")
    try:
        value = path.stat()
    except OSError as exc:
        raise LauncherError("Файл недоступен") from exc
    if not stat.S_ISREG(value.st_mode) or (maximum_size is not None and value.st_size > maximum_size):
        raise LauncherError("Файл недоступен или превышает допустимый размер")
    return value
