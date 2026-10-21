"""дерево зависимостей модов по манифестам внутри jar-файлов."""
from __future__ import annotations

import json
import re
import zipfile
from pathlib import Path
from typing import Any

from .errors import LauncherError
from .platform_support import is_reparse_point

MAX_MANIFEST_BYTES = 256 * 1024
NOISE_IDS = frozenset({
    "minecraft", "fabricloader", "fabric-loader", "fabric", "quilt_loader", "quilt-loader",
    "quilt_base", "quilted_fabric_api", "forge", "neoforge", "minecraftforge", "java",
    "cloth-config", "cloth_config", "architectury",
})
MAX_MODS = 600

def _read_zip_member(archive: zipfile.ZipFile, name: str) -> bytes:
    try:
        info = archive.getinfo(name)
        if info.is_dir() or info.file_size > MAX_MANIFEST_BYTES:
            return b""
        return archive.read(info)
    except (KeyError, OSError, RuntimeError, ValueError, zipfile.BadZipFile):
        return b""

def _fabric_manifest(raw: bytes) -> dict[str, Any] | None:
    try:
        value = json.loads(raw.decode("utf-8", errors="replace"))
    except (ValueError, TypeError):
        return None
    if not isinstance(value, dict):
        return None
    depends = value.get("depends") if isinstance(value.get("depends"), dict) else {}
    suggests = value.get("suggests") if isinstance(value.get("suggests"), dict) else {}
    breaks = value.get("breaks") if isinstance(value.get("breaks"), dict) else {}

    def entries(mapping: dict[str, Any], optional: bool) -> list[dict[str, Any]]:
        result = []
        for dep_id, constraint in mapping.items():
            text = constraint if isinstance(constraint, str) else "любая"
            result.append({"id": str(dep_id), "constraint": str(text)[:80], "optional": optional})
        return result

    return {
        "mod_id": str(value.get("id", "")),
        "name": str(value.get("name", "") or value.get("id", "")),
        "version": str(value.get("version", "")),
        "loader": "fabric",
        "dependencies": entries(depends, False) + entries(suggests, True),
        "breaks": [str(key) for key in breaks.keys()],
    }

def _quilt_manifest(raw: bytes) -> dict[str, Any] | None:
    try:
        value = json.loads(raw.decode("utf-8", errors="replace"))
    except (ValueError, TypeError):
        return None
    loader = value.get("quilt_loader") if isinstance(value, dict) and isinstance(value.get("quilt_loader"), dict) else None
    if not loader:
        return None
    metadata = loader.get("metadata") if isinstance(loader.get("metadata"), dict) else {}
    dependencies: list[dict[str, Any]] = []
    for item in loader.get("depends", []) if isinstance(loader.get("depends"), list) else []:
        if isinstance(item, str):
            dependencies.append({"id": item, "constraint": "любая", "optional": False})
        elif isinstance(item, dict) and item.get("id"):
            constraint = item.get("versions")
            text = ",".join(str(entry) for entry in constraint) if isinstance(constraint, list) else str(constraint or "любая")
            dependencies.append({"id": str(item["id"]), "constraint": text[:80], "optional": str(item.get("optional", "")).lower() == "true" or bool(item.get("unless") and not constraint)})
    return {
        "mod_id": str(loader.get("id", "")),
        "name": str(metadata.get("name", "") or loader.get("id", "")),
        "version": str(loader.get("version", "")),
        "loader": "quilt",
        "dependencies": dependencies,
        "breaks": [],
    }

def _toml_manifest(raw: bytes) -> dict[str, Any] | None:
    text = raw.decode("utf-8", errors="replace")
    mod_id = re.search(r"(?m)^modId\s*=\s*[\"']([\w.-]+)", text)
    if not mod_id:
        return None
    display = re.search(r"(?m)^displayName\s*=\s*[\"']([^\"']+)", text)
    version = re.search(r"(?m)^version\s*=\s*[\"']([^\"']+)", text)
    loader = "neoforge" if "neoforge" in text.lower() else "forge"
    dependencies: list[dict[str, Any]] = []
    for block in re.finditer(r"(?ms)\[\[dependencies\.[\w.-]+\]\](.*?)(?=\n\[|\Z)", text):
        body = block.group(1)
        dependency_id = re.search(r"modId\s*=\s*[\"']([\w.-]+)", body)
        if not dependency_id:
            continue
        kind = re.search(r"type\s*=\s*[\"'](\w+)", body)
        mandatory = re.search(r"mandatory\s*=\s*(\w+)", body)
        dependencies.append(
            {
                "id": dependency_id.group(1),
                "constraint": (version_range.group(1) if (version_range := re.search(r"versionRange\s*=\s*[\"']([^\"']*)", body)) else "")[:80],
                "optional": (kind.group(1).lower() != "required") if kind else (mandatory.group(1).lower() != "true" if mandatory else False),
            }
        )
    return {
        "mod_id": mod_id.group(1),
        "name": (display.group(1) if display else mod_id.group(1)),
        "version": version.group(1) if version else "",
        "loader": loader,
        "dependencies": dependencies,
        "breaks": [],
    }

