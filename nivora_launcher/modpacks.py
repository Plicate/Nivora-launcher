from __future__ import annotations

import json
import hashlib
import os
import shutil
import stat
import tempfile
import uuid
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable
from urllib.parse import quote, urlencode, urlsplit

from .constants import FTB_MODPACK_API_URL, MODRINTH_API_URL, TECHNIC_API_URL
from .errors import LauncherError
from .net import HttpClient


MAX_MODPACK_BYTES = 512 * 1024 * 1024
MAX_MODPACK_ENTRIES = 20_000
MAX_MODPACK_UNCOMPRESSED_BYTES = 2 * 1024 * 1024 * 1024
MAX_MODPACK_INDEX_BYTES = 32 * 1024 * 1024
MAX_MODPACK_MARKER_BYTES = 512 * 1024
MODPACK_MARKER = ".trial-launcher-modpacks.json"
# CurseForge-архивы хранят manifest.json со списком файлов как projectID/fileID:
# сами моды в архиве НЕТ, их нужно до Resolv-ить и скачать. Без этого импорт
# «притворялся успешным», ставя только overrides — а моды молча пропадали.
CURSEFORGE_MANIFEST_NAME = "manifest.json"
CURSEFORGE_MANIFEST_MAX_BYTES = 8 * 1024 * 1024
CURSEFORGE_RESOLVER_BASE = "https://api.cfwidget.com"
CURSEFORGE_DOWNLOAD_HOST_SUFFIX = "forgecdn.net"
CURSEFORGE_MAX_REMOTE_FILES = 2000
MOD_PREVIEW_LIMIT = 400

DEFAULT_MODPACK_CATALOGS = [
    {"id": "modrinth", "name": "Modrinth", "kind": "modrinth", "api_url": MODRINTH_API_URL},
    {"id": "ftb", "name": "Feed The Beast", "kind": "ftb", "api_url": FTB_MODPACK_API_URL},
    {"id": "technic", "name": "Technic", "kind": "technic", "api_url": TECHNIC_API_URL},
]


@dataclass
class ModpackResult:
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
    game_versions: list[str] = field(default_factory=list)
    manifest_files: list[dict[str, Any]] = field(default_factory=list, repr=False)

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
            "game_versions": self.game_versions,
            "project_type": "modpack",
        }


