from __future__ import annotations

import json
import os
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlencode

from .constants import MODRINTH_API_URL
from .errors import LauncherError
from .net import HttpClient
from .operations import OperationCancelled
from .platform_support import set_private_fd

@dataclass
class ModResult:
    source: str
    project_id: str
    name: str
    version: str
    file_name: str
    path: Path | None = None
    version_id: str = ""
    dependencies: list[dict[str, str]] = field(default_factory=list)

PROJECT_DIRECTORIES = {
    "mod": "mods",
    "resourcepack": "resourcepacks",
    "shader": "shaderpacks",
    "datapack": "datapacks",
}
PROJECT_METADATA_SUFFIX = ".trial-project.json"
MODRINTH_SEARCH_ORDERINGS = frozenset({"relevance", "downloads", "newest", "updated"})
MAX_MOD_DEPENDENCY_PROJECTS = 64
MAX_MOD_DEPENDENCY_DEPTH = 16
MAX_REQUIRED_DEPENDENCIES_PER_VERSION = 128
MAX_PROJECT_METADATA_BYTES = 64 * 1024

@dataclass
class _ResolvedProject:
    project_id: str
    version: dict[str, Any]
    project_metadata: dict[str, Any]
    primary_file: dict[str, Any]
    file_name: str
    dependencies: list[str] = field(default_factory=list)
    publisher_hashes: dict[str, str | None] = field(default_factory=dict)
    size: int | None = None
    target: Path | None = None