def _mcmod_manifest(raw: bytes) -> dict[str, Any] | None:
    try:
        value = json.loads(raw.decode("utf-8", errors="replace"))
    except (ValueError, TypeError):
        return None
    item = value[0] if isinstance(value, list) and value else value
    if not isinstance(item, dict):
        return None
    dependencies: list[dict[str, Any]] = []
    for token in item.get("useDependencies", []) if isinstance(item.get("useDependencies"), list) else []:
        text = str(token)
        match = re.match(r"([A-Za-z][\w.-]*)", text)
        if match:
            dependencies.append({"id": match.group(1), "constraint": text[:80], "optional": text.startswith("after:") or text.startswith("after-")})
    return {
        "mod_id": str(item.get("modid", "") or item.get("name", "")),
        "name": str(item.get("name", "")),
        "version": str(item.get("version", "")),
        "loader": "forge",
        "dependencies": dependencies,
        "breaks": [],
    }

def read_mod_manifest(path: Path) -> dict[str, Any] | None:
    if path.is_symlink() or is_reparse_point(path) or not path.is_file():
        return None
    suffix = path.suffix.lower()
    if suffix not in {".jar", ".zip"}:
        return None
    try:
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            for name, parser in (
                ("fabric.mod.json", _fabric_manifest),
                ("quilt.mod.json", _quilt_manifest),
                ("META-INF/mods.toml", _toml_manifest),
                ("META-INF/neoforge.mods.toml", _toml_manifest),
                ("mcmod.info", _mcmod_manifest),
            ):
                if name in names:
                    manifest = parser(_read_zip_member(archive, name))
                    if manifest and manifest.get("mod_id"):
                        return manifest
    except (OSError, ValueError, zipfile.BadZipFile):
        return None
    return None

def _sidecar(path: Path) -> dict[str, Any]:
    sidecar_path = path.with_name(f".{path.name}.trial-project.json")
    try:
        if sidecar_path.is_symlink() or not sidecar_path.is_file() or sidecar_path.stat().st_size > 64 * 1024:
            return {}
        value = json.loads(sidecar_path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError, TypeError):
        return {}

def dependency_graph(mods_directory: Path) -> dict[str, Any]:
    if mods_directory.is_symlink() or not mods_directory.is_dir():
        raise LauncherError("Каталог модов недоступен")
    nodes: list[dict[str, Any]] = []
    by_id: dict[str, dict[str, Any]] = {}
    by_file: dict[str, dict[str, Any]] = {}
    for path in sorted(mods_directory.iterdir(), key=lambda item: item.name.lower()):
        if len(nodes) >= MAX_MODS:
            break
        if not path.is_file():
            continue
        disabled = path.name.endswith(".disabled")
        manifest = read_mod_manifest(path)
        metadata = _sidecar(path)
        mod_id = (manifest or {}).get("mod_id") or metadata.get("project_id") or path.stem.lower()
        node = {
            "id": mod_id,
            "file": path.name,
            "display_name": (manifest or {}).get("name") or metadata.get("display_name") or path.stem,
            "version": (manifest or {}).get("version") or metadata.get("version") or "",
            "enabled": not disabled,
            "source": metadata.get("source", "Локальный файл"),
            "loader": (manifest or {}).get("loader", ""),
            "dependencies": (manifest or {}).get("dependencies", []),
        }
        nodes.append(node)
        by_id.setdefault(mod_id.lower(), node)
        by_file[path.name.lower()] = node

    edges: list[dict[str, Any]] = []
    missing: list[dict[str, Any]] = []
    seen_missing: set[str] = set()
    for node in nodes:
        for dependency in node.get("dependencies", []):
            dependency_id = str(dependency.get("id", "")).strip().lower()
            if not dependency_id or dependency_id in NOISE_IDS or dependency_id in node["id"].lower():
                continue
            target = by_id.get(dependency_id) or by_file.get(f"{dependency_id}.jar")
            if target:
                edges.append({"from": node["id"], "to": target["id"], "optional": bool(dependency.get("optional")), "constraint": dependency.get("constraint", "")})
            elif dependency_id not in seen_missing:
                seen_missing.add(dependency_id)
                missing.append({"id": dependency_id, "required_by": node["id"], "optional": bool(dependency.get("optional"))})
    return {"nodes": nodes, "edges": edges, "missing": missing}

def removal_impact(graph: dict[str, Any], mod_id: str) -> dict[str, Any]:
    """Что сломается, если удалить мод: обратный обход по рёбрам."""
    reverse: dict[str, list[str]] = {}
    for edge in graph.get("edges", []):
        reverse.setdefault(edge["to"].lower(), []).append(edge["from"])
    broken: list[str] = []
    pending = [str(mod_id).lower()]
    seen = set(pending)
    while pending:
        current = pending.pop()
        for dependent in reverse.get(current, []):
            if dependent not in seen:
                seen.add(dependent)
                broken.append(dependent)
                pending.append(dependent)
    return {"mod_id": mod_id, "breaks": broken}