class ModpackProvider:
    """Search and download modpacks from the supported public catalogs."""

    def __init__(self, http: HttpClient, catalog: dict[str, Any]) -> None:
        self.http = http
        self.kind = str(catalog.get("kind", "modrinth")).strip().lower()
        self.catalog_id = str(catalog.get("id") or self.kind).strip().lower()
        self.base_url = str(catalog.get("api_url", "")).rstrip("/")
        if self.kind not in {"modrinth", "ftb", "technic"}:
            raise LauncherError(f"Каталог {self.kind} не поддерживает модпаки")

    def search(self, query: str, game_version: str = "", limit: int = 12, offset: int = 0) -> list[ModpackResult]:
        limit = max(1, min(30, int(limit)))
        self.last_total: int | None = None
        if self.kind == "modrinth":
            return self._search_modrinth(query, game_version, limit, max(0, min(10_000, int(offset or 0))))
        if self.kind == "ftb":
            return self._search_ftb(query, game_version, limit)
        return self._search_technic(query, game_version, limit)

    def project_details(self, project_id: str) -> dict[str, Any]:
        project_id = str(project_id).strip()
        if not project_id:
            raise LauncherError("Не указан ID модпака")
        encoded_id = quote(project_id, safe="")
        if self.kind == "modrinth":
            value = self.http.get_json(f"{self.base_url}/project/{encoded_id}")
            if not isinstance(value, dict) or str(value.get("project_type", "")) != "modpack":
                raise LauncherError("Каталог не вернул подробности модпака")
            return value
        if self.kind == "ftb":
            value = self.http.get_json(f"{self.base_url}/modpack/{encoded_id}")
            if not isinstance(value, dict):
                raise LauncherError("Каталог FTB не вернул подробности модпака")
            return {**value, **_ftb_result(value).as_dict()}
        value = self.http.get_json(f"{self.base_url}/modpack/{encoded_id}?{urlencode({'build': 'multimc'})}")
        if not isinstance(value, dict):
            raise LauncherError("Каталог Technic не вернул подробности модпака")
        return {**value, **_technic_result(value).as_dict()}

    def install(
        self,
        project_id: str,
        target: Path,
        game_version: str = "",
        *,
        version: str = "",
        display_name: str = "",
        loader: str = "",
        loader_version: str | None = None,
        confirm_unverified: bool = False,
    ) -> dict[str, Any]:
        project_id = str(project_id or "").strip()
        _assert_modpack_not_installed(
            target,
            source=self.kind,
            project_id=project_id,
            catalog_id=self.catalog_id,
        )
        result, hashes, size = self._resolve_archive(project_id, game_version, version)
        publisher_hashes = _publisher_hashes(hashes)
        if self.kind == "ftb" and result.manifest_files:
            return ModpackImporter(self.http).import_manifest(
                result.manifest_files,
                target,
                source=self.kind,
                project_id=result.project_id,
                catalog_id=self.catalog_id,
                display_name=display_name or result.title,
                version=result.version,
                confirm_unverified=confirm_unverified,
            )
        if not result.download_url:
            raise LauncherError("Каталог не предоставил архив модпака для скачивания")
        if size is not None and size > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        if not any(publisher_hashes.values()) and not confirm_unverified:
            raise _unverified_download_error("Каталог модпаков не предоставил доверенный хеш архива")
        _validate_archive_url(result.download_url)
        with tempfile.TemporaryDirectory(prefix="trial-launcher-modpack-") as directory:
            archive_path = Path(directory) / (result.file_name or "modpack.zip")
            self.http.download(
                result.download_url,
                archive_path,
                **publisher_hashes,
                size=size,
            )
            imported = ModpackImporter(self.http).import_file(
                archive_path,
                target,
                source=self.kind,
                project_id=result.project_id,
                catalog_id=self.catalog_id,
                display_name=display_name or result.title,
                version=result.version,
                confirm_unverified=confirm_unverified,
                expected_game_version=game_version,
                expected_loader=loader,
                expected_loader_version=loader_version,
            )
            imported["trusted_hash"] = bool(any(publisher_hashes.values()) and imported.get("trusted_hash"))
            return imported

    def preflight(
        self,
        project_id: str,
        game_version: str = "",
        *,
        version: str = "",
        loader: str = "",
        loader_version: str | None = None,
        confirm_unverified: bool = False,
    ) -> dict[str, Any]:
        result, hashes, size = self._resolve_archive(project_id, game_version, version)
        if self.kind == "ftb" and result.manifest_files:
            importer = ModpackImporter(self.http)
            importer.preflight_manifest(
                result.manifest_files,
                confirm_unverified=confirm_unverified,
            )
            plan = importer._manifest_download_plan(result.manifest_files, confirm_unverified=True)
            mods = [
                {"name": item.get("preview_name") or Path(item["destination"]).name, "source": "ftb", "optional": False}
                for item in plan
            ]
            return {
                "minecraft": "",
                "loader": "",
                "loader_version": "",
                "compatible": True,
                "issues": [],
                "mods": {"total": len(mods), "mods": mods[:MOD_PREVIEW_LIMIT], "truncated": len(mods) > MOD_PREVIEW_LIMIT},
            }
        if not result.download_url:
            raise LauncherError("Каталог не предоставил архив модпака для скачивания")
        if size is not None and size > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        publisher_hashes = _publisher_hashes(hashes)
        if not any(publisher_hashes.values()) and not confirm_unverified:
            raise _unverified_download_error("Каталог модпаков не предоставил доверенный хеш архива")
        _validate_archive_url(result.download_url)
        is_mrpack = result.file_name.lower().endswith(".mrpack") or urlsplit(result.download_url).path.lower().endswith(".mrpack")
        if not is_mrpack:
            return {}
        with tempfile.TemporaryDirectory(prefix="trial-launcher-modpack-preflight-") as directory:
            archive_path = Path(directory) / (result.file_name or "modpack.mrpack")
            self.http.download(
                result.download_url,
                archive_path,
                **publisher_hashes,
                size=size,
            )
            dependencies = ModpackImporter(self.http).preflight_file(
                archive_path,
                confirm_unverified=confirm_unverified,
                expected_game_version=game_version,
                expected_loader=loader,
                expected_loader_version=loader_version,
                strict_compatibility=False,
            )
            try:
                dependencies["mods"] = ModpackImporter(self.http).mod_preview_file(archive_path)
            except LauncherError:
                pass
            return dependencies

    def _search_modrinth(self, query: str, game_version: str, limit: int, offset: int = 0) -> list[ModpackResult]:
        facets = [["project_type:modpack"]]
        if game_version:
            facets.append([f"versions:{game_version}"])
        params: dict[str, str] = {"limit": str(limit), "facets": json.dumps(facets, separators=(",", ":"))}
        if offset:
            params["offset"] = str(offset)
        if query.strip():
            params["query"] = query.strip()
        value = self.http.get_json(f"{self.base_url}/search?{urlencode(params)}")
        hits = value.get("hits", []) if isinstance(value, dict) else []
        total = value.get("total_hits") if isinstance(value, dict) else None
        self.last_total = int(total) if isinstance(total, int) and total >= 0 else None
        return [self._modrinth_result(item) for item in hits if isinstance(item, dict)]

    def _search_ftb(self, query: str, game_version: str, limit: int) -> list[ModpackResult]:
        value = self.http.get_json(f"{self.base_url}/modpack/all")
        rows = _rows(value, "modpacks", "packs", "results")
        needle = query.strip().casefold()
        results: list[ModpackResult] = []
        for item in rows:
            result = _ftb_result(item)
            haystack = f"{result.title} {result.description} {result.author}".casefold()
            if needle and needle not in haystack:
                continue
            if game_version and result.game_versions and game_version not in result.game_versions:
                continue
            results.append(result)
            if len(results) >= limit:
                break
        return results

    def _search_technic(self, query: str, game_version: str, limit: int) -> list[ModpackResult]:
        params = {"build": "multimc", "q": query.strip() or "modpack"}
        value = self.http.get_json(f"{self.base_url}/search?{urlencode(params)}")
        rows = _rows(value, "modpacks", "results", "packs")
        results: list[ModpackResult] = []
        for item in rows:
            result = _technic_result(item)
            if game_version and result.game_versions and game_version not in result.game_versions:
                continue
            results.append(result)
            if len(results) >= limit:
                break
        return results

    def _resolve_archive(self, project_id: str, game_version: str, version: str) -> tuple[ModpackResult, dict[str, str], int | None]:
        if self.kind == "modrinth":
            return self._resolve_modrinth(project_id, game_version, version)
        if self.kind == "ftb":
            return self._resolve_ftb(project_id, game_version, version)
        return self._resolve_technic(project_id, game_version, version)

    def _resolve_modrinth(self, project_id: str, game_version: str, version: str) -> tuple[ModpackResult, dict[str, str], int | None]:
        params: dict[str, str] = {}
        if game_version:
            params["game_versions"] = json.dumps([game_version])
        endpoint = f"{self.base_url}/project/{quote(project_id, safe='')}/version"
        value = self.http.get_json(f"{endpoint}?{urlencode(params)}" if params else endpoint)
        versions = value if isinstance(value, list) else []
        selected = next((item for item in versions if str(item.get("version_number", "")) == version), None) if version else None
        selected = selected or (versions[0] if versions else {})
        files = selected.get("files", []) if isinstance(selected, dict) else []
        primary = next((item for item in files if item.get("primary")), files[0] if files else {})
        if not isinstance(primary, dict):
            primary = {}
        hashes = primary.get("hashes", {}) if isinstance(primary.get("hashes"), dict) else {}
        metadata = self.http.get_json(f"{self.base_url}/project/{quote(project_id, safe='')}")
        metadata = metadata if isinstance(metadata, dict) else {}
        result = ModpackResult(
            "modrinth",
            project_id,
            _text(metadata.get("title") or metadata.get("name") or project_id),
            _text(metadata.get("description")),
            _text(metadata.get("author")),
            _text(metadata.get("icon_url") or metadata.get("icon")),
            _int(metadata.get("downloads")),
            _text(selected.get("version_number")),
            _text(primary.get("url")),
            _safe_archive_name(_text(primary.get("filename")) or f"{project_id}.mrpack"),
            [str(item) for item in selected.get("game_versions", []) if item] if isinstance(selected, dict) and isinstance(selected.get("game_versions"), list) else [],
        )
        return result, {key: _text(hashes.get(key)) for key in ("sha1", "sha256", "sha512")}, _optional_int(primary.get("size"))

    def _resolve_ftb(self, project_id: str, game_version: str, version: str) -> tuple[ModpackResult, dict[str, str], int | None]:
        value = self.http.get_json(f"{self.base_url}/modpack/{quote(project_id, safe='')}")
        item = value if isinstance(value, dict) else {}
        versions = _rows(item, "versions", "releases", "builds")
        selected = _pick_version(versions, game_version, version)
        version_id = _text(selected.get("id") or selected.get("version_id") or selected.get("version") or item.get("version_id"))
        detail = item
        if version_id:
            detail_value = self.http.get_json(f"{self.base_url}/modpack/{quote(project_id, safe='')}/{quote(version_id, safe='')}")
            if isinstance(detail_value, dict):
                detail = detail_value
        archive = _find_archive(detail)
        result = _ftb_result({**item, **detail, "id": project_id, "version": version_id, **archive})
        result.manifest_files = _ftb_manifest_files(detail)
        hashes = {key: _text(archive.get(key)) for key in ("sha1", "sha256", "sha512")}
        return result, hashes, _optional_int(archive.get("size"))

    def _resolve_technic(self, project_id: str, game_version: str, version: str) -> tuple[ModpackResult, dict[str, str], int | None]:
        value = self.http.get_json(f"{self.base_url}/modpack/{quote(project_id, safe='')}?{urlencode({'build': 'multimc'})}")
        item = value if isinstance(value, dict) else {}
        builds = _rows(item, "builds", "versions", "releases")
        selected = _pick_version(builds, game_version, version)
        detail = {**item, **selected}
        archive = _find_archive(detail)
        result = _technic_result({**detail, "slug": project_id, **archive})
        hashes = {key: _text(archive.get(key)) for key in ("sha1", "sha256", "sha512")}
        return result, hashes, _optional_int(archive.get("size"))

    @staticmethod
    def _modrinth_result(item: dict[str, Any]) -> ModpackResult:
        return ModpackResult(
            "modrinth",
            _text(item.get("project_id") or item.get("id") or item.get("slug")),
            _text(item.get("title") or item.get("name") or "Modpack"),
            _text(item.get("description")),
            _text(item.get("author")),
            _text(item.get("icon_url") or item.get("icon")),
            _int(item.get("downloads")),
        )


