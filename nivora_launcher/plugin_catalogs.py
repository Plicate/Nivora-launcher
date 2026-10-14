from __future__ import annotations

import hashlib
import json
import os
import re
import stat
import tempfile
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlencode

from .errors import LauncherError
from .mods import _safe_file_name
from .net import HttpClient, file_hash, verify_file
from .operations import OperationCancelled
from .platform_support import set_private_fd


DEFAULT_PLUGIN_CATALOGS = [
    {"id": "modrinth-plugins", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2", "enabled": True},
    {"id": "spigot", "name": "Spigot", "kind": "spigot", "api_url": "https://api.spiget.org/v2", "enabled": True},
    {"id": "hangar", "name": "Hangar", "kind": "hangar", "api_url": "https://hangar.papermc.io/api/v1", "enabled": True},
]
PLUGIN_SORT_MODES = frozenset({"relevance", "downloads", "newest", "updated"})
PLUGIN_METADATA_SCHEMA = 1
PLUGIN_METADATA_MAX_BYTES = 64 * 1024
MAX_PLUGIN_DEPENDENCY_PROJECTS = 64
MAX_PLUGIN_DEPENDENCY_DEPTH = 16
MAX_PLUGIN_DEPENDENCIES_PER_PROJECT = 100
UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE = "[unverified-download] Каталог не предоставил доверенный хеш плагина. Подтвердите скачивание явно."


@dataclass
class PluginCatalogResult:
    source: str
    project_id: str
    title: str
    description: str
    author: str
    icon_url: str
    downloads: int
    version: str = ""
    download_url: str = ""
    file_name: str = ""
    sha1: str = ""
    sha256: str = ""
    sha512: str = ""
    size: int | None = None
    created_at: str = ""
    updated_at: str = ""
    dependencies: list[dict[str, Any]] = field(default_factory=list)
    version_id: str = ""
    _downloads_known: bool = field(default=True, repr=False, compare=False)

    def as_dict(self) -> dict[str, Any]:
        return {
            "source": self.source,
            "project_id": self.project_id,
            "title": self.title,
            "description": self.description,
            "author": self.author,
            "icon_url": self.icon_url,
            "downloads": self.downloads,
            "version": self.version,
            "download_url": self.download_url,
            "file_name": self.file_name,
            "sha1": self.sha1,
            "sha256": self.sha256,
            "sha512": self.sha512,
            "size": self.size,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "dependencies": self.dependencies,
            "version_id": self.version_id,
        }


class PluginCatalog:
    def __init__(self, http: HttpClient, catalog: dict[str, Any]) -> None:
        self.http = http
        self.catalog = catalog
        self.kind = str(catalog.get("kind", "custom")).lower()
        self.base_url = str(catalog.get("api_url", "")).rstrip("/")

    def search(
        self,
        query: str,
        core: str,
        version: str,
        limit: int = 12,
        *,
        sort_by: str = "relevance",
    ) -> list[PluginCatalogResult]:
        limit = max(1, min(30, int(limit)))
        sort = _validate_sort(sort_by)
        query = query.strip()
        if self.kind == "modrinth":
            facets = [["project_type:plugin"]]
            if version:
                facets.append([f"versions:{version}"])
            if core and core not in {"custom", "bukkit"}:
                facets.append([f"categories:{core}"])
            params = {
                "limit": str(limit),
                "facets": json.dumps(facets, separators=(",", ":")),
                "index": sort,
            }
            if query:
                params["query"] = query
            value = self.http.get_json(f"{self.base_url}/search?{urlencode(params)}")
            hits = value.get("hits", []) if isinstance(value, dict) else []
            results = [_modrinth_result(item) for item in hits if isinstance(item, dict)]
            return _sort_results(results, sort)[:limit]
        if self.kind == "spigot":
            params: dict[str, Any] = {"size": limit}
            provider_sort = _provider_sort(self.kind, sort)
            if provider_sort:
                params["sort"] = provider_sort
            endpoint = f"{self.base_url}/search/resources/{quote(query or 'plugin', safe='')}?{urlencode(params)}"
            value = self.http.get_json(endpoint)
            results = [_spigot_result(item) for item in value if isinstance(item, dict)] if isinstance(value, list) else []
            return _sort_results(results, sort)[:limit]
        if self.kind == "hangar":
            params: dict[str, Any] = {"query": query, "limit": limit}
            provider_sort = _provider_sort(self.kind, sort)
            if provider_sort:
                params["sort"] = provider_sort
            value = self.http.get_json(f"{self.base_url}/projects?{urlencode(params)}")
            rows = value.get("result", value.get("projects", [])) if isinstance(value, dict) else value
            results = [_hangar_result(item) for item in rows if isinstance(item, dict)] if isinstance(rows, list) else []
            return _sort_results(results, sort)[:limit]
        value = self.http.get_json(f"{self.base_url}?{urlencode({'query': query, 'limit': limit, 'core': core, 'version': version})}")
        rows = value.get("results", value) if isinstance(value, dict) else value
        results = [_custom_result(self.kind, item) for item in rows if isinstance(item, dict)] if isinstance(rows, list) else []
        return _sort_results(results, sort)[:limit]

    def details(self, project_id: str, core: str = "", version: str = "") -> dict[str, Any]:
        project_id = str(project_id).strip()
        if not project_id:
            raise LauncherError("Укажите идентификатор плагина")
        if self.kind == "modrinth":
            details = self._modrinth_project_details(project_id)
            result = _modrinth_result(details, project_id)
            candidate = self._resolve_candidate(project_id, core, version, project_details=details)
        elif self.kind == "spigot":
            value = self.http.get_json(f"{self.base_url}/resources/{quote(project_id, safe='')}")
            if not isinstance(value, dict):
                raise LauncherError("Spigot не вернул сведения о плагине")
            result = _spigot_result(value, project_id)
            candidate = self._spigot_candidate(project_id, value, core, version)
        elif self.kind == "hangar":
            value = self.http.get_json(f"{self.base_url}/projects/{_hangar_project_path(project_id)}")
            result = _hangar_result(value, project_id) if isinstance(value, dict) else _empty_result(self.kind, project_id)
            candidate = self._resolve_candidate(project_id, core, version)
        else:
            matches = self.search(project_id, core, version, 30)
            matches = _sort_results([item for item in matches if item.project_id == project_id], "updated")
            result = matches[0] if matches else None
            if result is None:
                raise LauncherError("Каталог не вернул сведения о плагине")
            candidate = _candidate_from_result(result, compatible=bool(core and version))
        if candidate:
            _apply_candidate(result, candidate)
        return result.as_dict()

    def install(self, project_id: str, target: Path, version: str, core: str, *, confirm_unverified: bool = False) -> dict[str, Any]:
        project_id = str(project_id).strip()
        if not project_id:
            raise LauncherError("Укажите идентификатор плагина")
        candidate = self._resolve_candidate(project_id, core, version)
        if not candidate or not candidate.get("download_url"):
            raise LauncherError("Каталог не вернул файл плагина")
        if not candidate.get("compatible"):
            raise LauncherError("Каталог не подтвердил совместимость плагина с выбранным ядром и версией Minecraft")
        if not confirm_unverified and not _candidate_has_trusted_hash(candidate):
            raise LauncherError(UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE)
        candidates = self._resolve_install_plan(project_id, candidate, core, version)
        if not confirm_unverified and any(not _candidate_has_trusted_hash(item) for item in candidates):
            raise LauncherError(UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE)
        target = _plugin_directory(target)
        return self._install_candidate_plan(
            candidates,
            project_id,
            target,
            core,
            version,
            confirm_unverified=confirm_unverified,
        )

    def _resolve_install_plan(
        self,
        project_id: str,
        root_candidate: dict[str, Any],
        core: str,
        version: str,
    ) -> list[dict[str, Any]]:
        selected: dict[str, dict[str, Any]] = {}
        order: list[str] = []

        def visit(
            current_id: str,
            pinned_version_id: str,
            depth: int,
            candidate: dict[str, Any] | None = None,
        ) -> None:
            current_id = _plugin_project_id(current_id)
            pinned_version_id = _plugin_version_id(pinned_version_id)
            existing = selected.get(current_id)
            if existing is not None:
                selected_version_id = str(existing.get("version_id") or "")
                if pinned_version_id and pinned_version_id != selected_version_id:
                    raise LauncherError(f"Зависимости требуют разные версии проекта {current_id}")
                return
            if depth > MAX_PLUGIN_DEPENDENCY_DEPTH:
                raise LauncherError("Цепочка зависимостей серверных плагинов слишком глубокая")
            if len(selected) >= MAX_PLUGIN_DEPENDENCY_PROJECTS:
                raise LauncherError("В дереве зависимостей серверных плагинов слишком много проектов")

            current = candidate or self._resolve_candidate(
                current_id,
                core,
                version,
                pinned_version_id=pinned_version_id,
            )
            if not current:
                raise LauncherError(f"Каталог не нашёл обязательную зависимость {current_id}")
            returned_id = str(current.get("project_id") or current_id).strip()
            if returned_id != current_id:
                raise LauncherError(f"Каталог вернул сведения о другом проекте вместо {current_id}")
            if not current.get("compatible"):
                raise LauncherError(f"Каталог не подтвердил совместимость обязательной зависимости {current_id}")
            if not current.get("download_url"):
                raise LauncherError(f"Каталог не вернул файл обязательной зависимости {current_id}")
            selected_version_id = str(current.get("version_id") or "")
            if pinned_version_id and pinned_version_id != selected_version_id:
                raise LauncherError(f"Каталог не разрешил закреплённую версию {pinned_version_id} проекта {current_id}")

            selected[current_id] = current
            order.append(current_id)
            for dependency_id, dependency_version_id in self._required_dependencies(current):
                visit(dependency_id, dependency_version_id, depth + 1)

        visit(project_id, "", 0, root_candidate)
        return [selected[item] for item in order]

    def _required_dependencies(self, candidate: dict[str, Any]) -> list[tuple[str, str]]:
        raw = candidate.get("dependencies", [])
        if not isinstance(raw, list):
            return []
        if len(raw) > MAX_PLUGIN_DEPENDENCIES_PER_PROJECT:
            raise LauncherError("У серверного плагина слишком много зависимостей")

        result: list[tuple[str, str]] = []
        for item in raw:
            if not isinstance(item, dict) or item.get("optional") is True:
                continue
            dependency_type = str(item.get("dependency_type") or "").strip().lower()
            if dependency_type:
                required = dependency_type == "required"
            elif "required" in item:
                required = item.get("required") is True
            elif "optional" in item:
                required = item.get("optional") is False
            else:
                required = False
            if not required:
                continue

            dependency_id = str(item.get("project_id") or "").strip()
            pinned_version_id = _plugin_version_id(item.get("version_id"))
            if not dependency_id and self.kind == "modrinth" and pinned_version_id:
                dependency_id = self._modrinth_project_id_for_version(pinned_version_id)
            if not dependency_id:
                continue
            result.append((_plugin_project_id(dependency_id), pinned_version_id))
        return result

    def _modrinth_project_id_for_version(self, version_id: str) -> str:
        value = self.http.get_json(f"{self.base_url}/version/{quote(version_id, safe='')}")
        if not isinstance(value, dict):
            raise LauncherError(f"Не удалось определить проект для версии зависимости {version_id}")
        returned_version_id = str(value.get("id") or value.get("version_id") or "").strip()
        if returned_version_id and returned_version_id != version_id:
            raise LauncherError("Modrinth вернул другую версию зависимости")
        project_id = str(value.get("project_id") or "").strip()
        if not project_id:
            raise LauncherError(f"Не удалось определить проект для версии зависимости {version_id}")
        return _plugin_project_id(project_id)

    def _install_candidate_plan(
        self,
        candidates: list[dict[str, Any]],
        root_project_id: str,
        target: Path,
        core: str,
        version: str,
        *,
        confirm_unverified: bool,
    ) -> dict[str, Any]:
        staged: list[tuple[Path, Path, dict[str, Any]]] = []
        reserved_names: set[str] = set()
        with tempfile.TemporaryDirectory(prefix=".trial-plugin-install-", dir=target) as temporary:
            staging = Path(temporary)
            for candidate in candidates:
                project_id = str(candidate.get("project_id") or "plugin")
                file_name = _safe_jar_file_name(str(candidate.get("file_name") or f"{project_id}.jar"))
                destination = _unique_plugin_file(target, file_name, reserved_names)
                reserved_names.add(destination.name)
                staged_jar = staging / destination.name
                result = self._download_candidate(
                    candidate,
                    staging,
                    staged_jar,
                    core,
                    version,
                    confirm_unverified=confirm_unverified,
                )
                staged.append((staged_jar, destination, result))

            root_entry = next(
                ((destination, result) for _, destination, result in staged if result.get("project_id") == root_project_id),
                None,
            )
            if root_entry is None:
                raise LauncherError("Не удалось определить установленный основной плагин")
            self._publish_staged_plugins(staged, target)
            destination, root_result = root_entry
            root_result["path"] = str(destination)
            return root_result

    def _publish_staged_plugins(self, staged: list[tuple[Path, Path, dict[str, Any]]], target: Path) -> None:
        committed: list[Path] = []
        try:
            _plugin_directory(target, create=False)
            for staged_jar, destination, _result in staged:
                staged_metadata = _plugin_metadata_path(staged_jar)
                destination_metadata = _plugin_metadata_path(destination)
                for source, final in ((staged_metadata, destination_metadata), (staged_jar, destination)):
                    try:
                        os.link(source, final, follow_symlinks=False)
                    except FileExistsError as exc:
                        raise LauncherError("Файл плагина был создан во время установки") from exc
                    committed.append(final)
                    source.unlink()
        except BaseException as exc:
            rollback_errors: list[OSError] = []
            for path in reversed(committed):
                try:
                    if os.path.lexists(path):
                        path.unlink()
                except OSError as rollback_error:
                    rollback_errors.append(rollback_error)
            if rollback_errors:
                raise LauncherError("Не удалось полностью отменить установку серверных плагинов") from exc
            raise

    def installed(self, target: Path) -> list[dict[str, Any]]:
        return [entry for entry in installed_plugin_metadata_index(target) if self._matches_catalog(entry)]

    def check_updates(self, target: Path, core: str, version: str) -> list[dict[str, Any]]:
        _validate_update_context(core, version)
        updates: list[dict[str, Any]] = []
        for entry in self.installed(target):
            candidate = self._resolve_candidate(entry["project_id"], core, version)
            if not candidate or not candidate.get("compatible") or not _is_update(entry, candidate):
                continue
            updates.append(
                {
                    "source": self.kind,
                    "catalog_id": entry["catalog_id"],
                    "project_id": entry["project_id"],
                    "file_name": entry["file_name"],
                    "path": entry["path"],
                    "current_version": str(entry.get("version", "")),
                    "current_version_id": str(entry.get("version_id", "")),
                    "version": str(candidate.get("version", "")),
                    "version_id": str(candidate.get("version_id", "")),
                    "updated_at": str(candidate.get("updated_at", "")),
                    "trusted_hash": bool(candidate.get("trusted_hash")),
                    "dependencies": candidate.get("dependencies", []),
                }
            )
        return updates

    def update_installed(
        self,
        target: Path,
        file_name: str,
        core: str,
        version: str,
        *,
        confirm_unverified: bool = False,
    ) -> dict[str, Any]:
        _validate_update_context(core, version)
        file_name = _safe_jar_file_name(file_name)
        entry = next((item for item in self.installed(target) if item["file_name"] == file_name), None)
        if entry is None:
            raise LauncherError("Можно обновлять только JAR, установленный из этого каталога")
        candidate = self._resolve_candidate(entry["project_id"], core, version)
        if not candidate or not candidate.get("compatible") or not candidate.get("download_url"):
            raise LauncherError("Каталог не подтвердил совместимую версию плагина для этого ядра и Minecraft")
        if not _is_update(entry, candidate):
            return {
                **entry,
                "updated": False,
                "current_version": str(entry.get("version", "")),
                "latest_version": str(candidate.get("version", "")),
            }
        target = _plugin_directory(target, create=False)
        destination = target / file_name
        if destination.is_symlink() or not destination.is_file():
            raise LauncherError("Установленный JAR отсутствует или является символьной ссылкой")
        result = self._download_candidate(
            candidate,
            target,
            destination,
            core,
            version,
            confirm_unverified=confirm_unverified,
            replace=True,
            expected_entry=entry,
        )
        result["updated"] = True
        return result

    def _resolve_candidate(
        self,
        project_id: str,
        core: str,
        version: str,
        *,
        project_details: dict[str, Any] | None = None,
        pinned_version_id: str = "",
    ) -> dict[str, Any] | None:
        if self.kind == "modrinth":
            details = project_details or self._modrinth_project_details(project_id)
            if pinned_version_id:
                item = self.http.get_json(f"{self.base_url}/version/{quote(pinned_version_id, safe='')}")
                if not isinstance(item, dict):
                    return None
                returned_version_id = str(item.get("id") or item.get("version_id") or "").strip()
                if returned_version_id != pinned_version_id:
                    raise LauncherError("Modrinth вернул другую версию обязательной зависимости")
                returned_project_id = str(item.get("project_id") or "").strip()
                if returned_project_id != project_id:
                    raise LauncherError(f"Modrinth вернул версию другого проекта вместо {project_id}")
                candidate = _modrinth_candidate(project_id, details, item, core, version)
                candidate["compatible"] = _modrinth_plugin_version_compatible(item, core, version)
                return candidate
            versions = self._modrinth_versions(project_id, core, version)
            if not versions:
                return None
            item = _latest_item(versions, ("date_published", "published_at"))
            return _modrinth_candidate(project_id, details, item, core, version)
        if self.kind == "spigot":
            value = self.http.get_json(f"{self.base_url}/resources/{quote(project_id, safe='')}")
            return self._spigot_candidate(project_id, value if isinstance(value, dict) else {}, core, version)
        if self.kind == "hangar":
            project_path = _hangar_project_path(project_id)
            value = self.http.get_json(f"{self.base_url}/projects/{project_path}/versions?{urlencode({'limit': 100})}")
            rows = _provider_rows(value, "result", "versions")
            valid_rows = [item for item in rows if isinstance(item, dict)]
            if not valid_rows:
                return None
            compatible_rows = [item for item in valid_rows if _hangar_compatible(item, core, version)]
            item = _latest_item(compatible_rows or valid_rows, ("createdAt", "datePublished", "created_at"))
            return _hangar_candidate(self.base_url, project_id, item, compatible=bool(compatible_rows))
        matches = self.search(project_id, core, version, 30)
        matches = _sort_results([item for item in matches if item.project_id == project_id], "updated")
        result = matches[0] if matches else None
        return _candidate_from_result(result, compatible=bool(result and core and version)) if result else None

    def _modrinth_versions(self, project_id: str, core: str, version: str) -> list[dict[str, Any]]:
        params: dict[str, str] = {}
        if version:
            params["game_versions"] = json.dumps([version], separators=(",", ":"))
        if core and core not in {"custom", "bukkit"}:
            params["loaders"] = json.dumps([core], separators=(",", ":"))
        suffix = f"?{urlencode(params)}" if params else ""
        value = self.http.get_json(f"{self.base_url}/project/{quote(project_id, safe='')}/version{suffix}")
        rows = [item for item in value if isinstance(item, dict)] if isinstance(value, list) else []
        return [item for item in rows if _modrinth_plugin_version_compatible(item, core, version)]

    def _modrinth_project_details(self, project_id: str) -> dict[str, Any]:
        value = self.http.get_json(f"{self.base_url}/project/{quote(project_id, safe='')}")
        if not isinstance(value, dict):
            raise LauncherError("Modrinth не вернул сведения о проекте")
        if str(value.get("project_type", "")).strip() != "plugin":
            actual = str(value.get("project_type", "неизвестен")).strip() or "неизвестен"
            raise LauncherError(f"Modrinth-проект {project_id} имеет тип {actual}, ожидался plugin")
        returned_id = str(value.get("id") or value.get("project_id") or project_id).strip()
        if returned_id != project_id:
            raise LauncherError("Modrinth вернул сведения о другом проекте")
        return value

    def _spigot_candidate(self, project_id: str, item: dict[str, Any], core: str, version: str) -> dict[str, Any]:
        result = _spigot_result(item, project_id)
        update_date = result.updated_at or str(item.get("updateDate") or "")
        provider_version = str(item.get("version") or item.get("tag") or "")
        hashes = _provider_hashes(item)
        return {
            **_candidate_from_result(result, compatible=_spigot_compatible(item, core, version)),
            "version": provider_version or update_date,
            "version_id": update_date,
            "download_url": f"{self.base_url}/resources/{quote(project_id, safe='')}/download",
            "file_name": f"{_safe_file_name(project_id)}.jar",
            **hashes,
            "size": _optional_int(item.get("size")),
            "trusted_hash": bool(hashes.get("sha1") or hashes.get("sha256") or hashes.get("sha512")),
        }

    def _download_candidate(
        self,
        candidate: dict[str, Any],
        target: Path,
        destination: Path,
        core: str,
        version: str,
        *,
        confirm_unverified: bool,
        replace: bool = False,
        expected_entry: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        hashes = {name: candidate.get(name) or None for name in ("sha1", "sha256", "sha512")}
        trusted_hash = any(hashes.values())
        _safe_jar_file_name(str(candidate.get("file_name") or destination.name))
        if not trusted_hash and not confirm_unverified:
            raise LauncherError(UNVERIFIED_PLUGIN_DOWNLOAD_MESSAGE)
        target = _plugin_directory(target)
        if destination.parent != target:
            raise LauncherError("Путь JAR плагина находится вне каталога plugins")
        if destination.is_symlink() or (destination.exists() and not destination.is_file()):
            raise LauncherError("Путь JAR плагина небезопасен")
        if not replace and os.path.lexists(destination):
            raise LauncherError("Файл плагина уже существует")
        url = str(candidate.get("download_url", ""))
        if not url:
            raise LauncherError("Каталог не вернул ссылку для скачивания плагина")
        size = _optional_int(candidate.get("size"))
        descriptor, temporary_name = tempfile.mkstemp(prefix=".trial-plugin-", suffix=".jar", dir=target)
        os.close(descriptor)
        temporary = Path(temporary_name)
        try:
            self.http.download(url, temporary, **hashes, size=size)
            if temporary.is_symlink() or not temporary.is_file():
                raise LauncherError("Каталог не создал корректный временный JAR")
            if not verify_file(temporary, **hashes, size=size):
                raise LauncherError("Проверка хеша или размера JAR не пройдена")
            digest = file_hash(temporary, "sha256")
            if replace:
                current = next(
                    (entry for entry in installed_plugin_metadata_index(target) if entry["file_name"] == destination.name),
                    None,
                )
                if (
                    current is None
                    or expected_entry is None
                    or current.get("sha256") != expected_entry.get("sha256")
                    or current.get("project_id") != expected_entry.get("project_id")
                    or current.get("catalog_id") != expected_entry.get("catalog_id")
                ):
                    raise LauncherError("Установленный JAR изменился во время загрузки; обновление отменено")
            elif os.path.lexists(destination):
                raise LauncherError("Файл плагина был создан во время загрузки")
            temporary.replace(destination)
            result = {
                "source": self.kind,
                "catalog_id": str(self.catalog.get("id", self.kind)),
                "project_id": str(candidate.get("project_id", "")),
                "file_name": destination.name,
                "version": str(candidate.get("version", "")),
                "version_id": str(candidate.get("version_id", "")),
                "path": str(destination),
                "sha256": digest,
                "trusted_hash": trusted_hash,
                "created_at": str(candidate.get("created_at", "")),
                "updated_at": str(candidate.get("updated_at", "")),
                "dependencies": candidate.get("dependencies", []),
            }
            metadata = {
                "schema": PLUGIN_METADATA_SCHEMA,
                "managed_by": "trial-launcher",
                "catalog_id": result["catalog_id"],
                "source": self.kind,
                "api_url": self.base_url,
                "project_id": result["project_id"],
                "file_name": destination.name,
                "version": result["version"],
                "version_id": str(candidate.get("version_id", "")),
                "core": str(core),
                "minecraft_version": str(version),
                "sha256": digest,
                "trusted_hash": trusted_hash,
                "size": destination.stat().st_size,
                "created_at": result["created_at"],
                "updated_at": result["updated_at"],
                "installed_at": _now_iso(),
                "dependencies": _dependencies(candidate.get("dependencies")),
            }
            _write_plugin_metadata(destination, metadata)
            return result
        finally:
            if temporary.exists() or temporary.is_symlink():
                temporary.unlink()

    def _matches_catalog(self, entry: dict[str, Any]) -> bool:
        catalog_id = str(self.catalog.get("id", ""))
        return (
            entry.get("source") == self.kind
            and entry.get("api_url") == self.base_url
            and (not catalog_id or entry.get("catalog_id") == catalog_id)
        )


def normalize_plugin_catalogs(value: Any) -> list[dict[str, Any]]:
    raw = value if isinstance(value, list) else DEFAULT_PLUGIN_CATALOGS
    result: list[dict[str, Any]] = []
    seen: set[str] = set()
    for item in raw[:20]:
        if not isinstance(item, dict):
            continue
        catalog_id = str(item.get("id", "")).strip().lower()
        name = str(item.get("name", "")).strip()[:80]
        kind = str(item.get("kind", "custom")).strip().lower()
        api_url = str(item.get("api_url", "")).strip().rstrip("/")
        if not catalog_id or catalog_id in seen or not name or kind not in {"modrinth", "spigot", "hangar", "custom"} or not api_url.startswith("https://"):
            continue
        seen.add(catalog_id)
        result.append({"id": catalog_id[:64], "name": name, "kind": kind, "api_url": api_url[:512], "enabled": bool(item.get("enabled", True))})
    if not result:
        return [dict(item) for item in DEFAULT_PLUGIN_CATALOGS]
    return result


def _custom_result(kind: str, item: dict[str, Any]) -> PluginCatalogResult:
    return PluginCatalogResult(
        source=kind,
        project_id=str(item.get("project_id") or item.get("id") or item.get("slug") or ""),
        title=str(item.get("title") or item.get("name") or "Plugin"),
        description=str(item.get("description") or ""),
        author=str(item.get("author") or ""),
        icon_url=str(item.get("icon_url") or item.get("icon") or ""),
        downloads=_int(item.get("downloads")),
        version=str(item.get("version") or ""),
        download_url=str(item.get("download_url") or ""),
        file_name=str(item.get("file_name") or ""),
        sha1=str(item.get("sha1") or ""),
        sha256=str(item.get("sha256") or ""),
        sha512=str(item.get("sha512") or ""),
        size=_optional_int(item.get("size")),
        created_at=_date_value(_first(item, "created_at", "createdAt", "date_created")),
        updated_at=_date_value(_first(item, "updated_at", "updatedAt", "date_modified", "updateDate")),
        dependencies=_dependencies(_first(item, "dependencies", "plugin_dependencies")),
        version_id=str(item.get("version_id") or ""),
        _downloads_known="downloads" in item and item.get("downloads") is not None,
    )


def _modrinth_result(item: dict[str, Any], fallback_id: str = "") -> PluginCatalogResult:
    return PluginCatalogResult(
        source="modrinth",
        project_id=str(item.get("project_id") or item.get("id") or fallback_id),
        title=str(item.get("title") or item.get("name") or "Plugin"),
        description=str(item.get("description") or ""),
        author=str(item.get("author") or ""),
        icon_url=str(item.get("icon_url") or item.get("icon") or ""),
        downloads=_int(item.get("downloads")),
        version=str(item.get("version") or ""),
        download_url=str(item.get("download_url") or ""),
        file_name=str(item.get("file_name") or ""),
        sha1=str(item.get("sha1") or ""),
        sha256=str(item.get("sha256") or ""),
        sha512=str(item.get("sha512") or ""),
        size=_optional_int(item.get("size")),
        created_at=_date_value(_first(item, "date_created", "created_at", "createdAt")),
        updated_at=_date_value(_first(item, "date_modified", "updated_at", "updatedAt")),
        dependencies=_dependencies(_first(item, "dependencies", "plugin_dependencies")),
        version_id=str(item.get("version_id") or ""),
        _downloads_known="downloads" in item and item.get("downloads") is not None,
    )


def _spigot_result(item: dict[str, Any], fallback_id: str = "") -> PluginCatalogResult:
    author = item.get("author", "")
    if isinstance(author, dict):
        author = author.get("name", "")
    return PluginCatalogResult(
        source="spigot",
        project_id=str(item.get("id") or fallback_id),
        title=str(item.get("name") or "Spigot resource"),
        description=str(item.get("tagged") or item.get("description") or ""),
        author=str(author or ""),
        icon_url=str(item.get("icon_url") or item.get("icon") or ""),
        downloads=_int(item.get("downloads")),
        version=str(item.get("version") or ""),
        download_url=str(item.get("download_url") or ""),
        file_name=str(item.get("file_name") or ""),
        **_provider_hashes(item),
        size=_optional_int(item.get("size")),
        created_at=_date_value(_first(item, "releaseDate", "created_at", "createdAt")),
        updated_at=_date_value(_first(item, "updateDate", "updated_at", "updatedAt")),
        dependencies=_dependencies(_first(item, "dependencies", "depend", "softdepend")),
        version_id=str(item.get("version_id") or ""),
        _downloads_known="downloads" in item and item.get("downloads") is not None,
    )


def _hangar_result(item: dict[str, Any], fallback_id: str = "") -> PluginCatalogResult:
    namespace = item.get("namespace")
    namespace = namespace if isinstance(namespace, dict) else {}
    owner = item.get("owner")
    owner = owner if isinstance(owner, dict) else {}
    stats = item.get("stats")
    stats = stats if isinstance(stats, dict) else {}
    downloads = stats.get("downloads", item.get("downloads"))
    project_id = namespace.get("slug") or item.get("slug") or item.get("project_id") or fallback_id
    return PluginCatalogResult(
        source="hangar",
        project_id=str(project_id),
        title=str(item.get("name") or "Hangar project"),
        description=str(item.get("description") or ""),
        author=str(owner.get("name") or item.get("author") or ""),
        icon_url=str(item.get("avatar") or item.get("icon_url") or ""),
        downloads=_int(downloads),
        version=str(item.get("version") or ""),
        download_url=str(item.get("download_url") or ""),
        file_name=str(item.get("file_name") or ""),
        **_provider_hashes(item),
        size=_optional_int(item.get("size")),
        created_at=_date_value(_first(item, "createdAt", "created_at", "dateCreated")),
        updated_at=_date_value(_first(item, "lastUpdated", "updatedAt", "updated_at", "dateModified")),
        dependencies=_dependencies(_first(item, "pluginDependencies", "dependencies")),
        version_id=str(item.get("version_id") or ""),
        _downloads_known=("downloads" in stats and downloads is not None) or ("downloads" in item and item.get("downloads") is not None),
    )


def _empty_result(source: str, project_id: str) -> PluginCatalogResult:
    return PluginCatalogResult(source, project_id, "Plugin", "", "", "", 0, _downloads_known=False)


def _candidate_from_result(result: PluginCatalogResult, *, compatible: bool) -> dict[str, Any]:
    hashes = _valid_hashes({"sha1": result.sha1, "sha256": result.sha256, "sha512": result.sha512})
    return {
        "source": result.source,
        "project_id": result.project_id,
        "title": result.title,
        "description": result.description,
        "author": result.author,
        "icon_url": result.icon_url,
        "downloads": result.downloads,
        "version": result.version,
        "version_id": result.version_id,
        "download_url": result.download_url,
        "file_name": result.file_name,
        **hashes,
        "size": result.size,
        "created_at": result.created_at,
        "updated_at": result.updated_at,
        "dependencies": result.dependencies,
        "trusted_hash": bool(any(hashes.values())),
        "compatible": compatible,
    }


def _modrinth_candidate(
    project_id: str,
    project: dict[str, Any],
    item: dict[str, Any],
    core: str,
    version: str,
) -> dict[str, Any]:
    result = _modrinth_result(project, project_id)
    files = item.get("files", []) if isinstance(item.get("files"), list) else []
    primary = next((file for file in files if isinstance(file, dict) and file.get("primary")), None)
    primary = primary or next((file for file in files if isinstance(file, dict)), {})
    hashes = _valid_hashes(primary.get("hashes", {}) if isinstance(primary.get("hashes"), dict) else {})
    file_name = _safe_jar_file_name(str(primary.get("filename") or f"{project_id}.jar"))
    version_id = str(item.get("id") or "")
    published = _date_value(_first(item, "date_published", "published_at"))
    updated_at = result.updated_at or published
    compatible = bool(version and core and core not in {"custom", "bukkit"})
    return {
        **_candidate_from_result(result, compatible=compatible),
        "version": str(item.get("version_number") or item.get("name") or ""),
        "version_id": version_id,
        "download_url": str(primary.get("url") or ""),
        "file_name": file_name,
        **hashes,
        "size": _optional_int(primary.get("size")),
        "created_at": result.created_at,
        "updated_at": updated_at,
        "dependencies": _dependencies(item.get("dependencies")),
        "trusted_hash": bool(any(hashes.values())),
        "compatible": compatible,
    }


def _hangar_candidate(
    base_url: str,
    project_id: str,
    item: dict[str, Any],
    *,
    compatible: bool,
) -> dict[str, Any]:
    project_path = _hangar_project_path(project_id)
    resolved_version = str(item.get("name") or item.get("version") or "")
    hashes = _provider_hashes(item)
    file_name = _safe_jar_file_name(f"{project_id.replace('/', '-')}.jar")
    result = PluginCatalogResult(
        source="hangar",
        project_id=project_id,
        title=str(item.get("projectName") or item.get("name") or project_id),
        description=str(item.get("description") or ""),
        author="",
        icon_url="",
        downloads=_int(item.get("downloads")),
        _downloads_known="downloads" in item and item.get("downloads") is not None,
    )
    created_at = _date_value(_first(item, "createdAt", "datePublished", "created_at"))
    return {
        **_candidate_from_result(result, compatible=compatible),
        "version": resolved_version,
        "version_id": str(item.get("id") or item.get("versionId") or resolved_version),
        "download_url": f"{base_url}/projects/{project_path}/versions/{quote(resolved_version, safe='')}/download" if resolved_version else "",
        "file_name": file_name,
        **hashes,
        "size": _optional_int(item.get("size")),
        "created_at": created_at,
        "updated_at": created_at,
        "dependencies": _dependencies(_first(item, "pluginDependencies", "dependencies")),
        "trusted_hash": bool(any(hashes.values())),
        "compatible": compatible,
    }


def _apply_candidate(result: PluginCatalogResult, candidate: dict[str, Any]) -> None:
    result.version = str(candidate.get("version", ""))
    result.version_id = str(candidate.get("version_id", ""))
    result.download_url = str(candidate.get("download_url", ""))
    result.file_name = str(candidate.get("file_name", ""))
    result.sha1 = str(candidate.get("sha1", ""))
    result.sha256 = str(candidate.get("sha256", ""))
    result.sha512 = str(candidate.get("sha512", ""))
    result.size = _optional_int(candidate.get("size"))
    result.created_at = str(result.created_at or candidate.get("created_at", ""))
    result.updated_at = str(result.updated_at or candidate.get("updated_at", ""))
    result.dependencies = _dependencies(candidate.get("dependencies"))


def _validate_sort(sort: str) -> str:
    mode = str(sort or "relevance").strip().lower()
    if mode not in PLUGIN_SORT_MODES:
        raise LauncherError(f"Неизвестный порядок сортировки плагинов: {mode}")
    return mode


def _provider_sort(kind: str, mode: str) -> str:
    if kind == "spigot":
        return {"downloads": "-downloads", "newest": "-releaseDate", "updated": "-updateDate"}.get(mode, "")
    if kind == "hangar":
        return {"downloads": "POPULARITY", "newest": "NEWEST", "updated": "UPDATED"}.get(mode, "")
    return ""


def _sort_results(results: list[PluginCatalogResult], mode: str) -> list[PluginCatalogResult]:
    if mode == "relevance":
        return results
    values: list[tuple[PluginCatalogResult, float | int | None]] = []
    for item in results:
        if mode == "downloads":
            value: float | int | None = item.downloads if item._downloads_known else None
        elif mode == "newest":
            value = _date_timestamp(item.created_at)
        else:
            value = _date_timestamp(item.updated_at)
        values.append((item, value))
    if not any(value is not None for _, value in values):
        return results
    indexed = list(enumerate(values))
    indexed.sort(
        key=lambda pair: (
            pair[1][1] is None,
            -(pair[1][1] or 0),
            pair[1][0].project_id.casefold(),
            pair[1][0].title.casefold(),
            pair[0],
        )
    )
    return [item for _, (item, _) in indexed]


def _spigot_compatible(item: dict[str, Any], core: str, version: str) -> bool:
    compatible_cores = {"bukkit", "spigot", "paper", "purpur", "folia", "leaf", "pufferfish", "patina"}
    if str(core).strip().lower() not in compatible_cores or not version:
        return False
    tested = _first(item, "testedVersions", "tested_versions", "game_versions")
    if isinstance(tested, dict):
        tested = list(tested)
    if isinstance(tested, str):
        tested = [part.strip() for part in tested.split(",") if part.strip()]
    return _versions_match(tested if isinstance(tested, list) else [], version)


def _modrinth_plugin_version_compatible(item: dict[str, Any], core: str, version: str) -> bool:
    normalized_core = str(core).strip().lower()
    if not normalized_core or normalized_core in {"custom", "bukkit"} or not version:
        return False
    game_versions = item.get("game_versions")
    loaders = item.get("loaders")
    return (
        isinstance(game_versions, list)
        and version in game_versions
        and isinstance(loaders, list)
        and normalized_core in [str(value).strip().lower() for value in loaders]
    )


def _hangar_compatible(item: dict[str, Any], core: str, version: str) -> bool:
    platform = _hangar_platform(core)
    if not platform or not version:
        return False
    dependencies = _first(item, "platformDependencies", "platform_dependencies")
    if not isinstance(dependencies, dict):
        return False
    supported: Any = next((value for key, value in dependencies.items() if str(key).upper() == platform), None)
    if isinstance(supported, dict):
        supported = supported.get("versions", supported.get("minecraftVersions", list(supported)))
    if isinstance(supported, str):
        supported = [supported]
    elif not isinstance(supported, list):
        supported = list(supported) if isinstance(supported, tuple) else []
    return _versions_match(supported, version)


def _hangar_platform(core: str) -> str:
    normalized = str(core).strip().lower()
    if normalized in {"paper", "purpur", "folia", "leaf", "pufferfish", "patina"}:
        return "PAPER"
    if normalized == "velocity":
        return "VELOCITY"
    if normalized == "waterfall":
        return "WATERFALL"
    return ""


def _versions_match(values: list[Any], requested: str) -> bool:
    for value in values:
        candidate = str(value).strip()
        if candidate == "*" or candidate == requested or requested.startswith(f"{candidate}.") or candidate.startswith(f"{requested}."):
            return True
    return False


def _validate_update_context(core: str, version: str) -> None:
    if not str(core).strip() or not str(version).strip():
        raise LauncherError("Проверка обновлений требует ядро сервера и версию Minecraft")


def _is_update(installed: dict[str, Any], candidate: dict[str, Any] | None) -> bool:
    if not candidate:
        return False
    current_id = str(installed.get("version_id", ""))
    latest_id = str(candidate.get("version_id", ""))
    if current_id and latest_id:
        return current_id != latest_id
    current_updated = str(installed.get("updated_at", ""))
    latest_updated = str(candidate.get("updated_at", ""))
    if current_updated and latest_updated:
        return current_updated != latest_updated
    current_version = str(installed.get("version", ""))
    latest_version = str(candidate.get("version", ""))
    if current_version or latest_version:
        return current_version != latest_version
    current_hash = str(installed.get("sha256", ""))
    latest_hash = str(candidate.get("sha256", ""))
    return bool(current_hash and latest_hash and current_hash.lower() != latest_hash.lower())


def installed_plugin_metadata_index(target: Path) -> list[dict[str, Any]]:
    root = Path(target)
    if root.is_symlink():
        raise LauncherError("Каталог plugins является символьной ссылкой")
    if not root.exists():
        return []
    if not root.is_dir():
        raise LauncherError("Путь plugins не является каталогом")
    result: list[dict[str, Any]] = []
    try:
        paths = sorted(root.iterdir(), key=lambda item: item.name.casefold())
    except OSError as exc:
        raise LauncherError(f"Не удалось прочитать каталог plugins: {exc}") from exc
    for jar in paths[:2000]:
        if len(result) >= 1000:
            break
        if jar.suffix.lower() != ".jar" or jar.is_symlink() or not jar.is_file():
            continue
        try:
            _safe_jar_file_name(jar.name)
        except LauncherError:
            continue
        sidecar = _plugin_metadata_path(jar)
        if sidecar.is_symlink() or not sidecar.is_file():
            continue
        try:
            metadata = _read_plugin_metadata(sidecar)
            if not isinstance(metadata, dict) or metadata.get("schema") != PLUGIN_METADATA_SCHEMA or metadata.get("managed_by") != "trial-launcher":
                continue
            if metadata.get("file_name") != jar.name:
                continue
            if metadata.get("source") not in {"modrinth", "spigot", "hangar", "custom"}:
                continue
            catalog_id = str(metadata.get("catalog_id", ""))
            api_url = str(metadata.get("api_url", ""))
            project_id = str(metadata.get("project_id", ""))
            sha256 = str(metadata.get("sha256", "")).lower()
            if not catalog_id or not api_url.startswith("https://") or not project_id or not re.fullmatch(r"[0-9a-f]{64}", sha256):
                continue
            jar_info = _hash_regular_file(jar)
            if not jar_info or jar_info[1] != _optional_int(metadata.get("size")):
                continue
            if jar_info[0] != sha256:
                continue
            result.append(
                {
                    "source": metadata["source"],
                    "catalog_id": catalog_id,
                    "api_url": api_url,
                    "project_id": project_id,
                    "file_name": jar.name,
                    "path": str(jar),
                    "version": str(metadata.get("version", "")),
                    "version_id": str(metadata.get("version_id", "")),
                    "core": str(metadata.get("core", "")),
                    "minecraft_version": str(metadata.get("minecraft_version", "")),
                    "sha256": sha256,
                    "trusted_hash": bool(metadata.get("trusted_hash", False)),
                    "size": jar_info[1],
                    "created_at": _date_value(metadata.get("created_at")),
                    "updated_at": _date_value(metadata.get("updated_at")),
                    "installed_at": _date_value(metadata.get("installed_at")),
                    "dependencies": _dependencies(metadata.get("dependencies")),
                    "managed": True,
                }
            )
        except (OSError, UnicodeError, ValueError, TypeError):
            continue
    return result


def remove_installed_plugin(target: Path, file_name: str) -> bool:
    file_name = _safe_jar_file_name(file_name)
    entry = next(
        (item for item in installed_plugin_metadata_index(Path(target)) if item["file_name"] == file_name),
        None,
    )
    if entry is None:
        return False
    directory = _plugin_directory(Path(target), create=False)
    jar = directory / file_name
    metadata = _plugin_metadata_path(jar)
    if jar.is_symlink() or metadata.is_symlink() or not jar.is_file() or not metadata.is_file():
        raise LauncherError("Установленный плагин или его метаданные изменились")
    try:
        jar.unlink()
        metadata.unlink()
    except OSError as exc:
        raise LauncherError("Не удалось удалить плагин, установленный из каталога") from exc
    return True


def _plugin_directory(path: Path, *, create: bool = True) -> Path:
    directory = Path(path)
    if directory.is_symlink():
        raise LauncherError("Каталог plugins является символьной ссылкой")
    if create:
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    if directory.is_symlink() or not directory.is_dir():
        raise LauncherError("Каталог plugins недоступен")
    return directory


def _safe_jar_file_name(value: str) -> str:
    name = _safe_file_name(str(value))
    if len(name) > 180 or not name.lower().endswith(".jar"):
        raise LauncherError("Файл серверного плагина должен иметь безопасное имя и расширение .jar")
    return name


def _plugin_project_id(value: Any) -> str:
    project_id = "" if value is None or isinstance(value, bool) else str(value).strip()
    if (
        not project_id
        or len(project_id) > 200
        or project_id in {".", ".."}
        or any(ord(character) < 32 or ord(character) == 127 for character in project_id)
    ):
        raise LauncherError("Некорректный project ID зависимости серверного плагина")
    return project_id


def _plugin_version_id(value: Any) -> str:
    version_id = "" if value is None or isinstance(value, bool) else str(value).strip()
    if version_id and (
        len(version_id) > 200
        or any(ord(character) < 32 or ord(character) == 127 for character in version_id)
    ):
        raise LauncherError("Некорректный ID версии зависимости серверного плагина")
    return version_id


def _candidate_has_trusted_hash(candidate: dict[str, Any]) -> bool:
    return any(str(candidate.get(name) or "").strip() for name in ("sha1", "sha256", "sha512"))


def _unique_plugin_file(directory: Path, name: str, reserved_names: set[str] | None = None) -> Path:
    reserved_names = reserved_names or set()
    candidate = _unique_file(directory, name)
    if candidate.name not in reserved_names and not os.path.lexists(_plugin_metadata_path(candidate)):
        return candidate
    stem, suffix = candidate.stem, candidate.suffix
    index = 2
    while True:
        next_candidate = directory / f"{stem}-{index}{suffix}"
        if (
            next_candidate.name not in reserved_names
            and not os.path.lexists(next_candidate)
            and not os.path.lexists(_plugin_metadata_path(next_candidate))
        ):
            return next_candidate
        index += 1


def _plugin_metadata_path(jar: Path) -> Path:
    return jar.with_name(f".{jar.name}.trial-plugin.json")


def _write_plugin_metadata(jar: Path, value: dict[str, Any]) -> None:
    sidecar = _plugin_metadata_path(jar)
    if jar.parent.is_symlink() or sidecar.is_symlink():
        raise LauncherError("Метаданные каталога являются символьной ссылкой")
    encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    if len(encoded.encode("utf-8")) > PLUGIN_METADATA_MAX_BYTES:
        raise LauncherError("Метаданные плагина слишком велики")
    descriptor, temporary_name = tempfile.mkstemp(prefix=".trial-plugin-meta-", suffix=".tmp", dir=jar.parent)
    temporary = Path(temporary_name)
    try:
        set_private_fd(descriptor)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        if sidecar.is_symlink():
            raise LauncherError("Метаданные каталога являются символьной ссылкой")
        temporary.replace(sidecar)
    finally:
        if temporary.exists() or temporary.is_symlink():
            temporary.unlink()


def _read_plugin_metadata(path: Path) -> Any:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as handle:
        metadata_stat = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata_stat.st_mode) or metadata_stat.st_size > PLUGIN_METADATA_MAX_BYTES:
            return None
        encoded = handle.read(PLUGIN_METADATA_MAX_BYTES + 1)
    if len(encoded) > PLUGIN_METADATA_MAX_BYTES:
        return None
    return json.loads(encoded.decode("utf-8"))


def _hash_regular_file(path: Path) -> tuple[str, int] | None:
    try:
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "rb") as handle:
            file_stat = os.fstat(handle.fileno())
            if not stat.S_ISREG(file_stat.st_mode) or file_stat.st_size > 512 * 1024 * 1024:
                return None
            digest = hashlib.sha256()
            for block in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(block)
        return digest.hexdigest(), file_stat.st_size
    except OSError:
        return None