class ModrinthProvider:
    def __init__(
        self,
        http: HttpClient | None = None,
        base_url: str = MODRINTH_API_URL,
        *,
        catalog_id: str = "modrinth",
    ) -> None:
        self.http = http or HttpClient()
        self.base_url = base_url.rstrip("/") or MODRINTH_API_URL
        self.catalog_id = str(catalog_id or "modrinth").strip()[:80] or "modrinth"

    def search(
        self,
        query: str,
        game_version: str | None = None,
        loader: str | None = None,
        limit: int = 10,
        project_type: str = "mod",
        *,
        sort_by: str = "relevance",
        offset: int = 0,
    ) -> list[dict[str, Any]]:
        self._validate_project_type(project_type)
        sort_by = str(sort_by).strip().lower()
        if sort_by not in MODRINTH_SEARCH_ORDERINGS:
            raise LauncherError(f"Неизвестный порядок сортировки Modrinth: {sort_by}")
        try:
            limit = max(1, min(100, int(limit)))
        except (TypeError, ValueError) as exc:
            raise LauncherError("Лимит поиска должен быть числом") from exc

        facets = [[f"project_type:{project_type}"]]
        if game_version:
            facets.append([f"versions:{game_version}"])
        if loader and loader.lower() != "vanilla" and project_type == "mod":
            facets.append([f"categories:{loader.lower()}"])
        params: dict[str, str] = {
            "limit": str(limit),
            "index": sort_by,
            "facets": json.dumps(facets, separators=(",", ":")),
        }
        offset = max(0, min(10_000, int(offset or 0)))
        if offset:
            params["offset"] = str(offset)
        if query.strip():
            params["query"] = query.strip()
        value = self.http.get_json(f"{self.base_url}/search?{urlencode(params)}")
        hits = value.get("hits", []) if isinstance(value, dict) else []
        # всего найдено — для постраничного каталога.
        total = value.get("total_hits") if isinstance(value, dict) else None
        self.last_total = int(total) if isinstance(total, int) and total >= 0 else None
        return [
            item
            for item in hits
            if isinstance(item, dict) and _matches_project_type(item, project_type)
        ]

    def project_details(self, project_id: str, project_type: str = "mod") -> dict[str, Any]:
        self._validate_project_type(project_type)
        project_id = _project_id(project_id)
        value = self.http.get_json(f"{self.base_url}/project/{quote(project_id, safe='')}")
        if not isinstance(value, dict):
            raise LauncherError(f"Modrinth вернул некорректные данные проекта {project_id}")
        _validate_detail_project_type(value, project_id, project_type)
        return value

    def compatible_versions(
        self,
        project_id: str,
        game_version: str = "",
        loader: str | None = None,
        project_type: str = "mod",
    ) -> list[dict[str, Any]]:
        self.project_details(project_id, project_type)
        return self._compatible_versions(project_id, game_version, loader, project_type)

    def select_compatible_version(
        self,
        project_id: str,
        game_version: str = "",
        loader: str | None = None,
        project_type: str = "mod",
        *,
        version_id: str | None = None,
    ) -> dict[str, Any]:
        project_id = _project_id(project_id)
        versions = self.compatible_versions(project_id, game_version, loader, project_type)
        selected = next((item for item in versions if _version_id(item) == version_id), None) if version_id else None
        selected = selected or (versions[0] if versions and not version_id else None)
        if not selected:
            if version_id:
                raise LauncherError(f"Версия {version_id} проекта {project_id} несовместима с выбранной игрой или загрузчиком")
            raise LauncherError(f"Modrinth не нашёл совместимую версию для {project_id}")
        return selected

    def install(
        self,
        project: str,
        instance_dir: Path,
        minecraft_version: str,
        loader: str | None = None,
        project_type: str = "mod",
        target_dir: Path | None = None,
        *,
        confirm_unverified: bool = False,
        version_id: str | None = None,
        allow_replace: bool = False,
    ) -> ModResult:
        self._validate_project_type(project_type)
        project_id = _project_id(project)
        game_version = str(minecraft_version or "").strip()
        loader = str(loader or "").strip().lower() or None
        install_root, destination_dir = self._resource_paths(instance_dir, project_type, target_dir)
        self._validate_install_paths(install_root, destination_dir, project_type)

        projects, by_id = self._resolve_project_tree(
            project_id,
            game_version,
            loader,
            project_type,
            version_id=version_id,
        )
        for item in projects:
            item.publisher_hashes = _publisher_hashes(
                item.primary_file.get("hashes") if isinstance(item.primary_file.get("hashes"), dict) else {}
            )
            item.size = _optional_size(item.primary_file.get("size"))
            if not any(item.publisher_hashes.values()) and not confirm_unverified:
                raise LauncherError(
                    "[unverified-download] Modrinth не предоставил доверенный хеш файла "
                    f"для проекта {item.project_id}. Подтвердите скачивание явно."
                )

        self._prepare_targets(projects, destination_dir, project_type)
        root_project = by_id[project_id]
        if (
            not allow_replace
            and root_project.target is not None
            and (
                os.path.lexists(root_project.target)
                or os.path.lexists(_metadata_path(root_project.target))
            )
        ):
            raise LauncherError(
                f"Ресурс {project_id} уже установлен в этом экземпляре. "
                "Для смены версии используйте обновление, либо сначала удалите ресурс."
            )
        self._stage_and_commit(
            projects,
            by_id,
            install_root,
            destination_dir,
            game_version,
            loader,
            project_type,
            root_project_id=project_id,
            allow_replace=allow_replace,
        )

        root = by_id[project_id]
        if root.target is None:
            raise LauncherError("Не удалось определить путь установленного ресурса")
        name = str(root.project_metadata.get("title") or root.version.get("name") or project_id)
        dependencies = [
            {
                "project_id": dependency_id,
                "version_id": _version_id(by_id[dependency_id].version),
                "dependency_type": "required",
            }
            for dependency_id in root.dependencies
        ]
        return ModResult(
            "modrinth",
            project_id,
            name,
            str(root.version.get("version_number", "")),
            root.target.name,
            root.target,
            _version_id(root.version),
            dependencies,
        )

    def check_updates(
        self,
        instance_dir: Path,
        game_version: str,
        loader: str | None = None,
        project_type: str = "mod",
        *,
        target_dir: Path | None = None,
    ) -> list[dict[str, Any]]:
        self._validate_project_type(project_type)
        install_root, directory = self._resource_paths(instance_dir, project_type, target_dir)
        if install_root.is_symlink():
            raise LauncherError("Каталог экземпляра является символьной ссылкой")
        if not install_root.exists():
            return []
        if not install_root.is_dir() or directory.is_symlink():
            raise LauncherError("Каталог установленного ресурса недоступен")
        if not directory.exists():
            return []
        if not directory.is_dir():
            raise LauncherError("Каталог установленного ресурса недоступен")

        updates: list[dict[str, Any]] = []
        checked: set[str] = set()
        for sidecar in sorted(directory.rglob(f"*{PROJECT_METADATA_SUFFIX}"), key=lambda path: str(path).lower()):
            metadata = _read_metadata(sidecar)
            if not self._trusted_project_metadata(metadata, project_type):
                continue
            project_id = str(metadata.get("project_id", "")).strip()
            target = _target_for_sidecar(sidecar)
            if not project_id or project_id in checked or target is None or target.is_symlink() or not target.is_file():
                continue
            if not _within_resolved(sidecar, directory) or not _within_resolved(target, directory):
                continue
            checked.add(project_id)

            details = self.http.get_json(f"{self.base_url}/project/{quote(project_id, safe='')}")
            if not isinstance(details, dict) or str(details.get("project_type", "")).strip() != project_type:
                continue
            versions = self._compatible_versions(project_id, game_version, loader, project_type)
            latest = versions[0] if versions else None
            latest_version_id = _version_id(latest or {})
            installed_version_id = str(metadata.get("version_id", "")).strip()
            if not latest or not latest_version_id or latest_version_id == installed_version_id:
                continue
            updates.append(
                {
                    "source": "Modrinth",
                    "catalog": "modrinth",
                    "catalog_id": self.catalog_id,
                    "project_id": project_id,
                    "project_type": project_type,
                    "name": str(metadata.get("display_name") or metadata.get("name") or project_id),
                    "current_version": str(metadata.get("version", "")),
                    "current_version_id": installed_version_id,
                    "version": str(latest.get("version_number", "")),
                    "version_id": latest_version_id,
                    "game_version": str(game_version or ""),
                    "loader": str(loader or "vanilla").strip().lower(),
                    "path": str(target),
                }
            )
        return updates

    def _compatible_versions(
        self,
        project_id: str,
        game_version: str,
        loader: str | None,
        project_type: str,
    ) -> list[dict[str, Any]]:
        self._validate_project_type(project_type)
        project_id = _project_id(project_id)
        game_version = str(game_version or "").strip()
        loader = str(loader or "").strip().lower()
        params: dict[str, str] = {}
        if game_version:
            params["game_versions"] = json.dumps([game_version], separators=(",", ":"))
        if loader and loader != "vanilla" and project_type == "mod":
            params["loaders"] = json.dumps([loader], separators=(",", ":"))
        endpoint = f"{self.base_url}/project/{quote(project_id, safe='')}/version"
        url = f"{endpoint}?{urlencode(params)}" if params else endpoint
        value = self.http.get_json(url)
        versions = value if isinstance(value, list) else []
        return [
            item
            for item in versions
            if isinstance(item, dict)
            and _matches_project_type(item, project_type)
            and _version_is_compatible(item, game_version, loader, project_type)
            and (not item.get("project_id") or str(item.get("project_id")) == project_id)
        ]

    def _resolve_project_tree(
        self,
        project_id: str,
        game_version: str,
        loader: str | None,
        project_type: str,
        *,
        version_id: str | None = None,
    ) -> tuple[list[_ResolvedProject], dict[str, _ResolvedProject]]:
        selected: dict[str, _ResolvedProject] = {}
        order: list[str] = []

        def visit(current_id: str, pinned_version_id: str | None, depth: int) -> None:
            current_id = _project_id(current_id)
            pinned_version_id = str(pinned_version_id or "").strip() or None
            existing = selected.get(current_id)
            if existing:
                selected_version_id = _version_id(existing.version)
                if pinned_version_id and pinned_version_id != selected_version_id:
                    raise LauncherError(
                        f"Зависимости требуют разные версии проекта {current_id}; "
                        "установить обе версии одновременно нельзя"
                    )
                return
            if depth > MAX_MOD_DEPENDENCY_DEPTH:
                raise LauncherError("Цепочка зависимостей Modrinth слишком глубокая")
            if len(selected) >= MAX_MOD_DEPENDENCY_PROJECTS:
                raise LauncherError("В дереве зависимостей Modrinth слишком много проектов")

            project_metadata = self._project_metadata_for_install(current_id, project_type)
            versions = self._compatible_versions(current_id, game_version, loader, project_type)
            version = next((item for item in versions if _version_id(item) == pinned_version_id), None) if pinned_version_id else None
            version = version or (versions[0] if versions and not pinned_version_id else None)
            if not version:
                if pinned_version_id:
                    raise LauncherError(
                        f"Обязательная версия {pinned_version_id} проекта {current_id} "
                        "несовместима с выбранной игрой или загрузчиком"
                    )
                raise LauncherError(f"Modrinth не нашёл совместимую версию для {current_id}")
            reported_project_id = str(version.get("project_id") or "").strip()
            if reported_project_id and reported_project_id != current_id:
                raise LauncherError(f"Modrinth вернул версию другого проекта для {current_id}")
            primary = _primary_file(version, current_id)
            file_name = _safe_file_name(str(primary.get("filename") or f"{current_id}.jar"))
            project = _ResolvedProject(current_id, version, project_metadata, primary, file_name)
            selected[current_id] = project
            order.append(current_id)

            direct_dependencies: set[str] = set()
            for dependency_id, dependency_version_id in self._required_dependencies(version, current_id):
                if not dependency_id:
                    dependency_id = self._project_id_for_version(dependency_version_id or "")
                if dependency_id in direct_dependencies:
                    prior = selected.get(dependency_id)
                    if dependency_version_id and prior and _version_id(prior.version) != dependency_version_id:
                        raise LauncherError(f"Зависимости требуют разные версии проекта {dependency_id}")
                    continue
                direct_dependencies.add(dependency_id)
                visit(dependency_id, dependency_version_id, depth + 1)
                project.dependencies.append(dependency_id)

        visit(project_id, version_id, 0)
        return [selected[item] for item in order], selected

    def _required_dependencies(self, version: dict[str, Any], project_id: str) -> list[tuple[str, str | None]]:
        raw = version.get("dependencies", [])
        if raw is None:
            return []
        if not isinstance(raw, list):
            raise LauncherError(f"Список зависимостей проекта {project_id} имеет неожиданный формат")
        if len(raw) > MAX_REQUIRED_DEPENDENCIES_PER_VERSION:
            raise LauncherError(f"У проекта {project_id} слишком много зависимостей")
        dependencies: list[tuple[str, str | None]] = []
        for item in raw:
            if not isinstance(item, dict) or str(item.get("dependency_type") or "").lower() != "required":
                continue
            dependency_id = str(item.get("project_id") or "").strip()
            dependency_version_id = str(item.get("version_id") or "").strip() or None
            if not dependency_id and not dependency_version_id:
                raise LauncherError(f"Обязательная зависимость проекта {project_id} не имеет ID")
            if dependency_id:
                dependency_id = _project_id(dependency_id)
            dependencies.append((dependency_id, dependency_version_id))
        return dependencies

    def _project_id_for_version(self, version_id: str) -> str:
        if not version_id:
            raise LauncherError("Обязательная зависимость Modrinth не имеет project ID")
        value = self.http.get_json(f"{self.base_url}/version/{quote(version_id, safe='')}")
        if not isinstance(value, dict) or not value.get("project_id"):
            raise LauncherError(f"Не удалось определить проект для версии зависимости {version_id}")
        return _project_id(value["project_id"])

    def _project_metadata_for_install(self, project_id: str, project_type: str) -> dict[str, Any]:
        url = f"{self.base_url}/project/{quote(project_id, safe='')}"
        try:
            value = self.http.get_json(url)
        except OperationCancelled:
            raise
        except LauncherError:
            return {}
        if not isinstance(value, dict):
            return {}
        _validate_detail_project_type(value, project_id, project_type)
        return value

    def _resource_paths(
        self,
        instance_dir: Path,
        project_type: str,
        target_dir: Path | None,
    ) -> tuple[Path, Path]:
        if project_type == "datapack":
            if target_dir is None:
                raise LauncherError("Для датапака нужно выбрать мир")
            root = Path(target_dir)
        else:
            root = Path(instance_dir)
        return root, root / PROJECT_DIRECTORIES[project_type]

    def _validate_install_paths(self, install_root: Path, destination_dir: Path, project_type: str) -> None:
        if install_root.is_symlink():
            raise LauncherError("Каталог установки является символьной ссылкой")
        if install_root.exists() and not install_root.is_dir():
            raise LauncherError("Каталог установки недоступен")
        if project_type == "datapack" and not install_root.is_dir():
            raise LauncherError("Мир для установки датапака не найден")
        if destination_dir.is_symlink():
            raise LauncherError("Каталог ресурса является символьной ссылкой")
        if destination_dir.exists() and not destination_dir.is_dir():
            raise LauncherError("Каталог ресурса недоступен")
        if not install_root.exists() and (install_root.parent.is_symlink() or not install_root.parent.is_dir()):
            raise LauncherError("Родительский каталог установки недоступен")

    def _prepare_targets(
        self,
        projects: list[_ResolvedProject],
        directory: Path,
        project_type: str,
    ) -> None:
        if directory.is_symlink():
            raise LauncherError("Каталог ресурса является символьной ссылкой")
        used: set[Path] = set()
        for project in projects:
            target = self._existing_project_target(directory, project, project_type)
            target = target or directory / project.file_name
            sidecar = _metadata_path(target)
            if target in used or sidecar in used:
                raise LauncherError(f"Несколько проектов Modrinth используют файл {target.name}")
            used.update({target, sidecar})
            if not _within(target, directory) or not _within(sidecar, directory):
                raise LauncherError(f"Небезопасный путь файла проекта: {target.name}")
            self._validate_replaceable_pair(target, sidecar, project, project_type)
            project.target = target

    def _existing_project_target(
        self,
        directory: Path,
        project: _ResolvedProject,
        project_type: str,
    ) -> Path | None:
        if not directory.is_dir():
            return None
        matches: list[Path] = []
        for sidecar in directory.glob(f"*{PROJECT_METADATA_SUFFIX}"):
            if sidecar.is_symlink() or not sidecar.is_file():
                continue
            metadata = _read_metadata(sidecar)
            if str(metadata.get("project_id", "")).strip() != project.project_id:
                continue
            if str(metadata.get("project_type", "")).strip() != project_type:
                raise LauncherError(f"Метаданные проекта {project.project_id} содержат другой тип ресурса")
            if not self._same_catalog_metadata(metadata):
                continue
            target = _target_for_sidecar(sidecar)
            if target:
                matches.append(target)
        if len(matches) > 1:
            raise LauncherError(f"Для проекта {project.project_id} найдено несколько установленных файлов")
        return matches[0] if matches else None

    def _validate_replaceable_pair(
        self,
        target: Path,
        sidecar: Path,
        project: _ResolvedProject,
        project_type: str,
    ) -> None:
        if target.is_symlink() or sidecar.is_symlink():
            raise LauncherError("Файл ресурса или его метаданные являются символьной ссылкой")
        if target.exists() and not target.is_file():
            raise LauncherError(f"Путь ресурса уже занят: {target.name}")
        if sidecar.exists() and not sidecar.is_file():
            raise LauncherError(f"Путь метаданных уже занят: {sidecar.name}")
        if not target.exists() and not sidecar.exists():
            return
        metadata = _read_metadata(sidecar)
        if (
            str(metadata.get("project_id", "")).strip() != project.project_id
            or str(metadata.get("project_type", "")).strip() != project_type
            or str(metadata.get("source", "")).casefold() != "modrinth"
            or not self._same_catalog_metadata(metadata)
        ):
            raise LauncherError(f"Файл уже существует и не принадлежит проекту {project.project_id}: {target.name}")

    def _same_catalog_metadata(self, metadata: dict[str, Any]) -> bool:
        catalog = str(metadata.get("catalog", "modrinth")).strip().lower()
        catalog_id = str(metadata.get("catalog_id", self.catalog_id)).strip()
        catalog_url = str(metadata.get("catalog_url", "")).strip()
        return (
            catalog == "modrinth"
            and catalog_id == self.catalog_id
            and (not catalog_url or catalog_url == self.base_url)
        )

    def _stage_and_commit(
        self,
        projects: list[_ResolvedProject],
        by_id: dict[str, _ResolvedProject],
        install_root: Path,
        destination_dir: Path,
        game_version: str,
        loader: str | None,
        project_type: str,
        *,
        root_project_id: str,
        allow_replace: bool,
    ) -> None:
        if destination_dir.is_dir():
            staging_parent = destination_dir
        elif install_root.is_dir():
            staging_parent = install_root
        else:
            staging_parent = install_root.parent
        if staging_parent.is_symlink() or not staging_parent.is_dir():
            raise LauncherError("Не удалось создать временный каталог для ресурсов")

        with tempfile.TemporaryDirectory(prefix=".trial-mods-", dir=staging_parent) as temporary:
            staging = Path(temporary)
            staged_files: list[tuple[_ResolvedProject, Path, Path]] = []
            for index, project in enumerate(projects):
                staged_asset = staging / f"asset-{index}"
                self.http.download(
                    str(project.primary_file["url"]),
                    staged_asset,
                    **project.publisher_hashes,
                    size=project.size,
                )
                metadata = self._sidecar_metadata(project, by_id, game_version, loader, project_type)
                staged_metadata = staging / f"metadata-{index}.json"
                _write_private_json(staged_metadata, metadata)
                if project.target is None:
                    raise LauncherError("Не удалось определить путь установленного ресурса")
                staged_files.append((project, staged_asset, staged_metadata))
            self._commit_staged_files(
                staged_files,
                install_root,
                destination_dir,
                staging,
                project_type,
                root_project_id=root_project_id,
                allow_replace=allow_replace,
            )

    def _sidecar_metadata(
        self,
        project: _ResolvedProject,
        by_id: dict[str, _ResolvedProject],
        game_version: str,
        loader: str | None,
        project_type: str,
    ) -> dict[str, Any]:
        dependencies = [
            {
                "project_id": dependency_id,
                "version_id": _version_id(by_id[dependency_id].version),
                "dependency_type": "required",
            }
            for dependency_id in project.dependencies
        ]
        display_name = str(project.project_metadata.get("title") or project.version.get("name") or project.project_id)
        return {
            "source": "Modrinth",
            "catalog": "modrinth",
            "catalog_id": self.catalog_id,
            "catalog_url": self.base_url,
            "project_id": project.project_id,
            "project_type": project_type,
            "name": display_name,
            "display_name": display_name,
            "version": str(project.version.get("version_number", "")),
            "version_id": _version_id(project.version),
            "game_version": game_version,
            "loader": str(loader or "vanilla").strip().lower(),
            "dependencies": dependencies,
            "author": str(project.project_metadata.get("author") or ""),
            "icon_url": str(project.project_metadata.get("icon_url") or project.project_metadata.get("icon") or ""),
        }

    def _commit_staged_files(
        self,
        staged_files: list[tuple[_ResolvedProject, Path, Path]],
        install_root: Path,
        destination_dir: Path,
        staging: Path,
        project_type: str,
        *,
        root_project_id: str,
        allow_replace: bool,
    ) -> None:
        created_directories: list[Path] = []
        installed_paths: list[Path] = []
        backups: list[tuple[Path, Path]] = []
        try:
            for directory in (install_root, destination_dir):
                if not directory.exists():
                    directory.mkdir()
                    created_directories.append(directory)
                if directory.is_symlink() or not directory.is_dir():
                    raise LauncherError("Каталог установки изменился во время скачивания")

            backup_directory = staging / "backups"
            backup_directory.mkdir()
            for index, (project, staged_asset, staged_metadata) in enumerate(staged_files):
                if project.target is None:
                    raise LauncherError("Не удалось определить путь установленного ресурса")
                target = project.target
                sidecar = _metadata_path(target)
                if (
                    project.project_id == root_project_id
                    and not allow_replace
                    and (os.path.lexists(target) or os.path.lexists(sidecar))
                ):
                    raise LauncherError(f"Ресурс {root_project_id} уже установлен в этом экземпляре")
                self._validate_replaceable_pair(target, sidecar, project, project_type)
                for suffix, existing in (("asset", target), ("metadata", sidecar)):
                    if os.path.lexists(existing):
                        backup = backup_directory / f"{index}-{suffix}"
                        os.replace(existing, backup)
                        backups.append((backup, existing))
                os.replace(staged_asset, target)
                installed_paths.append(target)
                os.replace(staged_metadata, sidecar)
                installed_paths.append(sidecar)
        except Exception:
            for path in reversed(installed_paths):
                try:
                    if path.is_file() or path.is_symlink():
                        path.unlink()
                except OSError:
                    pass
            for backup, original in reversed(backups):
                try:
                    if os.path.lexists(original):
                        original.unlink()
                    os.replace(backup, original)
                except OSError:
                    pass
            for directory in reversed(created_directories):
                try:
                    directory.rmdir()
                except OSError:
                    pass
            raise

    def _trusted_project_metadata(self, metadata: dict[str, Any], project_type: str) -> bool:
        return (
            str(metadata.get("source", "")).casefold() == "modrinth"
            and str(metadata.get("catalog", "")).strip().lower() == "modrinth"
            and str(metadata.get("catalog_id", "")).strip() == self.catalog_id
            and str(metadata.get("catalog_url", "")).strip() == self.base_url
            and str(metadata.get("project_type", "")).strip() == project_type
            and bool(str(metadata.get("project_id", "")).strip())
            and bool(str(metadata.get("version_id", "")).strip())
            and isinstance(metadata.get("game_version"), str)
            and isinstance(metadata.get("loader"), str)
        )

    @staticmethod
    def _validate_project_type(project_type: str) -> None:
        if project_type not in PROJECT_DIRECTORIES:
            raise LauncherError(f"Неизвестный тип Modrinth-проекта: {project_type}")