class ModpackImporter:
    def __init__(self, http: HttpClient) -> None:
        self.http = http

    def import_bytes(
        self,
        content: bytes,
        target: Path,
        *,
        file_name: str = "modpack.zip",
        source: str = "local",
        project_id: str = "",
        catalog_id: str = "",
        display_name: str = "",
        version: str = "",
        confirm_unverified: bool = False,
        expected_game_version: str = "",
        expected_loader: str = "",
        expected_loader_version: str | None = None,
    ) -> dict[str, Any]:
        if len(content) > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        safe_name = _safe_archive_name(file_name)
        with tempfile.TemporaryDirectory(prefix="trial-launcher-modpack-import-") as directory:
            archive_path = Path(directory) / safe_name
            archive_path.write_bytes(content)
            return self.import_file(
                archive_path,
                target,
                source=source,
                project_id=project_id,
                catalog_id=catalog_id,
                display_name=display_name or Path(safe_name).stem,
                version=version,
                confirm_unverified=confirm_unverified,
                expected_game_version=expected_game_version,
                expected_loader=expected_loader,
                expected_loader_version=expected_loader_version,
            )

    def preflight_bytes(
        self,
        content: bytes,
        *,
        file_name: str = "modpack.zip",
        confirm_unverified: bool = False,
        expected_game_version: str = "",
        expected_loader: str = "",
        expected_loader_version: str | None = None,
    ) -> dict[str, str]:
        if len(content) > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        safe_name = _safe_archive_name(file_name)
        with tempfile.TemporaryDirectory(prefix="trial-launcher-modpack-preflight-") as directory:
            archive_path = Path(directory) / safe_name
            archive_path.write_bytes(content)
            return self.preflight_file(
                archive_path,
                confirm_unverified=confirm_unverified,
                expected_game_version=expected_game_version,
                expected_loader=expected_loader,
                expected_loader_version=expected_loader_version,
            )

    def preflight_file(
        self,
        archive_path: Path,
        *,
        confirm_unverified: bool = False,
        expected_game_version: str = "",
        expected_loader: str = "",
        expected_loader_version: str | None = None,
        strict_compatibility: bool = True,
    ) -> dict[str, Any]:
        if archive_path.is_symlink() or not archive_path.is_file():
            raise LauncherError("Архив модпака не найден")
        if archive_path.stat().st_size > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        try:
            with zipfile.ZipFile(archive_path) as archive:
                index, manifest, selected = self._archive_plan(archive)[:3]
            dependencies: dict[str, Any]
            if manifest:
                dependencies = _validate_curseforge_compatibility(
                    manifest,
                    expected_game_version,
                    expected_loader,
                    expected_loader_version,
                    strict=strict_compatibility,
                )
                self._curseforge_download_plan(manifest, confirm_unverified=confirm_unverified)
            else:
                dependencies = _validate_mrpack_compatibility(
                    index,
                    expected_game_version,
                    expected_loader,
                    expected_loader_version,
                    strict=strict_compatibility,
                )
                self._modrinth_download_plan(index, confirm_unverified)
            return dependencies
        except zipfile.BadZipFile as exc:
            raise LauncherError("Файл не является корректным ZIP-модпаком") from exc

    def mod_preview_file(self, archive_path: Path) -> dict[str, Any]:
        """Summarize the mods inside an archive for the import preview dialog."""
        if archive_path.is_symlink() or not archive_path.is_file():
            raise LauncherError("Архив модпака не найден")
        if archive_path.stat().st_size > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        try:
            with zipfile.ZipFile(archive_path) as archive:
                index, manifest, selected = self._archive_plan(archive)[:3]
        except zipfile.BadZipFile as exc:
            raise LauncherError("Файл не является корректным ZIP-модпаком") from exc
        mods: list[dict[str, Any]] = []
        seen: set[str] = set()

        def add(name: str, source: str, optional: bool = False) -> None:
            clean = name.strip()
            if not clean or clean.lower() in seen:
                return
            seen.add(clean.lower())
            mods.append({"name": clean[:160], "source": source, "optional": optional})

        for entry in index.get("files", []) if isinstance(index.get("files"), list) else []:
            if not isinstance(entry, dict):
                continue
            path = _text(entry.get("path"))
            if path.lower().startswith("mods/") and path.lower().endswith(".jar"):
                add(Path(path.replace("\\", "/")).name, "modrinth")
        for _info, destination in selected:
            if destination.lower().startswith("mods/") and destination.lower().endswith(".jar"):
                add(Path(destination).name, "archive")
        if manifest:
            plan = self._curseforge_download_plan(manifest, confirm_unverified=True, resolve=True)
            for item in plan:
                add(item.get("preview_name") or item.get("destination") or "", "curseforge", bool(item.get("optional")))
        return {"total": len(mods), "mods": mods[:MOD_PREVIEW_LIMIT], "truncated": len(mods) > MOD_PREVIEW_LIMIT}

    def preflight_manifest(
        self,
        entries: list[dict[str, Any]],
        *,
        confirm_unverified: bool = False,
    ) -> None:
        self._manifest_download_plan(entries, confirm_unverified)

    def mod_preview_bytes(self, content: bytes, *, file_name: str = "modpack.zip") -> dict[str, Any]:
        if len(content) > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        safe_name = _safe_archive_name(file_name)
        with tempfile.TemporaryDirectory(prefix="trial-launcher-modpack-preview-") as directory:
            archive_path = Path(directory) / safe_name
            archive_path.write_bytes(content)
            return self.mod_preview_file(archive_path)

    def import_file(
        self,
        archive_path: Path,
        target: Path,
        *,
        source: str,
        project_id: str,
        display_name: str,
        version: str,
        catalog_id: str = "",
        confirm_unverified: bool = False,
        expected_game_version: str = "",
        expected_loader: str = "",
        expected_loader_version: str | None = None,
    ) -> dict[str, Any]:
        if archive_path.is_symlink() or not archive_path.is_file():
            raise LauncherError("Архив модпака не найден")
        if archive_path.stat().st_size > MAX_MODPACK_BYTES:
            raise LauncherError("Архив модпака слишком большой (максимум 512 MiB)")
        if target.is_symlink() or not target.is_dir():
            raise LauncherError("Папка экземпляра модпака недоступна")
        archive_sha256 = _sha256_file(archive_path)
        _assert_modpack_not_installed(
            target,
            source=source,
            project_id=project_id,
            catalog_id=catalog_id,
            archive_sha256=archive_sha256 if source == "local" or not project_id else "",
        )
        self.preflight_file(
            archive_path,
            confirm_unverified=confirm_unverified,
            expected_game_version=expected_game_version,
            expected_loader=expected_loader,
            expected_loader_version=expected_loader_version,
        )
        stage = target / f".trial-launcher-modpack-{uuid.uuid4().hex}.part"
        if stage.exists() or stage.is_symlink():
            raise LauncherError("Временная папка модпака уже существует")
        stage.mkdir(mode=0o700)
        try:
            files = self._stage_archive(
                archive_path,
                stage,
                confirm_unverified=confirm_unverified,
                expected_game_version=expected_game_version,
                expected_loader=expected_loader,
                expected_loader_version=expected_loader_version,
            )
            written = self._merge(
                stage,
                target,
                finalize=lambda written: _record_modpack(
                    target,
                    source,
                    project_id,
                    display_name,
                    version,
                    written,
                    catalog_id=catalog_id,
                    archive_sha256=archive_sha256,
                ),
            )
            marker = target / MODPACK_MARKER
            return {
                "source": source,
                "project_id": project_id,
                "name": display_name[:160] or "Modpack",
                "version": version[:80],
                "file_name": archive_path.name,
                "path": str(target),
                "files": written,
                "trusted_hash": bool(files.get("trusted_hash")),
                "skipped_loader_files": list(files.get("skipped_loader_files") or []),
                "marker": str(marker),
            }
        finally:
            shutil.rmtree(stage, ignore_errors=True)

    def import_manifest(
        self,
        entries: list[dict[str, Any]],
        target: Path,
        *,
        source: str,
        project_id: str,
        display_name: str,
        version: str,
        catalog_id: str = "",
        confirm_unverified: bool = False,
    ) -> dict[str, Any]:
        if not isinstance(entries, list) or not entries:
            raise LauncherError("Манифест модпака не содержит файлов")
        if target.is_symlink() or not target.is_dir():
            raise LauncherError("Папка экземпляра модпака недоступна")
        _assert_modpack_not_installed(target, source=source, project_id=project_id, catalog_id=catalog_id)
        plan = self._manifest_download_plan(entries, confirm_unverified)
        stage = target / f".trial-launcher-modpack-{uuid.uuid4().hex}.part"
        if stage.exists() or stage.is_symlink():
            raise LauncherError("Временная папка модпака уже существует")
        stage.mkdir(mode=0o700)
        try:
            result = self._stage_manifest(plan, stage)
            written = self._merge(
                stage,
                target,
                finalize=lambda written: _record_modpack(
                    target,
                    source,
                    project_id,
                    display_name,
                    version,
                    written,
                    catalog_id=catalog_id,
                ),
            )
            marker = target / MODPACK_MARKER
            return {
                "source": source,
                "project_id": project_id,
                "name": display_name[:160] or "Modpack",
                "version": version[:80],
                "file_name": "ftb-manifest",
                "path": str(target),
                "files": written,
                "trusted_hash": bool(result.get("trusted_hash")),
                "marker": str(marker),
            }
        finally:
            shutil.rmtree(stage, ignore_errors=True)

    @staticmethod
    def _archive_plan(archive: zipfile.ZipFile) -> tuple[dict[str, Any], dict[str, Any], list[tuple[zipfile.ZipInfo, str]]]:
        infos = archive.infolist()
        if len(infos) > MAX_MODPACK_ENTRIES:
            raise LauncherError("В модпаке слишком много файлов")
        names = [_archive_name(info.filename) for info in infos]
        if len(set(names)) != len(names):
            raise LauncherError("Модпак содержит повторные пути")
        if any(_is_symlink(info) for info in infos):
            raise LauncherError("Символьные ссылки в модпаке запрещены")
        prefix = _archive_prefix(names)
        index_name = next((name for name in names if name.removeprefix(prefix) == "modrinth.index.json"), "")
        index: dict[str, Any] = {}
        if index_name:
            try:
                index_info = next(info for info, name in zip(infos, names) if name == index_name)
                if index_info.file_size > MAX_MODPACK_INDEX_BYTES:
                    raise LauncherError("modrinth.index.json слишком большой")
                with archive.open(index_info) as index_file:
                    index_bytes = index_file.read(MAX_MODPACK_INDEX_BYTES + 1)
                if len(index_bytes) > MAX_MODPACK_INDEX_BYTES or len(index_bytes) != index_info.file_size:
                    raise LauncherError("modrinth.index.json слишком большой или повреждён")
                index_value = json.loads(index_bytes.decode("utf-8"))
            except LauncherError:
                raise
            except (UnicodeDecodeError, ValueError, KeyError, zipfile.BadZipFile) as exc:
                raise LauncherError("modrinth.index.json повреждён") from exc
            if not isinstance(index_value, dict):
                raise LauncherError("Индекс модпака имеет неожиданный формат")
            index = index_value

        manifest: dict[str, Any] = {}
        manifest_name = next((name for name in names if name.removeprefix(prefix) == CURSEFORGE_MANIFEST_NAME), "")
        if manifest_name and not index_name:
            try:
                manifest_info = next(info for info, name in zip(infos, names) if name == manifest_name)
                if manifest_info.file_size > CURSEFORGE_MANIFEST_MAX_BYTES:
                    raise LauncherError("manifest.json слишком большой")
                with archive.open(manifest_info) as manifest_file:
                    manifest_bytes = manifest_file.read(CURSEFORGE_MANIFEST_MAX_BYTES + 1)
                if len(manifest_bytes) > CURSEFORGE_MANIFEST_MAX_BYTES or len(manifest_bytes) != manifest_info.file_size:
                    raise LauncherError("manifest.json слишком большой или повреждён")
                manifest_value = json.loads(manifest_bytes.decode("utf-8"))
            except LauncherError:
                raise
            except (UnicodeDecodeError, ValueError, KeyError, zipfile.BadZipFile) as exc:
                raise LauncherError("manifest.json CurseForge повреждён") from exc
            if not isinstance(manifest_value, dict):
                raise LauncherError("manifest.json CurseForge имеет неожиданный формат")
            files = manifest_value.get("files")
            if files is not None and (not isinstance(files, list) or len(files) > CURSEFORGE_MAX_REMOTE_FILES):
                raise LauncherError("manifest.json CurseForge содержит слишком много файлов")
            manifest = manifest_value

        selected: list[tuple[zipfile.ZipInfo, str]] = []
        destinations: set[str] = set()
        skipped_loader: list[str] = []
        for info, name in zip(infos, names):
            relative = name.removeprefix(prefix)
            if not relative or info.is_dir() or relative in {index_name, manifest_name}:
                continue
            destination = _archive_destination(relative)
            if not destination:
                if _loader_managed_path(relative) and len(skipped_loader) < 100:
                    skipped_loader.append(_strip_modpack_prefix(relative))
                continue
            if destination in destinations:
                raise LauncherError(f"Модпак содержит повторный путь: {destination}")
            destinations.add(destination)
            selected.append((info, destination))
        return index, manifest, selected, skipped_loader

    @staticmethod
    def _modrinth_download_plan(index: dict[str, Any], confirm_unverified: bool) -> list[dict[str, Any]]:
        entries = index.get("files", [])
        if not entries:
            return []
        if not isinstance(entries, list) or len(entries) > MAX_MODPACK_ENTRIES:
            raise LauncherError("Индекс модпака содержит слишком много файлов")
        plan: list[dict[str, Any]] = []
        destinations: set[str] = set()
        for entry in entries:
            if not isinstance(entry, dict):
                raise LauncherError("Индекс модпака содержит некорректный файл")
            raw_path = str(entry.get("path", ""))
            if _is_server_override(raw_path):
                continue
            if _loader_managed_path(raw_path):
                # Лаунчер сам управляет versions/ и libraries/: удалённый
                # файл оттуда мог бы перезаписать модлоадер.
                continue
            relative = _archive_destination(raw_path)
            downloads = entry.get("downloads", [])
            if not relative or not isinstance(downloads, list) or not downloads:
                raise LauncherError("Индекс модпака содержит небезопасный или неполный файл")
            if relative in destinations:
                raise LauncherError(f"Индекс модпака содержит повторный путь: {relative}")
            destinations.add(relative)
            url = str(downloads[0]).strip()
            _validate_archive_url(url)
            hashes = entry.get("hashes", {}) if isinstance(entry.get("hashes"), dict) else {}
            publisher_hashes = _publisher_hashes(hashes)
            trusted = any(publisher_hashes.values())
            if not trusted and not confirm_unverified:
                raise _unverified_download_error("Файл модпака не имеет доверенного хеша")
            size = _optional_int(entry.get("fileSize"))
            if size is not None and size > MAX_MODPACK_UNCOMPRESSED_BYTES:
                raise LauncherError("Распакованный модпак слишком большой")
            plan.append({"destination": relative, "url": url, "hashes": publisher_hashes, "size": size, "trusted": trusted})
        return plan

    @staticmethod
    def _manifest_download_plan(entries: list[dict[str, Any]], confirm_unverified: bool) -> list[dict[str, Any]]:
        if not isinstance(entries, list) or not entries:
            raise LauncherError("Манифест модпака не содержит файлов")
        if len(entries) > MAX_MODPACK_ENTRIES:
            raise LauncherError("Манифест модпака содержит слишком много файлов")
        plan: list[dict[str, Any]] = []
        destinations: set[str] = set()
        for entry in entries:
            if not isinstance(entry, dict) or bool(entry.get("serveronly", False)):
                continue
            destination = _ftb_manifest_destination(entry)
            url = _text(entry.get("url") or entry.get("download_url") or entry.get("downloadUrl"))
            if not destination or not url:
                raw_path = "/".join(
                    part
                    for part in (
                        _text(entry.get("path")).replace("\\", "/").strip("/"),
                        _text(entry.get("name") or entry.get("file_name") or entry.get("filename")).replace("\\", "/").strip("/"),
                    )
                    if part
                )
                if _loader_managed_path(raw_path):
                    continue
                raise LauncherError("Манифест FTB содержит неполный файл")
            if destination in destinations:
                raise LauncherError(f"Манифест FTB содержит повторный путь: {destination}")
            destinations.add(destination)
            _validate_archive_url(url)
            raw_hashes = entry.get("hashes") if isinstance(entry.get("hashes"), dict) else {}
            publisher_hashes = _publisher_hashes({
                key: raw_hashes.get(key) or entry.get(key)
                for key in ("sha1", "sha256", "sha512")
            })
            trusted = any(publisher_hashes.values())
            if not trusted and not confirm_unverified:
                raise _unverified_download_error("Файл манифеста FTB не имеет доверенного хеша")
            size = _optional_int(entry.get("size") or entry.get("fileSize"))
            if size is not None and size > MAX_MODPACK_UNCOMPRESSED_BYTES:
                raise LauncherError("Распакованный модпак слишком большой")
            plan.append({
                "destination": destination,
                "url": url,
                "hashes": publisher_hashes,
                "size": size,
                "trusted": trusted,
                "preview_name": _text(entry.get("name") or entry.get("file_name") or entry.get("filename")) or Path(destination).name,
            })
        return plan

    # --- CurseForge: manifest.json ссылается на моды как projectID/fileID ---
    def _curseforge_download_plan(
        self,
        manifest: dict[str, Any],
        *,
        confirm_unverified: bool,
        resolve: bool = True,
    ) -> list[dict[str, Any]]:
        """Build the download plan for a CurseForge manifest.

        Files are resolved through the public cfwidget API; downloads are
        accepted only from the official forgecdn.net CDN. CurseForge
        manifests carry no publisher hashes, so every file is treated as an
        unverified download that the user must confirm.
        """
        files = manifest.get("files")
        if not isinstance(files, list):
            return []
        # CurseForge-манифесты никогда не содержат хешей: если вообще есть
        # файлы для загрузки, согласие «загружать без хеша» нужно до любых
        # сетевых обращений к каталогу.
        required_files = [
            entry
            for entry in files
            if isinstance(entry, dict) and not bool(entry.get("serveronly", False))
        ]
        if required_files and not confirm_unverified:
            raise _unverified_download_error("Файл модпака CurseForge не имеет издательского хеша")
        plan: list[dict[str, Any]] = []
        destinations: set[str] = set()
        for entry in files:
            if not isinstance(entry, dict) or bool(entry.get("serveronly", False)):
                continue
            project_id = _optional_int(entry.get("projectID"))
            file_id = _optional_int(entry.get("fileID"))
            if not project_id or not file_id:
                raise LauncherError("manifest.json CurseForge содержит неполный файл")
            if len(plan) >= CURSEFORGE_MAX_REMOTE_FILES:
                raise LauncherError("manifest.json CurseForge содержит слишком много файлов")
            resolved = self._resolve_curseforge_file(project_id, file_id) if resolve else None
            if resolve and resolved is None:
                # Необязательный файл можно пропустить; обязательный — нет:
                # молча терять моды и «притворяться, что импортировали»,
                # ровно то, что мы здесь чиним.
                if bool(entry.get("required") is False):
                    continue
                raise LauncherError(
                    f"Не удалось определить файл CurseForge {project_id}/{file_id}: "
                    "каталог ещё обрабатывает проект. Повторите импорт через минуту."
                )
            if resolved is None:
                # идентификаторы без сети — только для сводки превью
                name = f"CurseForge project {project_id}"
                optional = bool(entry.get("required") is False)
                plan.append({"destination": "", "url": "", "hashes": {}, "size": None, "trusted": False, "preview_name": name, "optional": optional})
                continue
            url, display_name, size = resolved
            _validate_curseforge_download_url(url)
            file_name = url.rsplit("/", 1)[-1] or display_name
            if not file_name.lower().endswith(".jar"):
                file_name = f"{display_name or file_id}.jar".strip()
            destination = _archive_destination(f"mods/{file_name}")
            if not destination:
                raise LauncherError("manifest.json CurseForge содержит небезопасный путь файла")
            if destination in destinations:
                raise LauncherError(f"manifest.json CurseForge содержит повторный путь: {destination}")
            destinations.add(destination)
            plan.append({
                "destination": destination,
                "url": url,
                "hashes": {},
                "size": size,
                "trusted": False,
                "preview_name": display_name or file_name,
                "optional": bool(entry.get("required") is False),
            })
        return plan

    def _resolve_curseforge_file(self, project_id: int, file_id: int) -> tuple[str, str, int | None] | None:
        """Resolve one projectID/fileID pair to a CDN download URL."""
        cache = getattr(self, "_cfwidget_cache", None)
        if cache is None:
            cache = {}
            self._cfwidget_cache = cache
        key = f"{project_id}/{file_id}"
        if key in cache:
            return cache[key]
        try:
            payload = self.http.get_json(f"{CURSEFORGE_RESOLVER_BASE}/{project_id}/files/{file_id}")
        except LauncherError:
            # cfwidget отвечает 202/ошибкой для проектов, которые ещё не в
            # его очереди: не вешаем импорт, отмечаем как неразрешённый
            cache[key] = None
            return None
        if not isinstance(payload, dict):
            cache[key] = None
            return None
        if isinstance(payload.get("queue"), (str, int)) or payload.get("queued"):
            cache[key] = None
            return None
        url = _text(payload.get("download_url") or payload.get("url"))
        display = _text(payload.get("display") or payload.get("name"))
        size = _optional_int(payload.get("size") or payload.get("filesize"))
        if not url:
            cache[key] = None
            return None
        value = (url, display, size)
        cache[key] = value
        return value

    def _stage_archive(
        self,
        archive_path: Path,
        stage: Path,
        *,
        confirm_unverified: bool,
        expected_game_version: str,
        expected_loader: str,
        expected_loader_version: str | None,
    ) -> dict[str, Any]:
        try:
            with zipfile.ZipFile(archive_path) as archive:
                index, manifest, selected, skipped_loader = self._archive_plan(archive)
                downloads = self._modrinth_download_plan(index, confirm_unverified)
                curseforge = self._curseforge_download_plan(manifest, confirm_unverified=confirm_unverified) if manifest else []
                curseforge_downloads = [item for item in curseforge if item["url"]]
                if manifest:
                    _validate_curseforge_compatibility(
                        manifest,
                        expected_game_version,
                        expected_loader,
                        expected_loader_version,
                    )
                else:
                    _validate_mrpack_compatibility(
                        index,
                        expected_game_version,
                        expected_loader,
                        expected_loader_version,
                    )
                archive_destinations = {destination for _info, destination in selected}
                remote = [item for item in downloads] + curseforge_downloads
                overlap = archive_destinations.intersection(item["destination"] for item in remote)
                if overlap:
                    raise LauncherError(f"Архив и манифест модпака повторяют путь: {sorted(overlap)[0]}")
                staged_bytes = 0
                for info, destination in selected:
                    remaining = MAX_MODPACK_UNCOMPRESSED_BYTES - staged_bytes
                    staged_bytes += _extract_zip_file(archive, info, stage / destination, remaining_bytes=remaining)
                trusted_modrinth = self._stage_modrinth_files(downloads, stage, staged_bytes)
                staged_bytes += sum(int(item["size"]) for item in downloads if item.get("size"))
                trusted_curseforge = True
                if curseforge_downloads:
                    trusted_curseforge = self._stage_modrinth_files(curseforge_downloads, stage, staged_bytes)
                return {
                    "trusted_hash": bool(trusted_modrinth and trusted_curseforge),
                    "skipped_loader_files": skipped_loader,
                }
        except zipfile.BadZipFile as exc:
            raise LauncherError("Файл не является корректным ZIP-модпаком") from exc

    def _stage_modrinth_files(self, entries: list[dict[str, Any]], stage: Path, staged_bytes: int) -> bool:
        for entry in entries:
            remaining = MAX_MODPACK_UNCOMPRESSED_BYTES - staged_bytes
            staged_bytes += self._download_staged_file(entry, stage, remaining)
        return all(bool(entry["trusted"]) for entry in entries)

    def _stage_manifest(self, entries: list[dict[str, Any]], stage: Path) -> dict[str, Any]:
        staged_bytes = 0
        for entry in entries:
            remaining = MAX_MODPACK_UNCOMPRESSED_BYTES - staged_bytes
            staged_bytes += self._download_staged_file(entry, stage, remaining)
        return {"trusted_hash": all(bool(entry["trusted"]) for entry in entries)}

    def _download_staged_file(self, entry: dict[str, Any], stage: Path, remaining: int) -> int:
        if remaining < 0:
            raise LauncherError("Распакованный модпак слишком большой")
        size = entry["size"]
        if size is not None and size > remaining:
            raise LauncherError("Распакованный модпак слишком большой")
        path = stage / entry["destination"]
        if path.exists() or path.is_symlink():
            raise LauncherError(f"Путь модпака уже существует: {entry['destination']}")

        def enforce_limit(current: int, total: int | None) -> None:
            if current > remaining or (total is not None and total > remaining):
                raise LauncherError("Распакованный модпак слишком большой")

        self.http.download(
            entry["url"],
            path,
            **entry["hashes"],
            size=size,
            progress=enforce_limit,
        )
        if path.is_symlink() or not path.is_file():
            raise LauncherError(f"Не удалось подготовить файл модпака: {entry['destination']}")
        actual_size = path.stat().st_size
        if actual_size > remaining:
            path.unlink()
            raise LauncherError("Распакованный модпак слишком большой")
        return actual_size

    @staticmethod
    def _merge(
        stage: Path,
        target: Path,
        *,
        finalize: Callable[[list[str]], Any] | None = None,
    ) -> list[str]:
        written: list[str] = []
        changed: dict[Path, Path | None] = {}
        created_directories: list[Path] = []
        keep_backup = False
        backup_root = Path(tempfile.mkdtemp(prefix=".trial-launcher-modpack-backup-", dir=target))

        def ensure_directory(directory: Path) -> None:
            _reject_symlink_parents(target, directory)
            if directory.is_symlink():
                raise LauncherError(f"Путь модпака является символьной ссылкой: {directory}")
            if directory.exists():
                if not directory.is_dir():
                    raise LauncherError(f"Путь модпака не является папкой: {directory}")
                return
            missing: list[Path] = []
            current = directory
            while current != target and not current.exists():
                if current.is_symlink():
                    raise LauncherError(f"Путь модпака является символьной ссылкой: {current}")
                missing.append(current)
                current = current.parent
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            created_directories.extend(reversed(missing))

        def rollback() -> list[OSError]:
            errors: list[OSError] = []
            for destination, backup in reversed(list(changed.items())):
                try:
                    _reject_symlink_parents(target, destination.parent)
                    if backup is not None:
                        os.replace(backup, destination)
                    elif destination.is_symlink() or destination.is_file():
                        destination.unlink(missing_ok=True)
                    elif destination.exists():
                        raise OSError(f"Неожиданная папка вместо файла: {destination}")
                except OSError as exc:
                    errors.append(exc)
            for directory in reversed(created_directories):
                try:
                    directory.rmdir()
                except OSError:
                    pass
            return errors

        try:
            for item in sorted(stage.rglob("*"), key=lambda path: (path.is_file(), str(path))):
                relative = item.relative_to(stage)
                destination = target / relative
                if item.is_dir():
                    ensure_directory(destination)
                    continue
                if not item.is_file() or item.is_symlink():
                    raise LauncherError(f"Некорректный файл модпака: {relative}")
                _reject_symlink_parents(target, destination.parent)
                if destination.is_symlink():
                    raise LauncherError(f"Файл модпака является символьной ссылкой: {relative}")
                ensure_directory(destination.parent)
                if destination.exists() and not destination.is_file():
                    raise LauncherError(f"Путь модпака не является файлом: {relative}")
                backup: Path | None = None
                if destination.exists():
                    backup = backup_root / relative
                    backup.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                    shutil.copy2(destination, backup)
                changed[destination] = backup
                shutil.copyfile(item, destination)
                os.chmod(destination, stat.S_IRUSR | stat.S_IWUSR)
                written.append(str(relative))
            if finalize:
                finalize(written)
            return written
        except Exception as exc:
            errors = rollback()
            if errors:
                keep_backup = True
                raise LauncherError("Импорт модпака завершился ошибкой; резервная копия оставлена для восстановления") from exc
            raise
        finally:
            if not keep_backup:
                shutil.rmtree(backup_root, ignore_errors=True)