def _hangar_project_path(project_id: str) -> str:
    parts = str(project_id).strip().split("/")
    if not parts or len(parts) > 2 or any(not part or part in {".", ".."} for part in parts):
        raise LauncherError("Некорректный идентификатор Hangar-проекта")
    return "/".join(quote(part, safe="") for part in parts)


def _provider_rows(value: Any, *keys: str) -> list[Any]:
    if isinstance(value, list):
        return value
    if not isinstance(value, dict):
        return []
    for key in keys:
        rows = value.get(key)
        if isinstance(rows, list):
            return rows
    return []


def _latest_item(rows: list[dict[str, Any]], date_fields: tuple[str, ...]) -> dict[str, Any]:
    if not rows:
        return {}
    selected = rows[0]
    selected_at = _date_timestamp(_first(selected, *date_fields)) or 0
    for item in rows[1:]:
        item_at = _date_timestamp(_first(item, *date_fields)) or 0
        if item_at > selected_at:
            selected, selected_at = item, item_at
    return selected


def _provider_hashes(item: dict[str, Any]) -> dict[str, str]:
    raw = item.get("hashes") if isinstance(item.get("hashes"), dict) else {}
    checksums = item.get("checksums") if isinstance(item.get("checksums"), dict) else {}
    return _valid_hashes({**checksums, **raw, **{key: item.get(key) for key in ("sha1", "sha256", "sha512")}})


