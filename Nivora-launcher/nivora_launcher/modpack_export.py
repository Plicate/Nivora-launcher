"""Экспорт экземпляра в формат Modrinth (.mrpack).

Моды с Modrinth-метаданными попадают в files[] с URL и sha1 (запрашиваем у
каталога по version_id), всё остальное — в overrides/. Так модпак остаётся
обновляемым у получателя, а локальные моды не теряются.
"""
from __future__ import annotations

import hashlib
import json
import re
import tempfile
import time
import zipfile
from pathlib import Path
from typing import Any

from .errors import LauncherError
from .platform_support import is_reparse_point

MRPACK_FORMAT_VERSION = 1
MAX_EXPORT_FILES = 20_000
MAX_EXPORT_BYTES = 4 * 1024 * 1024 * 1024
OVERRIDE_ROOTS = ("config", "defaultconfigs", "shaderpacks", "resourcepacks", "scripts", "kubejs", "journeymap")
OVERRIDE_FILES = ("options.txt", "servers.dat", "servers.dat_old")
EXPORT_DIR_NAME = "exports"


def _loader_from_version(version_id: str, loader: str) -> str:
    if loader != "vanilla":
        return loader
    lowered = version_id.lower()
    for name in ("neoforge", "forge", "quilt", "fabric"):
        if name in lowered:
            return name
    return "vanilla"


def _loader_version(version_id: str, minecraft_version: str = "") -> str:
    value = version_id
    if minecraft_version:
        value = re.sub(rf"-{re.escape(minecraft_version)}$", "", value, flags=re.IGNORECASE)
    for pattern in (
        r"fabric-loader-([0-9][\w.+-]*)",
        r"quilt-loader-([0-9][\w.+-]*)",
        r"forge-([0-9][\w.+-]*)",
        r"neoforge-?([0-9][\w.+-]*)",
    ):
        match = re.search(pattern, value, re.IGNORECASE)
        if match:
            return match.group(1)
    return ""


def _sha1(path: Path) -> str:
    digest = hashlib.sha1()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _version_download(http: Any, base_url: str, version_id: str, file_name: str) -> dict[str, Any] | None:
    try:
        value = http.get_json(f"{base_url.rstrip('/')}/version/{version_id}")
        files = value.get("files", []) if isinstance(value, dict) else []
        primary = next((item for item in files if isinstance(item, dict) and item.get("primary")), None)
        candidate = primary or next((item for item in files if isinstance(item, dict)), None)
        if not candidate:
            return None
        url = str(candidate.get("url", ""))
        hashes = candidate.get("hashes") if isinstance(candidate.get("hashes"), dict) else {}
        sha1 = str(hashes.get("sha1", ""))
        if not url.startswith("https://") or not sha1:
            return None
        return {"url": url, "sha1": sha1, "size": int(candidate.get("size") or 0), "primary_file_name": str(candidate.get("filename") or "")}
    except Exception:
        return None


def export_instance_as_mrpack(
    http: Any,
    instance: Any,
    destination_dir: Path,
    *,
    display_name: str = "",
    version_label: str = "",
) -> dict[str, Any]:
    if getattr(instance, "game_edition", "java") != "java":
        raise LauncherError("Экспорт модпака поддерживается только для Java-экземпляров")
    loader = _loader_from_version(instance.version_id, instance.loader)
    if loader == "vanilla":
        raise LauncherError("Ванильный экземпляр нечего экспортировать как модпак — установите загрузчик или моды")
    minecraft_version = instance.minecraft_version or ""
    if not minecraft_version:
        raise LauncherError("Не удалось определить версию Minecraft экземпляра")

    mods_dir = instance.path / "mods"
    indexed: list[dict[str, Any]] = []
    override_files: list[Path] = []
    if mods_dir.is_dir() and not mods_dir.is_symlink():
        for path in sorted(mods_dir.iterdir(), key=lambda item: item.name.lower()):
            if not path.is_file() or path.name.endswith(".disabled") or path.name.startswith("."):
                continue
            if path.name.endswith(".jar") or path.name.endswith(".zip"):
                metadata = _read_sidecar(path)
                catalog_url = str(metadata.get("catalog_url") or "https://api.modrinth.com")
                version_id = str(metadata.get("version_id", ""))
                resolved = _version_download(http, catalog_url, version_id, path.name) if version_id else None
                if resolved and resolved["primary_file_name"] == path.name:
                    indexed.append(
                        {
                            "path": f"mods/{path.name}",
                            "hashes": {"sha1": resolved["sha1"]},
                            "env": {"client": "required", "server": "unsupported"},
                            "downloads": [resolved["url"]],
                            "file_size": resolved["size"],
                        }
                    )
                else:
                    override_files.append(path)

    for root_name in OVERRIDE_ROOTS:
        root = instance.path / root_name
        if root.is_dir() and not root.is_symlink():
            override_files.extend(path for path in root.rglob("*") if path.is_file() and not path.is_symlink())
    for file_name in OVERRIDE_FILES:
        path = instance.path / file_name
        if path.is_file() and not path.is_symlink():
            override_files.append(path)

    if not indexed and not override_files:
        raise LauncherError("В экземпляре нет модов и настроек для экспорта")

    dependencies: dict[str, str] = {"minecraft": minecraft_version}
    loader_version = _loader_version(instance.version_id, minecraft_version)
    loader_key = {"fabric": "fabric-loader", "quilt": "quilt-loader", "forge": "forge", "neoforge": "neoforge"}.get(loader)
    if loader_key and loader_version:
        dependencies[loader_key] = loader_version

    name = display_name or instance.display_name or instance.name
    index = {
        "formatVersion": MRPACK_FORMAT_VERSION,
        "game": "minecraft",
        "versionId": version_label or time.strftime("%Y.%m.%d-%H%M"),
        "name": name,
        "summary": (instance.description or "")[:256],
        "files": indexed,
        "dependencies": dependencies,
    }

    destination_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    archive_name = re.sub(r"[^A-Za-z0-9._-]+", "-", name).strip(".-")[:64] or instance.name
    safe_name = f"{archive_name}-{index['versionId']}.mrpack"
    destination = destination_dir / safe_name
    total = 0
    files_count = 0
    with tempfile.NamedTemporaryFile(prefix=".export-", suffix=".part", dir=destination_dir, delete=False) as temporary:
        temporary_path = Path(temporary.name)
    try:
        with zipfile.ZipFile(temporary_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
            archive.writestr("modrinth.index.json", json.dumps(index, ensure_ascii=False, indent=2))
            files_count += 1
            for path in override_files:
                if files_count >= MAX_EXPORT_FILES or total > MAX_EXPORT_BYTES:
                    raise LauncherError("Экземпляр слишком велик для экспорта (лимит 4 ГиБ)")
                size = path.stat().st_size
                total += size
                files_count += 1
                archive.write(path, f"overrides/{path.relative_to(instance.path)}")
        temporary_path.replace(destination)
    finally:
        if temporary_path.exists():
            temporary_path.unlink()
    return {
        "path": str(destination),
        "file_name": destination.name,
        "size_bytes": destination.stat().st_size,
        "indexed_files": len(indexed),
        "override_files": len(override_files),
    }


def _read_sidecar(path: Path) -> dict[str, Any]:
    sidecar = path.with_name(f".{path.name}.trial-project.json")
    try:
        if sidecar.is_symlink() or not sidecar.is_file() or sidecar.stat().st_size > 64 * 1024 or is_reparse_point(sidecar):
            return {}
        value = json.loads(sidecar.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError, TypeError):
        return {}