def _rows(value: Any, *keys: str) -> list[dict[str, Any]]:
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict)]
    if not isinstance(value, dict):
        return []
    for key in keys:
        rows = value.get(key)
        if isinstance(rows, list):
            return [item for item in rows if isinstance(item, dict)]
    return []


def _ftb_result(item: dict[str, Any]) -> ModpackResult:
    archive = _find_archive(item)
    return ModpackResult(
        "ftb",
        _text(item.get("id") or item.get("project_id") or item.get("slug")),
        _text(item.get("name") or item.get("title") or item.get("displayName") or "FTB Modpack"),
        _text(item.get("synopsis") or item.get("description") or item.get("summary")),
        _text(item.get("author") or item.get("authors")),
        _text(item.get("icon") or item.get("logo") or item.get("logoUrl") or item.get("icon_url")),
        _int(item.get("downloads") or item.get("downloadCount")),
        _text(item.get("version") or item.get("version_id") or item.get("latestVersion")),
        _text(archive.get("url") or archive.get("download_url")),
        _safe_archive_name(_text(archive.get("file_name") or archive.get("filename")) or "ftb-modpack.zip"),
        _game_versions(item),
    )


def _technic_result(item: dict[str, Any]) -> ModpackResult:
    archive = _find_archive(item)
    return ModpackResult(
        "technic",
        _text(item.get("slug") or item.get("id") or item.get("name")),
        _text(item.get("displayName") or item.get("display_name") or item.get("name") or "Technic Modpack"),
        _text(item.get("description") or item.get("synopsis")),
        _text(item.get("author") or item.get("username")),
        _text(item.get("icon") or item.get("logo") or item.get("icon_url")),
        _int(item.get("downloads") or item.get("downloadCount")),
        _text(item.get("version") or item.get("build") or item.get("recommendedBuild")),
        _text(archive.get("url") or archive.get("download_url")),
        _safe_archive_name(_text(archive.get("file_name") or archive.get("filename")) or "technic-modpack.zip"),
        _game_versions(item),
    )


