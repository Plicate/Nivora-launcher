"""Импорт миров Minecraft Java в ``saves/`` экземпляра.

Интерфейс передаёт данные частями (``begin`` → ``chunk``… → ``finish``):
мир бывает на сотни мегабайт, а один JSON-запрос с base64 такого размера
съел бы память и desktop bridge, и воркера.

- **Архив** (.zip, .tar, .tar.gz/.tgz, .tar.bz2, .tar.xz) — распаковывается
  во временную папку с проверкой путей, ссылок и размера; каждая папка
  с ``level.dat`` внутри становится отдельным миром.
- **Папка** — файлы приходят с относительными путями и собираются в
  staging-каталоге, дальше тот же поиск ``level.dat``.

Staging лежит внутри экземпляра (``.nivora-world-import``), поэтому перенос
готового мира в ``saves/`` — атомарный rename на том же диске.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import tarfile
import time
import uuid
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any

from .errors import LauncherError

STAGING_DIRECTORY = ".nivora-world-import"
MAX_UPLOAD_BYTES = 8 * 1024 * 1024 * 1024
MAX_EXTRACTED_BYTES = 16 * 1024 * 1024 * 1024
MAX_FILES = 300_000
MAX_CHUNK_BYTES = 32 * 1024 * 1024
MAX_SEARCH_DEPTH = 6
STALE_UPLOAD_SECONDS = 24 * 60 * 60
ARCHIVE_SUFFIXES = (".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz")
UNSUPPORTED_ARCHIVES = (".rar", ".7z")
_UPLOAD_ID = re.compile(r"[0-9a-f]{32}")
_FORBIDDEN_NAME = re.compile(r'[<>:"|?*\x00-\x1f]')


def _staging_root(instance_dir: Path) -> Path:
    root = instance_dir / STAGING_DIRECTORY
    if root.is_symlink() or (root.exists() and not root.is_dir()):
        raise LauncherError("Каталог импорта миров недоступен")
    return root


def _upload_dir(instance_dir: Path, upload_id: str) -> Path:
    if not _UPLOAD_ID.fullmatch(str(upload_id)):
        raise LauncherError("Некорректный идентификатор загрузки")
    directory = _staging_root(instance_dir) / upload_id
    if directory.is_symlink() or not directory.is_dir():
        raise LauncherError("Загрузка мира не найдена или уже завершена")
    return directory


def _read_meta(directory: Path) -> dict[str, Any]:
    try:
        value = json.loads((directory / "meta.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise LauncherError("Загрузка мира повреждена") from exc
    if not isinstance(value, dict):
        raise LauncherError("Загрузка мира повреждена")
    return value


def _write_meta(directory: Path, meta: dict[str, Any]) -> None:
    (directory / "meta.json").write_text(json.dumps(meta), encoding="utf-8")


def safe_relative_path(value: str) -> PurePosixPath:
    """Относительный путь файла внутри загружаемой папки/архива."""
    text = str(value or "").replace("\\", "/").strip()
    if not text or "\x00" in text or text.startswith("/") or re.match(r"^[A-Za-z]:", text):
        raise LauncherError(f"Небезопасный путь файла: {value!r}")
    parts = [part for part in text.split("/") if part not in ("", ".")]
    if not parts or any(part == ".." or _FORBIDDEN_NAME.search(part) for part in parts):
        raise LauncherError(f"Небезопасный путь файла: {value!r}")
    if len(parts) > 32 or len(text) > 1024:
        raise LauncherError("Слишком глубокая вложенность файлов мира")
    return PurePosixPath(*parts)


def cleanup_stale(instance_dir: Path) -> None:
    root = instance_dir / STAGING_DIRECTORY
    if root.is_symlink() or not root.is_dir():
        return
    now = time.time()
    for item in root.iterdir():
        try:
            if item.is_dir() and not item.is_symlink() and now - item.stat().st_mtime > STALE_UPLOAD_SECONDS:
                shutil.rmtree(item, ignore_errors=True)
        except OSError:
            continue


def begin(instance_dir: Path, kind: str, label: str) -> str:
    kind = str(kind).strip().lower()
    if kind not in {"archive", "folder"}:
        raise LauncherError("Импортировать можно архив или папку мира")
    label = str(label or "").strip()[:200]
    if kind == "archive":
        lowered = label.lower()
        if lowered.endswith(UNSUPPORTED_ARCHIVES):
            raise LauncherError("Архивы RAR и 7z не поддерживаются — упакуйте мир в ZIP или выберите папку мира")
        if not lowered.endswith(ARCHIVE_SUFFIXES):
            raise LauncherError("Поддерживаются архивы .zip, .tar, .tar.gz, .tar.bz2 и .tar.xz")
    cleanup_stale(instance_dir)
    root = _staging_root(instance_dir)
    root.mkdir(mode=0o700, exist_ok=True)
    upload_id = uuid.uuid4().hex
    directory = root / upload_id
    (directory / "payload").mkdir(mode=0o700, parents=True)
    _write_meta(directory, {"kind": kind, "label": label, "bytes": 0, "files": 0, "created": time.time()})
    return upload_id


def chunk(instance_dir: Path, upload_id: str, relative_path: str, data: bytes, offset: int) -> dict[str, Any]:
    directory = _upload_dir(instance_dir, upload_id)
    meta = _read_meta(directory)
    if len(data) > MAX_CHUNK_BYTES:
        raise LauncherError("Слишком большой фрагмент загрузки")
    relative = safe_relative_path(relative_path)
    if meta.get("kind") == "archive" and len(relative.parts) != 1:
        raise LauncherError("Архив мира передаётся одним файлом")
    target = directory / "payload" / Path(*relative.parts)
    payload = (directory / "payload").resolve()
    parent = target.parent
    if not parent.resolve().is_relative_to(payload):
        raise LauncherError("Путь файла выходит за пределы загрузки")
    if target.is_symlink():
        raise LauncherError("Файл загрузки является ссылкой")
    exists = target.exists()
    current = target.stat().st_size if exists else 0
    offset = int(offset)
    if offset != current:
        raise LauncherError("Фрагменты мира пришли не по порядку — повторите импорт")
    total = int(meta.get("bytes", 0)) + len(data)
    if total > MAX_UPLOAD_BYTES:
        raise LauncherError("Мир слишком большой для импорта (максимум 8 ГиБ)")
    files = int(meta.get("files", 0)) + (0 if exists else 1)
    if files > MAX_FILES:
        raise LauncherError("В мире слишком много файлов")
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with open(target, "ab") as handle:
        handle.write(data)
    meta["bytes"] = total
    meta["files"] = files
    _write_meta(directory, meta)
    return {"bytes": total, "files": files}


def cancel(instance_dir: Path, upload_id: str) -> None:
    try:
        directory = _upload_dir(instance_dir, upload_id)
    except LauncherError:
        return
    shutil.rmtree(directory, ignore_errors=True)


def finish(instance_dir: Path, upload_id: str) -> list[str]:
    directory = _upload_dir(instance_dir, upload_id)
    try:
        meta = _read_meta(directory)
        payload = directory / "payload"
        label = str(meta.get("label", "")).strip()
        if meta.get("kind") == "archive":
            archives = [item for item in payload.iterdir() if item.is_file() and not item.is_symlink()]
            if len(archives) != 1:
                raise LauncherError("Архив мира не загружен")
            root = directory / "extracted"
            root.mkdir(mode=0o700)
            extract_archive(archives[0], root)
            fallback_name = _strip_archive_suffix(archives[0].name or label)
        else:
            root = payload
            fallback_name = label or "Мир"
        worlds = find_world_roots(root)
        if not worlds:
            raise LauncherError(
                "Мир Minecraft не найден: внутри нет level.dat. "
                + ("Проверьте, что архив содержит папку сохранения." if meta.get("kind") == "archive" else "Выберите папку конкретного мира из saves/.")
            )
        saves = instance_dir / "saves"
        if saves.is_symlink() or (saves.exists() and not saves.is_dir()):
            raise LauncherError("Каталог миров является символьной ссылкой")
        saves.mkdir(mode=0o700, exist_ok=True)
        imported: list[str] = []
        for world in worlds:
            # Папка самого архива/выбранной папки не несёт имени — берём имя файла.
            if world == root:
                name = fallback_name
            else:
                name = world.name
            target = _unique_world_dir(saves, _safe_world_name(name))
            os.replace(world, target)
            imported.append(target.name)
        return imported
    finally:
        shutil.rmtree(directory, ignore_errors=True)


def _strip_archive_suffix(name: str) -> str:
    lowered = name.lower()
    for suffix in sorted(ARCHIVE_SUFFIXES, key=len, reverse=True):
        if lowered.endswith(suffix):
            return name[: -len(suffix)]
    return name


def _safe_world_name(value: str) -> str:
    name = _FORBIDDEN_NAME.sub("_", str(value)).replace("/", "_").replace("\\", "_").strip().strip(".")
    if name.upper() in {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}:
        name = f"{name}_"
    return name[:80] or "Мир"


def _unique_world_dir(saves: Path, name: str) -> Path:
    candidate = saves / name
    index = 2
    while os.path.lexists(candidate):
        candidate = saves / f"{name} ({index})"
        index += 1
    return candidate


def find_world_roots(root: Path) -> list[Path]:
    """Папки с level.dat (миры не вкладываются друг в друга)."""
    found: list[Path] = []

    def walk(directory: Path, depth: int) -> None:
        level = directory / "level.dat"
        if level.is_file() and not level.is_symlink():
            found.append(directory)
            return
        if depth >= MAX_SEARCH_DEPTH:
            return
        try:
            children = sorted(item for item in directory.iterdir() if item.is_dir() and not item.is_symlink())
        except OSError:
            return
        for child in children:
            if child.name in {"__MACOSX"}:
                continue
            walk(child, depth + 1)

    walk(root, 0)
    return found


def extract_archive(archive: Path, destination: Path) -> None:
    if zipfile.is_zipfile(archive):
        _extract_zip(archive, destination)
        return
    try:
        is_tar = tarfile.is_tarfile(archive)
    except OSError:
        is_tar = False
    if is_tar:
        _extract_tar(archive, destination)
        return
    raise LauncherError("Файл не похож на ZIP или TAR-архив либо повреждён")


def _extract_zip(archive: Path, destination: Path) -> None:
    total = 0
    try:
        with zipfile.ZipFile(archive) as source:
            members = source.infolist()
            if len(members) > MAX_FILES:
                raise LauncherError("В архиве слишком много файлов")
            for member in members:
                mode = (member.external_attr >> 16) & 0o170000
                if mode == 0o120000:
                    raise LauncherError("Архив мира содержит символьные ссылки")
                relative = safe_relative_path(member.filename)
                target = destination / Path(*relative.parts)
                if member.is_dir():
                    target.mkdir(mode=0o700, parents=True, exist_ok=True)
                    continue
                total += member.file_size
                if total > MAX_EXTRACTED_BYTES:
                    raise LauncherError("Распакованный мир слишком большой")
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                written = 0
                with source.open(member) as reader, open(target, "xb") as writer:
                    while True:
                        block = reader.read(1024 * 1024)
                        if not block:
                            break
                        written += len(block)
                        # Заголовок мог соврать о размере (zip-бомба).
                        if written > member.file_size + 1024 or total - member.file_size + written > MAX_EXTRACTED_BYTES:
                            raise LauncherError("Распакованный мир слишком большой")
                        writer.write(block)
    except FileExistsError as exc:
        raise LauncherError("В архиве повторяются файлы") from exc
    except (zipfile.BadZipFile, RuntimeError, NotImplementedError) as exc:
        raise LauncherError(f"Не удалось распаковать архив: {exc}") from exc


def _extract_tar(archive: Path, destination: Path) -> None:
    total = 0
    count = 0
    try:
        with tarfile.open(archive, "r:*") as source:
            for member in source:
                count += 1
                if count > MAX_FILES:
                    raise LauncherError("В архиве слишком много файлов")
                if member.issym() or member.islnk():
                    raise LauncherError("Архив мира содержит ссылки")
                if not (member.isdir() or member.isfile()):
                    continue
                relative = safe_relative_path(member.name)
                target = destination / Path(*relative.parts)
                if member.isdir():
                    target.mkdir(mode=0o700, parents=True, exist_ok=True)
                    continue
                total += member.size
                if total > MAX_EXTRACTED_BYTES:
                    raise LauncherError("Распакованный мир слишком большой")
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                reader = source.extractfile(member)
                if reader is None:
                    continue
                with reader, open(target, "xb") as writer:
                    shutil.copyfileobj(reader, writer, 1024 * 1024)
    except FileExistsError as exc:
        raise LauncherError("В архиве повторяются файлы") from exc
    except (tarfile.TarError, EOFError) as exc:
        raise LauncherError(f"Не удалось распаковать архив: {exc}") from exc