def _project_id(value: Any) -> str:
    project_id = str(value or "").strip()
    if (
        not project_id
        or len(project_id) > 200
        or project_id in {".", ".."}
        or any(ord(character) < 32 or ord(character) == 127 for character in project_id)
    ):
        raise LauncherError("Некорректный project ID ресурса")
    return project_id

def _validate_detail_project_type(value: dict[str, Any], project_id: str, expected: str) -> None:
    actual = str(value.get("project_type") or "").strip()
    if actual and actual != expected:
        raise LauncherError(
            f"Modrinth-проект {project_id} имеет тип {actual}, ожидался {expected}"
        )

def _matches_project_type(value: dict[str, Any], expected: str) -> bool:
    actual = str(value.get("project_type") or "").strip()
    return not actual or actual == expected

def _version_is_compatible(
    version: dict[str, Any],
    game_version: str,
    loader: str,
    project_type: str,
) -> bool:
    if game_version and "game_versions" in version:
        game_versions = version.get("game_versions")
        if not isinstance(game_versions, list) or game_version not in game_versions:
            return False
    if project_type == "mod" and loader and loader != "vanilla" and "loaders" in version:
        loaders = version.get("loaders")
        if not isinstance(loaders, list) or loader not in loaders:
            return False
    return True

def _version_id(version: dict[str, Any]) -> str:
    return str(version.get("id") or version.get("version_id") or "").strip()