def _ftb_manifest_files(value: Any) -> list[dict[str, Any]]:
    rows = value.get("files") if isinstance(value, dict) else None
    if not isinstance(rows, list):
        return []
    return [dict(item) for item in rows if isinstance(item, dict)]


def _ftb_manifest_destination(entry: dict[str, Any]) -> str:
    path = _text(entry.get("path")).replace("\\", "/").strip()
    name = _text(entry.get("name") or entry.get("file_name") or entry.get("filename")).replace("\\", "/").strip()
    if name:
        path = f"{path.rstrip('/')}/{name}" if path else name
    while path.startswith("./"):
        path = path[2:]
    if not name and path.endswith("/"):
        return ""
    return _archive_destination(path)


def _find_archive(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        direct_keys = ("download_url", "downloadUrl", "download", "archive_url", "archiveUrl", "url")
        for key in direct_keys:
            candidate = value.get(key)
            if isinstance(candidate, str) and candidate.lower().endswith((".zip", ".mrpack")):
                return {"url": candidate, **{name: value.get(name) for name in ("sha1", "sha256", "sha512", "size")}}
        for key in ("files", "downloads", "artifacts", "builds", "versions"):
            rows = value.get(key)
            if isinstance(rows, list):
                for row in rows:
                    found = _find_archive(row)
                    if found:
                        return found
            elif isinstance(rows, dict):
                found = _find_archive(rows)
                if found:
                    return found
    return {}


def _pick_version(rows: list[dict[str, Any]], game_version: str, version: str) -> dict[str, Any]:
    if version:
        exact = next((row for row in rows if _text(row.get("id") or row.get("version_id") or row.get("version")) == version), None)
        if exact:
            return exact
    if game_version:
        compatible = [row for row in rows if game_version in _game_versions(row)]
        if compatible:
            return compatible[0]
    return rows[0] if rows else {}


def _game_versions(value: dict[str, Any]) -> list[str]:
    candidates = value.get("game_versions") or value.get("gameVersions") or value.get("minecraftVersions") or value.get("minecraft_versions") or value.get("minecraft")
    if isinstance(candidates, str):
        return [candidates]
    if isinstance(candidates, list):
        return [item for item in (_text(value) for value in candidates) if item]
    return []


def _archive_name(value: str) -> str:
    normalized = value.replace("\\", "/").rstrip("/")
    if "\x00" in normalized or normalized.startswith("/") or any(part in {"", ".", ".."} for part in normalized.split("/")):
        raise LauncherError("Модпак содержит небезопасный путь")
    return normalized


def _archive_prefix(names: Iterable[str]) -> str:
    values = list(names)
    if not values:
        return ""
    first_parts = {value.split("/", 1)[0] for value in values if value}
    prefix = next(iter(first_parts)) if len(first_parts) == 1 else ""
    if prefix and any(value.removeprefix(f"{prefix}/") in {"modrinth.index.json", "manifest.json"} for value in values):
        return f"{prefix}/"
    return ""


# Каталогами версии, библиотек и native-компонентов управляет лаунчер:
# файлы модпака поверх них ломают установленный модлоадер.
_LOADER_MANAGED_ROOTS = {"versions", "libraries", "natives", "bin"}
# Экспорты MultiMC/Prism кладут игру в .minecraft — как и overrides,
# это префикс-обёртка, а не настоящий каталог экземпляра.
_MODPACK_ROOT_PREFIXES = {"overrides", "client-overrides", ".minecraft", "minecraft"}
_MODPACK_METADATA_FILES = {
    "manifest.json",
    "modrinth.index.json",
    "modlist.html",
    "minecraftinstance.json",
    ".technicpack",
    "mmc-pack.json",
    "instance.cfg",
}


def _strip_modpack_prefix(value: str) -> str:
    parts = _archive_name(value).split("/")
    while parts and parts[0].casefold() in _MODPACK_ROOT_PREFIXES:
        parts.pop(0)
    return "/".join(parts)


def _loader_managed_path(value: str) -> bool:
    parts = _strip_modpack_prefix(value).split("/")
    return bool(parts) and parts[0] in _LOADER_MANAGED_ROOTS


def _archive_destination(value: str) -> str:
    normalized = _archive_name(value)
    parts = normalized.split("/")
    if any(part.casefold() == "server-overrides" for part in parts):
        return ""
    while parts and parts[0].casefold() in _MODPACK_ROOT_PREFIXES:
        parts.pop(0)
    if not parts:
        return ""
    if any(part.startswith(".trial-launcher") for part in parts):
        raise LauncherError("Модпак не может заменять служебные файлы лаунчера")
    if parts[0] in _MODPACK_METADATA_FILES:
        return ""
    if parts[0] in _LOADER_MANAGED_ROOTS:
        # Каталоги versions/, libraries/, natives/ и bin/ создаёт лаунчер:
        # позволить модпаку перезаписывать их — прямой способ сломать
        # модлоадер экземпляра.
        return ""
    return "/".join(parts)


def _is_server_override(value: str) -> bool:
    parts = _archive_name(value).split("/")
    return any(part.casefold() == "server-overrides" for part in parts)


def _extract_zip_file(
    archive: zipfile.ZipFile,
    info: zipfile.ZipInfo,
    destination: Path,
    *,
    remaining_bytes: int = MAX_MODPACK_UNCOMPRESSED_BYTES,
) -> int:
    if destination.is_symlink():
        raise LauncherError(f"Файл модпака является символьной ссылкой: {destination.name}")
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    written = 0
    with archive.open(info) as source:
        fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "wb") as handle:
            while chunk := source.read(1024 * 1024):
                if len(chunk) > remaining_bytes - written:
                    raise LauncherError("Распакованный модпак слишком большой")
                handle.write(chunk)
                written += len(chunk)
    return written