def _valid_hashes(value: Any) -> dict[str, str]:
    value = value if isinstance(value, dict) else {}
    lengths = {"sha1": 40, "sha256": 64, "sha512": 128}
    result: dict[str, str] = {}
    for algorithm, length in lengths.items():
        digest = str(value.get(algorithm, "")).strip().lower()
        result[algorithm] = digest if len(digest) == length and re.fullmatch(r"[0-9a-f]+", digest) else ""
    return result


def _dependencies(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, dict):
        rows: list[Any] = []
        for key, item in list(value.items())[:100]:
            if isinstance(item, dict):
                rows.append({"id": str(key), **item})
            else:
                rows.append({"id": str(key), "version": item})
    elif isinstance(value, list):
        rows = value[:100]
    else:
        return []
    result: list[dict[str, Any]] = []
    for item in rows:
        if isinstance(item, dict):
            result.append({str(key)[:80]: _bounded_json(child) for key, child in list(item.items())[:30]})
        elif isinstance(item, (str, int, float)):
            result.append({"id": str(item)[:200]})
    return result


def _bounded_json(value: Any, depth: int = 0) -> Any:
    if depth >= 4:
        return None
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, str):
        return value[:500]
    if isinstance(value, list):
        return [_bounded_json(item, depth + 1) for item in value[:50]]
    if isinstance(value, dict):
        return {str(key)[:80]: _bounded_json(item, depth + 1) for key, item in list(value.items())[:30]}
    return str(value)[:200]


