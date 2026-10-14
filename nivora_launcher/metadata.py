from __future__ import annotations

import copy
import json
import re
from pathlib import Path
from typing import Any
from urllib.parse import quote

from .config import ConfigStore
from .constants import (
    MINECRAFT_ASSET_BASE_URL,
    MINECRAFT_LIBRARY_BASE_URL,
    VERSION_MANIFEST_URL,
    is_supported_version,
)
from .errors import LauncherError
from .net import HttpClient


class VersionRepository:
    def __init__(self, config: ConfigStore, http: HttpClient | None = None) -> None:
        self.config = config
        self.http = http or HttpClient()
        self.manifest_url = config.settings.data_api_url or VERSION_MANIFEST_URL
        self._manifest: dict[str, Any] | None = None
        self._versions: dict[str, dict[str, Any]] = {}

    def manifest(self, refresh: bool = False) -> dict[str, Any]:
        if self._manifest is None or refresh:
            value = self.http.get_json(self.manifest_url)
            if not isinstance(value, dict) or not isinstance(value.get("versions"), list):
                raise LauncherError("Mojang version manifest имеет неожиданный формат")
            self._manifest = value
        return self._manifest

    def list_versions(self, include_snapshots: bool = True) -> list[dict[str, Any]]:
        entries = [
            entry
            for entry in self.manifest().get("versions", [])
            if isinstance(entry, dict) and is_supported_version(str(entry.get("id", "")))
        ]
        if not include_snapshots:
            entries = [entry for entry in entries if entry.get("type") == "release"]
        return entries

    def find_version(self, version_id: str) -> dict[str, Any]:
        if not is_supported_version(version_id):
            raise LauncherError(f"Версия вне поддерживаемого диапазона 1.0–26.3: {version_id}")
        if version_id in self._versions:
            return self._versions[version_id]
        for entry in self.manifest().get("versions", []):
            if isinstance(entry, dict) and entry.get("id") == version_id:
                self._versions[version_id] = entry
                return entry
        raise LauncherError(f"Версия Minecraft не найдена в официальном manifest: {version_id}")

    def fetch_version_json(self, version_id: str) -> dict[str, Any]:
        entry = self.find_version(version_id)
        url = entry.get("url")
        if not url:
            raise LauncherError(f"У версии {version_id} отсутствует URL JSON")
        value = self.http.get_json(str(url))
        if not isinstance(value, dict):
            raise LauncherError(f"JSON версии {version_id} имеет неожиданный формат")
        return value

    def resolve(self, version_id: str, _seen: set[str] | None = None) -> dict[str, Any]:
        seen = _seen or set()
        if version_id in seen:
            raise LauncherError(f"Циклическое наследование version JSON: {version_id}")
        seen.add(version_id)
        child = self.fetch_version_json(version_id)
        parent_id = child.get("inheritsFrom")
        if not parent_id:
            return child
        parent = self.resolve(str(parent_id), seen)
        return merge_version_json(parent, child)

    def asset_url(self, sha1: str) -> str:
        return f"{MINECRAFT_ASSET_BASE_URL}/{sha1[:2]}/{sha1}"

    def library_url(self, library: dict[str, Any], classifier: str | None = None) -> str:
        downloads = library.get("downloads", {})
        if classifier:
            item = downloads.get("classifiers", {}).get(classifier)
            if item:
                if item.get("url"):
                    return str(item["url"])
                if item.get("path"):
                    base = str(library.get("url") or MINECRAFT_LIBRARY_BASE_URL).rstrip("/")
                    return f"{base}/{item['path']}"
            base = str(library.get("url") or MINECRAFT_LIBRARY_BASE_URL).rstrip("/")
            return f"{base}/{maven_path(str(library.get('name', '')), classifier)}"
        artifact = downloads.get("artifact")
        if artifact and artifact.get("url"):
            return str(artifact["url"])
        name = str(library.get("name", ""))
        path = maven_path(name, classifier)
        base = str(library.get("url") or MINECRAFT_LIBRARY_BASE_URL).rstrip("/")
        return f"{base}/{path}"


def merge_version_json(parent: dict[str, Any], child: dict[str, Any]) -> dict[str, Any]:
    """Merge Mojang's inheritance format without mutating either input."""
    result = copy.deepcopy(parent)
    for key, value in child.items():
        if key == "libraries":
            result["libraries"] = _merge_named_list(parent.get("libraries", []), value)
        elif key == "arguments" and isinstance(value, dict):
            result["arguments"] = _merge_arguments(parent.get("arguments", {}), value)
        elif key in {"downloads", "logging"} and isinstance(value, dict):
            result[key] = _deep_merge(parent.get(key, {}), value)
        else:
            result[key] = copy.deepcopy(value)
    return result


def _merge_named_list(parent: Any, child: Any) -> list[Any]:
    merged: list[Any] = copy.deepcopy(parent) if isinstance(parent, list) else []
    if not isinstance(child, list):
        return merged
    indexes = {item.get("name"): index for index, item in enumerate(merged) if isinstance(item, dict)}
    for item in child:
        if isinstance(item, dict) and item.get("name") in indexes:
            merged[indexes[item["name"]]] = copy.deepcopy(item)
        else:
            if isinstance(item, dict):
                indexes[item.get("name")] = len(merged)
            merged.append(copy.deepcopy(item))
    return merged


def _merge_arguments(parent: Any, child: dict[str, Any]) -> dict[str, Any]:
    result = copy.deepcopy(parent) if isinstance(parent, dict) else {}
    for key, value in child.items():
        if isinstance(value, list) and isinstance(result.get(key), list):
            result[key] = result[key] + copy.deepcopy(value)
        else:
            result[key] = copy.deepcopy(value)
    return result


def _deep_merge(parent: Any, child: Any) -> Any:
    if not isinstance(parent, dict) or not isinstance(child, dict):
        return copy.deepcopy(child)
    result = copy.deepcopy(parent)
    for key, value in child.items():
        result[key] = _deep_merge(result[key], value) if key in result else copy.deepcopy(value)
    return result


def maven_path(name: str, classifier: str | None = None) -> str:
    parts = name.split(":")
    if len(parts) < 3:
        raise LauncherError(f"Некорректная Maven library coordinate: {name}")
    group, artifact, version = parts[:3]
    if (
        any(not part or not re.fullmatch(r"[A-Za-z0-9._-]+", part) or ".." in part for part in (group, artifact))
        or not version
        or not re.fullmatch(r"[A-Za-z0-9._+-]+", version)
        or ".." in version
    ):
        raise LauncherError(f"Небезопасная Maven library coordinate: {name}")
    if classifier is None and len(parts) >= 4:
        classifier = parts[3]
    if classifier is not None and (not classifier or not re.fullmatch(r"[A-Za-z0-9._-]+", classifier)):
        raise LauncherError(f"Небезопасный classifier Maven library: {classifier}")
    suffix = f"-{classifier}" if classifier else ""
    return f"{group.replace('.', '/')}/{artifact}/{version}/{artifact}-{version}{suffix}.jar"


def safe_instance_name(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9._-]+", "-", value).strip(".-")
    return quote(cleaned or "default", safe="._-")[:64]