def _reject_symlink_parents(root: Path, path: Path) -> None:
    current = path
    root = root.resolve()
    while True:
        if current.is_symlink():
            raise LauncherError(f"Путь модпака является символьной ссылкой: {path}")
        if current == root:
            return
        if current == current.parent:
            raise LauncherError(f"Путь модпака выходит за пределы экземпляра: {path}")
        current = current.parent


def _is_symlink(info: zipfile.ZipInfo) -> bool:
    mode = (info.external_attr >> 16) & 0xFFFF
    return stat.S_ISLNK(mode)


def _record_modpack(
    target: Path,
    source: str,
    project_id: str,
    name: str,
    version: str,
    files: list[str],
    *,
    catalog_id: str = "",
    archive_sha256: str = "",
) -> Path:
    marker = target / MODPACK_MARKER
    if marker.is_symlink():
        raise LauncherError("Метаданные модпаков являются символьной ссылкой")
    records: list[dict[str, Any]] = []
    if marker.exists():
        try:
            if marker.stat().st_size > MAX_MODPACK_MARKER_BYTES:
                raise LauncherError("Метаданные модпаков слишком велики")
            current = json.loads(marker.read_text(encoding="utf-8"))
            if isinstance(current, list):
                records = [item for item in current if isinstance(item, dict)]
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            raise LauncherError("Метаданные модпаков повреждены")
    normalized_catalog_id = catalog_id.strip().lower() or source.strip().lower()
    records = [
        item
        for item in records
        if not (
            (archive_sha256 and str(item.get("archive_sha256", "")) == archive_sha256)
            or (
                project_id
                and str(item.get("project_id", "")) == project_id
                and str(item.get("source", "")).strip().lower() == source.strip().lower()
                and str(item.get("catalog_id") or item.get("source") or "").strip().lower() == normalized_catalog_id
            )
        )
    ]
    records.append(
        {
            "source": source,
            "catalog_id": normalized_catalog_id,
            "project_id": project_id,
            "name": name[:160],
            "version": version[:80],
            "archive_sha256": archive_sha256,
            "file_count": min(len(files), MAX_MODPACK_ENTRIES),
        }
    )
    records = records[-200:]
    temporary = target / f".{MODPACK_MARKER}.{uuid.uuid4().hex}.part"
    try:
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(records[-200:], handle, ensure_ascii=False)
        temporary.replace(marker)
        return marker
    except Exception:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        raise


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _assert_modpack_not_installed(
    target: Path,
    *,
    source: str,
    project_id: str,
    catalog_id: str = "",
    archive_sha256: str = "",
) -> None:
    if target.is_symlink() or not target.is_dir():
        raise LauncherError("Папка экземпляра модпака недоступна")
    marker = target / MODPACK_MARKER
    if marker.is_symlink():
        raise LauncherError("Метаданные модпаков являются символьной ссылкой")
    if not marker.exists():
        return
    if not marker.is_file() or marker.stat().st_size > MAX_MODPACK_MARKER_BYTES:
        raise LauncherError("Метаданные модпаков повреждены или слишком велики")
    try:
        records = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as exc:
        raise LauncherError("Метаданные модпаков повреждены") from exc
    if not isinstance(records, list):
        raise LauncherError("Метаданные модпаков имеют неожиданный формат")
    expected_catalog = catalog_id.strip().lower() or source.strip().lower()
    for item in records:
        if not isinstance(item, dict):
            continue
        if archive_sha256 and str(item.get("archive_sha256", "")) == archive_sha256:
            raise LauncherError("Этот архив модпака уже импортирован в выбранный экземпляр")
        stored_source = str(item.get("source", "")).strip().lower()
        stored_catalog = str(item.get("catalog_id") or item.get("source") or "").strip().lower()
        if (
            project_id
            and str(item.get("project_id", "")) == project_id
            and stored_source == source.strip().lower()
            and stored_catalog == expected_catalog
        ):
            raise LauncherError("Этот модпак уже установлен в выбранный экземпляр")