def _primary_file(version: dict[str, Any], project_id: str) -> dict[str, Any]:
    files = version.get("files", [])
    files = [item for item in files if isinstance(item, dict)] if isinstance(files, list) else []
    primary = next((item for item in files if item.get("primary")), files[0] if files else None)
    if not primary or not str(primary.get("url") or "").strip():
        raise LauncherError(f"У Modrinth-проекта {project_id} нет файла для скачивания")
    return primary

def _metadata_path(target: Path) -> Path:
    return target.with_name(f".{target.name}{PROJECT_METADATA_SUFFIX}")

def _target_for_sidecar(sidecar: Path) -> Path | None:
    if not sidecar.name.startswith(".") or not sidecar.name.endswith(PROJECT_METADATA_SUFFIX):
        return None
    file_name = sidecar.name[1 : -len(PROJECT_METADATA_SUFFIX)]
    return sidecar.with_name(file_name) if file_name else None

def _read_metadata(path: Path) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        return {}
    try:
        if path.stat().st_size > MAX_PROJECT_METADATA_BYTES:
            return {}
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError, json.JSONDecodeError):
        return {}
    return value if isinstance(value, dict) else {}

def _write_private_json(path: Path, value: dict[str, Any]) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    set_private_fd(fd)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False)

def _safe_file_name(value: str) -> str:
    normalized = value.replace("\\", "/")
    name = Path(normalized).name
    if not name or name in {".", ".."} or "/" in normalized or name != value:
        raise LauncherError(f"Небезопасное имя файла мода: {value}")
    return name

def _publisher_hashes(value: dict[str, Any]) -> dict[str, str | None]:
    lengths = {"sha1": 40, "sha256": 64, "sha512": 128}
    hashes: dict[str, str | None] = {}
    for algorithm, length in lengths.items():
        digest = str(value.get(algorithm, "")).strip()
        if len(digest) != length or any(character not in "0123456789abcdefABCDEF" for character in digest):
            digest = ""
        hashes[algorithm] = digest or None
    return hashes

def _optional_size(value: Any) -> int | None:
    try:
        size = int(value)
    except (TypeError, ValueError):
        return None
    return size if size > 0 else None

def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False

def _within_resolved(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except (OSError, RuntimeError, ValueError):
        return False