def _first(value: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        result = value.get(key)
        if result is not None and result != "":
            return result
    return None


def _date_value(value: Any) -> str:
    if value is None or isinstance(value, bool):
        return ""
    if isinstance(value, (int, float)):
        timestamp = float(value)
        if timestamp > 100_000_000_000:
            timestamp /= 1000
        try:
            return datetime.fromtimestamp(timestamp, timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
        except (OverflowError, OSError, ValueError):
            return ""
    text = str(value).strip()
    if not text:
        return ""
    if re.fullmatch(r"\d+(?:\.\d+)?", text):
        try:
            return _date_value(float(text))
        except ValueError:
            return ""
    try:
        parsed = datetime.fromisoformat(text.replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=timezone.utc)
        return parsed.astimezone(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    except ValueError:
        return text[:64]


def _date_timestamp(value: Any) -> float | None:
    normalized = _date_value(value)
    if not normalized:
        return None
    try:
        return datetime.fromisoformat(normalized.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def _int(value: Any) -> int:
    try:
        return max(0, int(value or 0))
    except (TypeError, ValueError, OverflowError):
        return 0


def _optional_int(value: Any) -> int | None:
    try:
        return max(0, int(value)) if value is not None else None
    except (TypeError, ValueError, OverflowError):
        return None


def _unique_file(directory: Path, name: str) -> Path:
    candidate = directory / name
    if not os.path.lexists(candidate):
        return candidate
    stem, suffix = candidate.stem, candidate.suffix
    index = 2
    while os.path.lexists(directory / f"{stem}-{index}{suffix}"):
        index += 1
    return directory / f"{stem}-{index}{suffix}"