def _validate_archive_url(value: str) -> None:
    try:
        parsed = urlsplit(value)
    except ValueError as exc:
        raise LauncherError("URL архива модпака некорректен") from exc
    if parsed.scheme.lower() != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise LauncherError("Архив модпака должен загружаться по HTTPS")


def _validate_curseforge_download_url(value: str) -> None:
    """CurseForge files are downloaded only from the official CDN host."""
    try:
        parsed = urlsplit(value)
    except ValueError as exc:
        raise LauncherError("URL файла CurseForge некорректен") from exc
    host = (parsed.hostname or "").lower()
    if parsed.scheme.lower() != "https" or parsed.username or parsed.password:
        raise LauncherError("Файл CurseForge должен загружаться по HTTPS")
    if not host.endswith("." + CURSEFORGE_DOWNLOAD_HOST_SUFFIX) and host != CURSEFORGE_DOWNLOAD_HOST_SUFFIX:
        raise LauncherError("Файл CurseForge можно загружать только с официального CDN (forgecdn.net)")


def _safe_archive_name(value: str) -> str:
    name = Path(str(value).replace("\\", "/")).name
    if not name or name in {".", ".."} or name != str(value).replace("\\", "/"):
        return "modpack.zip"
    return name[:160]


def _text(value: Any) -> str:
    if isinstance(value, str):
        return value.strip()
    if isinstance(value, dict):
        return _text(value.get("name") or value.get("displayName") or value.get("text"))
    if isinstance(value, list):
        return ", ".join(item for item in (_text(part) for part in value) if item)
    return str(value).strip() if value is not None else ""


def _publisher_hashes(value: dict[str, Any]) -> dict[str, str | None]:
    lengths = {"sha1": 40, "sha256": 64, "sha512": 128}
    hashes: dict[str, str | None] = {}
    for algorithm, length in lengths.items():
        digest = _text(value.get(algorithm)).lower()
        if len(digest) != length or any(character not in "0123456789abcdef" for character in digest):
            digest = ""
        hashes[algorithm] = digest or None
    return hashes


def _unverified_download_error(message: str) -> LauncherError:
    return LauncherError(f"[unverified-download] {message}. Подтвердите скачивание явно.")


def _validate_curseforge_compatibility(
    manifest: dict[str, Any],
    game_version: str,
    loader: str,
    loader_version: str | None,
    *,
    strict: bool = True,
) -> dict[str, Any]:
    """Требования manifest.json CurseForge: minecraft.version + modLoaders[].id."""
    minecraft = manifest.get("minecraft")
    if not isinstance(minecraft, dict):
        return {"minecraft": "", "loader": "", "loader_version": "", "compatible": True, "issues": []}
    required_game = _text(minecraft.get("version"))
    selected_game = _text(game_version)

    loader_labels = {
        "forge": "Forge",
        "fabric": "Fabric",
        "fabric-loader": "Fabric",
        "quilt": "Quilt",
        "neoforge": "NeoForge",
    }
    mod_loaders = minecraft.get("modLoaders")
    required_loader = ""
    required_loader_version = ""
    if isinstance(mod_loaders, list):
        primary = next(
            (item for item in mod_loaders if isinstance(item, dict) and bool(item.get("primary"))),
            None,
        ) or next((item for item in mod_loaders if isinstance(item, dict)), None)
        if primary:
            identifier = _text(primary.get("id"))
            family, _, version = identifier.partition("-")
            family = family.strip().lower()
            if family not in loader_labels:
                message = f"Модпак требует незнакомый загрузчик «{identifier}»"
                if strict:
                    raise LauncherError(message)
                return {"minecraft": required_game, "loader": "", "loader_version": "", "compatible": False, "issues": [message]}
            required_loader = "fabric" if family == "fabric-loader" else family
            required_loader_version = version.strip()

    mismatches: list[str] = []
    if required_game and selected_game and required_game != selected_game:
        mismatches.append(f"Модпак требует Minecraft {required_game}; выбран Minecraft {selected_game}")
    selected_loader = _text(loader).lower()
    if required_loader and selected_loader and selected_loader != required_loader:
        version_detail = f" версии {required_loader_version}" if required_loader_version else ""
        mismatches.append(f"Модпак требует загрузчик {required_loader}{version_detail}; выбран {selected_loader}")
    elif required_loader_version and loader_version and str(loader_version) != required_loader_version:
        mismatches.append(f"Модпак требует {required_loader} {required_loader_version}; выбран {loader_version}")
    return {
        "minecraft": required_game,
        "loader": required_loader,
        "loader_version": required_loader_version,
        "compatible": not mismatches,
        "issues": mismatches,
    }


def _validate_mrpack_compatibility(
    index: dict[str, Any],
    game_version: str,
    loader: str,
    loader_version: str | None,
    *,
    strict: bool = True,
) -> dict[str, Any]:
    dependencies = index.get("dependencies")
    if not isinstance(dependencies, dict):
        return {"minecraft": "", "loader": "", "loader_version": "", "compatible": True, "issues": []}

    required_game = _text(dependencies.get("minecraft"))
    selected_game = _text(game_version)
    mismatches: list[str] = []
    if required_game and selected_game and required_game != selected_game:
        mismatches.append(f"Модпак требует Minecraft {required_game}; выбран Minecraft {selected_game}")

    dependency_loaders = {
        "fabric-loader": ("fabric", "Fabric"),
        "forge": ("forge", "Forge"),
        "neoforge": ("neoforge", "NeoForge"),
        "quilt-loader": ("quilt", "Quilt"),
    }
    required = [
        (family, label, _text(dependencies.get(dependency)))
        for dependency, (family, label) in dependency_loaders.items()
        if dependency in dependencies and _text(dependencies.get(dependency))
    ]
    if len(required) > 1:
        message = "Манифест модпака требует несколько несовместимых загрузчиков"
        if strict:
            raise LauncherError(message)
        return {
            "minecraft": required_game,
            "loader": "",
            "loader_version": "",
            "compatible": False,
            "issues": [message],
        }

    required_loader, loader_label, required_loader_version = required[0] if required else ("", "", "")
    selected_loader = _text(loader).lower()
    if required_loader and selected_loader and selected_loader != required_loader:
        version_detail = f" версии {required_loader_version}" if required_loader_version else ""
        mismatches.append(f"Модпак требует загрузчик {required_loader}{version_detail}; выбран {selected_loader}")

    selected_loader_version = _text(loader_version)
    if required_loader_version and loader_version is not None:
        if not selected_loader_version:
            mismatches.append(f"Модпак требует {loader_label} версии {required_loader_version}; версия загрузчика не выбрана")
        elif selected_loader_version != required_loader_version:
            mismatches.append(
                f"Модпак требует {loader_label} версии {required_loader_version}; выбрана версия {selected_loader_version}"
            )
    if mismatches and strict:
        raise LauncherError("; ".join(mismatches))

    return {
        "minecraft": required_game,
        "loader": required_loader,
        "loader_version": required_loader_version,
        "compatible": not mismatches,
        "issues": mismatches,
    }


def _int(value: Any) -> int:
    try:
        return max(0, int(value or 0))
    except (TypeError, ValueError):
        return 0


def _optional_int(value: Any) -> int | None:
    try:
        return max(0, int(value)) if value is not None else None
    except (TypeError, ValueError):
        return None
