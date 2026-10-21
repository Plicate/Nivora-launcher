from __future__ import annotations

import concurrent.futures
import ctypes
import errno
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
import time
import uuid
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlsplit

from .config import ConfigStore
from .constants import MAX_INSTANCE_VERSION_JSON_BYTES
from .errors import LauncherError
from .loaders import LoaderRepository
from .metadata import VersionRepository, maven_path, merge_version_json, safe_instance_name
from .net import HttpClient, verify_file
from .platform_support import is_reparse_point, mojang_os_name, process_start_options, set_private_fd
from .portable_fs import regular_file, safe_path
from .runtime import JavaRuntimeManager, safe_process_environment
from .security import prepare_process

_DIRECTORY_OPEN_FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
_NOFOLLOW = getattr(os, "O_NOFOLLOW", 0)

@dataclass
class InstallResult:
    version_id: str
    instance_dir: Path
    version_json: dict[str, Any]
    client_jar: Path
    libraries: list[Path]
    natives_dir: Path
    assets_root: Path
    asset_index_name: str
    logging_file: Path | None

class MinecraftInstaller:
    def __init__(
        self,
        config: ConfigStore,
        repository: VersionRepository | None = None,
        loaders: LoaderRepository | None = None,
        http: HttpClient | None = None,
    ) -> None:
        self.config = config
        self.http = http or HttpClient()
        self.repository = repository or VersionRepository(config, self.http)
        self.loaders = loaders or LoaderRepository(self.http)
        self.runtime = JavaRuntimeManager(config, self.http)

    def install(
        self,
        minecraft_version: str,
        *,
        loader: str = "vanilla",
        loader_version: str | None = None,
        instance_name: str | None = None,
        assets: bool = True,
        replace_existing: bool = False,
    ) -> InstallResult:
        base = self.repository.resolve(minecraft_version)
        if loader in {"forge", "neoforge"}:
            target = self._instance_dir(instance_name or f"{loader}-{minecraft_version}")
            build = lambda staging: self._install_external_loader(
                minecraft_version,
                loader,
                loader_version,
                assets,
                staging,
                base,
            )
            if replace_existing:
                return self._install_staged(target, build)
            return self._install_external_loader(
                minecraft_version,
                loader,
                loader_version,
                assets,
                target,
                base,
            )
        if loader in {"fabric", "quilt"}:
            profile = self.loaders.profile(loader, minecraft_version, loader_version)
            resolved = merge_version_json(base, profile)
            resolved["id"] = str(profile["id"])
            resolved["inheritsFrom"] = minecraft_version
            launch_id = str(profile["id"])
        elif loader == "vanilla":
            resolved = base
            launch_id = minecraft_version
        else:
            raise LauncherError(f"Неизвестный loader: {loader}")
        instance = self._instance_dir(instance_name or launch_id)
        build = lambda staging: self._materialize(launch_id, staging, resolved, assets=assets)
        if replace_existing:
            return self._install_staged(instance, build)
        return self._materialize(launch_id, instance, resolved, assets=assets)

    def repair_version(
        self,
        instance_dir: Path,
        minecraft_version: str,
        *,
        loader: str = "vanilla",
        loader_version: str | None = None,
        version_id: str | None = None,
    ) -> InstallResult:
        """пересоздать версию/модлоадер экземпляра, не трогая mods и config."""
        instance_dir = instance_dir.expanduser().resolve()
        instances_root = (self.config.game_dir / "instances").resolve()
        if instance_dir == instances_root or not _within(instance_dir, instances_root) or not instance_dir.is_dir():
            raise LauncherError(f"Недопустимый путь экземпляра: {instance_dir}")
        if self._instance_dir(instance_dir.name).resolve() != instance_dir:
            raise LauncherError(f"Имя экземпляра не соответствует папке: {instance_dir.name}")
        minecraft_version = str(minecraft_version).strip()
        if not minecraft_version:
            raise LauncherError("Не указана версия Minecraft")
        target_version = str(version_id or minecraft_version).strip()
        _safe_version_id(target_version)
        versions_root = instance_dir / "versions"
        if versions_root.is_symlink():
            raise LauncherError("Каталог версий является символьной ссылкой")
        versions_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        version_dir = versions_root / target_version
        backup = versions_root / f".trial-repair-{uuid.uuid4().hex}.part"
        moved = False
        if os.path.lexists(version_dir):
            if version_dir.is_symlink() or not version_dir.is_dir():
                raise LauncherError(f"Каталог версии не является папкой: {version_dir}")
            if _contains_symlink(version_dir, instance_dir):
                raise LauncherError("Каталог версии содержит символьные ссылки")
            os.rename(version_dir, backup)
            moved = True
        try:
            return self.install(
                minecraft_version,
                loader=loader,
                loader_version=loader_version,
                instance_name=instance_dir.name,
                assets=False,
                replace_existing=False,
            )
        except Exception:
            if moved:
                if os.path.lexists(version_dir):
                    _remove_instance_path(version_dir)
                os.rename(backup, version_dir)
            raise
        finally:
            if moved and os.path.lexists(backup):
                _remove_instance_path(backup)

    def load_existing(self, instance_dir: Path, version_id: str | None = None) -> InstallResult:
        """Build a launchable result from files already present in an instance."""
        instance_dir = instance_dir.expanduser().resolve()
        instances_root = (self.config.game_dir / "instances").resolve()
        if instance_dir == instances_root or not _within(instance_dir, instances_root) or not instance_dir.is_dir():
            raise LauncherError(f"Недопустимый путь экземпляра: {instance_dir}")
        versions = instance_dir / "versions"
        if versions.is_symlink():
            raise LauncherError("Каталог версий является символьной ссылкой")
        if version_id:
            _safe_version_id(str(version_id))
            metadata_path = versions / version_id / f"{version_id}.json"
        else:
            candidates = [
                path
                for path in versions.glob("*/*.json")
                if not path.is_symlink()
                and not _contains_symlink(path.parent, instance_dir)
                and _within(path.resolve(), instance_dir.resolve())
            ] if versions.exists() else []
            if not candidates:
                raise LauncherError(f"В экземпляре нет установленной версии: {instance_dir}")
            metadata_path = max(candidates, key=lambda path: path.stat().st_mtime)
            version_id = metadata_path.stem
        try:
            if metadata_path.is_symlink() or _contains_symlink(metadata_path.parent, instance_dir) or not _within(metadata_path.resolve(), instance_dir.resolve()):
                raise LauncherError("Metadata версии является символьной ссылкой")
            if not metadata_path.is_file() or metadata_path.stat().st_size > MAX_INSTANCE_VERSION_JSON_BYTES:
                raise LauncherError("Metadata версии слишком велика или недоступна")
            version_json = json.loads(metadata_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Не удалось прочитать metadata версии: {metadata_path}") from exc
        if not isinstance(version_json, dict):
            raise LauncherError(f"Некорректный metadata JSON: {metadata_path}")
        if not version_json.get("mainClass"):
            parent_id = str(version_json.get("inheritsFrom") or "").strip()
            if parent_id and parent_id != version_id:
                # профили forge/neoforge наследуют ванильный JSON: без
                # родителя нет mainClass и половины classpath.
                try:
                    version_json = merge_version_json(self.repository.resolve(parent_id), version_json)
                except LauncherError:
                    pass
        version_id = str(version_id)
        _safe_version_id(version_id)
        client_jar = self._download_client(instance_dir, version_id, version_json, repair_only=True)
        natives_dir = metadata_path.parent / "natives"
        libraries = self._download_libraries(instance_dir, version_json, natives_dir, repair_only=True)
        asset_index_name, assets_root = self._download_assets(
            instance_dir,
            version_json,
            download_objects=True,
            repair_only=True,
        )
        logging_file = self._download_logging(instance_dir, version_json, version_dir=metadata_path.parent, repair_only=True)
        return InstallResult(
            version_id=version_id,
            instance_dir=instance_dir,
            version_json=version_json,
            client_jar=client_jar,
            libraries=libraries,
            natives_dir=natives_dir,
            assets_root=assets_root,
            asset_index_name=asset_index_name,
            logging_file=logging_file,
        )

    def _materialize(
        self,
        version_id: str,
        instance: Path,
        version_json: dict[str, Any],
        *,
        assets: bool,
    ) -> InstallResult:
        _safe_version_id(version_id)
        instances_root = self.config.game_dir / "instances"
        _ensure_safe_directory(instance, instances_root, "экземпляр")
        instance.mkdir(parents=True, exist_ok=True)
        instance.chmod(0o700)
        version_dir = instance / "versions" / version_id
        _ensure_safe_directory(version_dir, instance, "каталог версии")
        version_dir.mkdir(parents=True, exist_ok=True)
        metadata_path = version_dir / f"{version_id}.json"
        if metadata_path.is_symlink():
            raise LauncherError(f"Metadata версии является символьной ссылкой: {metadata_path}")
        metadata_path.write_text(json.dumps(version_json, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

        client_jar = self._download_client(instance, version_id, version_json)
        libraries = self._download_libraries(instance, version_json, version_dir / "natives")
        asset_index_name, assets_root = self._download_assets(instance, version_json, download_objects=assets)
        logging_file = self._download_logging(instance, version_json, version_dir=version_dir)
        return InstallResult(
            version_id=version_id,
            instance_dir=instance,
            version_json=version_json,
            client_jar=client_jar,
            libraries=libraries,
            natives_dir=version_dir / "natives",
            assets_root=assets_root,
            asset_index_name=asset_index_name,
            logging_file=logging_file,
        )

    def _download_managed_file(
        self,
        instance: Path,
        state: dict[str, Any],
        url: str,
        destination: Path,
        info: dict[str, Any],
        *,
        repair_only: bool,
    ) -> tuple[bool, bool]:
        _assert_safe_cached_file(destination, instance)
        metadata = _integrity_metadata(info)
        if metadata and _integrity_cache_matches(instance, destination, state, metadata):
            return False, True
        if repair_only and destination.is_file() and not metadata:
            return False, True
        downloaded = self.http.download(url, destination, **(metadata or {}))
        if metadata:
            relative = destination.relative_to(instance).as_posix()
            state[relative] = {
                "metadata": metadata,
                "fingerprint": _file_fingerprint(destination),
            }
        return downloaded, False

    def _download_client(
        self,
        instance: Path,
        version_id: str,
        version_json: dict[str, Any],
        *,
        repair_only: bool = False,
    ) -> Path:
        _safe_version_id(version_id)
        version_dir = instance / "versions" / version_id
        destination = version_dir / f"{version_id}.jar"
        downloads = version_json.get("downloads", {})
        info = downloads.get("client", {}) if isinstance(downloads, dict) else {}
        if not isinstance(info, dict):
            info = {}
        if info.get("url"):
            _ensure_safe_directory(version_dir, instance, "каталог версии")
            version_dir.mkdir(parents=True, exist_ok=True)
            state = _read_integrity_state(instance, version_dir)
            previous = dict(state)
            self._download_managed_file(
                instance,
                state,
                str(info["url"]),
                destination,
                info,
                repair_only=repair_only,
            )
            if state != previous:
                _write_integrity_state(instance, version_dir, state)
            return destination

        parent_id = version_json.get("jar")
        if not parent_id or parent_id == version_id:
            parent_id = version_json.get("inheritsFrom")
        if parent_id and parent_id != version_id:
            parent = self.repository.resolve(str(parent_id))
            parent_jar = self._download_client(instance, str(parent_id), parent, repair_only=repair_only)
            parent_downloads = parent.get("downloads", {})
            parent_info = parent_downloads.get("client", {}) if isinstance(parent_downloads, dict) else {}
            if not isinstance(parent_info, dict):
                parent_info = {}
            parent_metadata = _integrity_metadata(parent_info)
            metadata = _integrity_metadata(info) or parent_metadata
            _assert_safe_cached_file(destination, instance)
            if destination.is_file() and metadata:
                state = _read_integrity_state(instance, version_dir)
                if _integrity_cache_matches(instance, destination, state, metadata) or verify_file(destination, **metadata):
                    relative = destination.relative_to(instance).as_posix()
                    state[relative] = {"metadata": metadata, "fingerprint": _file_fingerprint(destination)}
                    _write_integrity_state(instance, version_dir, state)
                    return destination
                destination.unlink()
            elif destination.is_file() and repair_only and not metadata:
                return destination
            _ensure_safe_directory(version_dir, instance, "каталог версии")
            version_dir.mkdir(parents=True, exist_ok=True)
            shutil.copy2(parent_jar, destination)
            if metadata:
                if metadata != parent_metadata and not verify_file(destination, **metadata):
                    destination.unlink()
                    raise LauncherError(f"Inherited client jar не соответствует metadata версии {version_id}")
                state = _read_integrity_state(instance, version_dir)
                relative = destination.relative_to(instance).as_posix()
                state[relative] = {"metadata": metadata, "fingerprint": _file_fingerprint(destination)}
                _write_integrity_state(instance, version_dir, state)
            return destination

        metadata = _integrity_metadata(info)
        _assert_safe_cached_file(destination, instance)
        if destination.is_file():
            if metadata:
                _ensure_safe_directory(version_dir, instance, "каталог версии")
                version_dir.mkdir(parents=True, exist_ok=True)
                state = _read_integrity_state(instance, version_dir)
                if _integrity_cache_matches(instance, destination, state, metadata) or verify_file(destination, **metadata):
                    relative = destination.relative_to(instance).as_posix()
                    state[relative] = {"metadata": metadata, "fingerprint": _file_fingerprint(destination)}
                    _write_integrity_state(instance, version_dir, state)
                    return destination
                raise LauncherError(f"Client jar повреждён, но для версии {version_id} нет URL загрузки")
            return destination
        raise LauncherError(f"У версии {version_id} нет client download")

    def _download_libraries(
        self,
        instance: Path,
        version_json: dict[str, Any],
        natives_dir: Path,
        *,
        repair_only: bool = False,
    ) -> list[Path]:
        _ensure_safe_directory(natives_dir, instance, "каталог native")
        had_natives = natives_dir.is_dir()
        natives_dir.mkdir(parents=True, exist_ok=True)
        version_dir = natives_dir.parent
        state = _read_integrity_state(instance, version_dir)
        previous = dict(state)
        version_json = _apply_library_overrides(version_json, _instance_library_overrides(instance))
        libraries: list[Path] = []
        needs_native_extraction = not repair_only or not had_natives
        for library in version_json.get("libraries", []):
            if not isinstance(library, dict) or not _rules_allow(library.get("rules")):
                continue
            name = str(library.get("name", ""))
            if not name:
                continue
            downloads = library.get("downloads", {})
            if not isinstance(downloads, dict):
                downloads = {}
            artifact = downloads.get("artifact", {})
            if not isinstance(artifact, dict):
                artifact = {}
            coordinate_parts = name.split(":")
            native_only = len(coordinate_parts) >= 4 and coordinate_parts[3].startswith("natives-")
            if native_only and artifact:
                native_destination = instance / "libraries" / _safe_relative_path(artifact.get("path") or maven_path(name), "native library")
                downloaded, cached = self._download_managed_file(
                    instance,
                    state,
                    self.repository.library_url(library),
                    native_destination,
                    artifact,
                    repair_only=repair_only,
                )
                if downloaded or not cached:
                    needs_native_extraction = True
                if needs_native_extraction:
                    extract_info = library.get("extract", {})
                    excluded = set(extract_info.get("exclude", [])) if isinstance(extract_info, dict) else set()
                    _safe_extract(native_destination, natives_dir, excluded)
                continue
            if artifact or "downloads" not in library:
                artifact_path = _safe_relative_path(artifact.get("path") or maven_path(name), "library")
                artifact_destination = instance / "libraries" / artifact_path
                self._download_managed_file(
                    instance,
                    state,
                    self.repository.library_url(library),
                    artifact_destination,
                    artifact,
                    repair_only=repair_only,
                )
                if not native_only:
                    libraries.append(artifact_destination)
            natives = library.get("natives", {})
            platform_key = mojang_os_name()
            native_classifier = natives.get(platform_key) if isinstance(natives, dict) else None
            if native_classifier:
                classifier = str(native_classifier).replace("${arch}", "64")
                classifiers = downloads.get("classifiers", {})
                classifier_info = classifiers.get(classifier, {}) if isinstance(classifiers, dict) else {}
                if not isinstance(classifier_info, dict):
                    classifier_info = {}
                native_path = _safe_relative_path(classifier_info.get("path") or maven_path(name, classifier), "native classifier")
                native_destination = instance / "libraries" / native_path
                downloaded, cached = self._download_managed_file(
                    instance,
                    state,
                    self.repository.library_url(library, classifier),
                    native_destination,
                    classifier_info,
                    repair_only=repair_only,
                )
                if downloaded or not cached:
                    needs_native_extraction = True
                if needs_native_extraction:
                    extract_info = library.get("extract", {})
                    excluded = set(extract_info.get("exclude", [])) if isinstance(extract_info, dict) else set()
                    _safe_extract(native_destination, natives_dir, excluded)
        if state != previous:
            _write_integrity_state(instance, version_dir, state)
        return libraries

    def _download_assets(
        self,
        instance: Path,
        version_json: dict[str, Any],
        *,
        download_objects: bool,
        repair_only: bool = False,
    ) -> tuple[str, Path]:
        assets_root = instance / "assets"
        _ensure_safe_directory(assets_root, instance, "каталог assets")
        assets_root.mkdir(parents=True, exist_ok=True)
        asset_index = version_json.get("assetIndex")
        if not isinstance(asset_index, dict) or not asset_index.get("url"):
            return str(version_json.get("assets", "")), assets_root
        index_name = _safe_component(str(asset_index.get("id", version_json.get("assets", "assets"))), "asset index")
        index_path = assets_root / "indexes" / f"{index_name}.json"
        marker = assets_root / ".trial-assets-complete.json"
        marker_data = _read_asset_marker(marker)
        index_metadata = _integrity_metadata(asset_index)
        index_unchanged = (
            marker_data.get("asset_index_metadata") == index_metadata
            and _asset_fingerprint_matches(
                instance,
                index_path,
                asset_index.get("size"),
                marker_data.get("asset_index_fingerprint"),
            )
        )
        _assert_safe_cached_file(index_path, instance)
        if not index_unchanged and not (repair_only and index_path.is_file() and not index_metadata):
            self.http.download(str(asset_index["url"]), index_path, **(index_metadata or {}))
        index_fingerprint = _file_fingerprint(index_path) if index_path.is_file() else None
        if not download_objects:
            return index_name, assets_root
        try:
            index = json.loads(index_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Некорректный asset index {index_name}") from exc
        if not isinstance(index, dict):
            raise LauncherError(f"Некорректный asset index {index_name}")
        objects = index.get("objects", {})
        if not isinstance(objects, dict):
            return index_name, assets_root
        assets_by_digest: dict[str, dict[str, Any]] = {}
        for item in objects.values():
            if isinstance(item, dict) and item.get("hash"):
                digest = str(item["hash"])
                if not re.fullmatch(r"[A-Fa-f0-9]{40}", digest):
                    raise LauncherError(f"Некорректный hash asset: {digest}")
                assets_by_digest[digest] = item
        fingerprints = _asset_marker_fingerprints(marker, assets_by_digest)
        if _assets_marker_valid(marker, asset_index, assets_root, assets_by_digest):
            materialized = _materialize_legacy_assets(
                instance,
                assets_root,
                index_name,
                index,
                marker_data.get("materialized", {}),
            )
            if (
                materialized != marker_data.get("materialized", {})
                or marker_data.get("asset_index_metadata") != index_metadata
                or marker_data.get("asset_index_fingerprint") != index_fingerprint
            ):
                _write_asset_marker(marker, asset_index, assets_by_digest, fingerprints, materialized, index_metadata, index_fingerprint)
            return index_name, assets_root
        jobs: list[tuple[str, Path, str, int | None]] = []
        for digest, item in assets_by_digest.items():
            path = assets_root / "objects" / digest[:2] / digest
            if _asset_fingerprint_matches(instance, path, item.get("size"), fingerprints.get(digest)):
                continue
            jobs.append(
                (
                    f"https://resources.download.minecraft.net/{digest[:2]}/{digest}",
                    path,
                    digest,
                    item.get("size"),
                )
            )
        workers = min(32, max(1, int(self.config.settings.max_downloads)))
        with concurrent.futures.ThreadPoolExecutor(max_workers=workers, thread_name_prefix="assets") as pool:
            pending: set[concurrent.futures.Future[bool]] = set()
            jobs_iter = iter(jobs)
            for _ in range(min(workers * 2, len(jobs))):
                url, path, digest, size = next(jobs_iter)
                pending.add(pool.submit(self.http.download, url, path, sha1=digest, size=size))
            while pending:
                done, pending = concurrent.futures.wait(
                    pending,
                    return_when=concurrent.futures.FIRST_COMPLETED,
                )
                for future in done:
                    future.result()
                    try:
                        url, path, digest, size = next(jobs_iter)
                    except StopIteration:
                        continue
                    pending.add(pool.submit(self.http.download, url, path, sha1=digest, size=size))
        materialized = _materialize_legacy_assets(
            instance,
            assets_root,
            index_name,
            index,
            marker_data.get("materialized", {}),
        )
        fingerprints = {
            digest: _file_fingerprint(assets_root / "objects" / digest[:2] / digest)
            for digest in assets_by_digest
        }
        _write_asset_marker(marker, asset_index, assets_by_digest, fingerprints, materialized, index_metadata, index_fingerprint)
        return index_name, assets_root

    def _download_logging(
        self,
        instance: Path,
        version_json: dict[str, Any],
        *,
        version_dir: Path,
        repair_only: bool = False,
    ) -> Path | None:
        logging_info = version_json.get("logging", {})
        info = logging_info.get("client", {}) if isinstance(logging_info, dict) else {}
        file_info = info.get("file") if isinstance(info, dict) else None
        if not isinstance(file_info, dict) or (not file_info.get("url") and not file_info.get("id")):
            return None
        file_id = _safe_component(str(file_info.get("id", "client.xml")), "logging config")
        destination = instance / "assets" / "log_configs" / file_id
        if not file_info.get("url"):
            _assert_safe_cached_file(destination, instance)
            return destination if destination.is_file() else None
        _ensure_safe_directory(version_dir, instance, "каталог версии")
        version_dir.mkdir(parents=True, exist_ok=True)
        state = _read_integrity_state(instance, version_dir)
        previous = dict(state)
        self._download_managed_file(
            instance,
            state,
            str(file_info["url"]),
            destination,
            file_info,
            repair_only=repair_only,
        )
        if state != previous:
            _write_integrity_state(instance, version_dir, state)
        return destination

    def _install_staged(
        self,
        target: Path,
        build: Callable[[Path], InstallResult],
    ) -> InstallResult:
        instances_root = self.config.game_dir / "instances"
        if instances_root.is_symlink():
            raise LauncherError("Каталог экземпляров является символьной ссылкой")
        instances_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        if instances_root.is_symlink() or not instances_root.is_dir():
            raise LauncherError("Каталог экземпляров недоступен")
        instances_root = instances_root.resolve()
        target = instances_root / target.name
        _ensure_safe_directory(target, instances_root, "экземпляр")
        if os.path.lexists(target) and (target.is_symlink() or not target.is_dir()):
            raise LauncherError(f"Путь экземпляра не является каталогом: {target}")
        staging = Path(tempfile.mkdtemp(prefix=".trial-stage-", dir=instances_root))
        staging.chmod(0o700)
        try:
            staged_result = build(staging)
            result = _rebase_install_result(staged_result, staging, target)
            old_instance = _swap_staged_directory(staging, target)
            if old_instance is not None and old_instance != staging:
                try:
                    _remove_instance_path(old_instance)
                except OSError:
                    pass
            return result
        finally:
            if os.path.lexists(staging):
                _remove_instance_path(staging)

    def _install_external_loader(
        self,
        minecraft_version: str,
        loader: str,
        loader_version: str | None,
        assets: bool,
        instance_dir: Path,
        base_version_json: dict[str, Any],
    ) -> InstallResult:
        _safe_version_id(minecraft_version)
        base = self._materialize(minecraft_version, instance_dir, base_version_json, assets=False)
        existing = _find_external_version(base.instance_dir, loader, loader_version, minecraft_version)
        if existing:
            local_json_path, local_json = existing
            parent_id = local_json.get("inheritsFrom")
            resolved = merge_version_json(self.repository.resolve(str(parent_id)), local_json) if parent_id else local_json
            return self._materialize(local_json_path.stem, base.instance_dir, resolved, assets=assets)
        if not loader_version:
            choices = self.loaders.choices(loader, minecraft_version)
            if not choices:
                raise LauncherError(f"Не найден {loader} для Minecraft {minecraft_version}")
            loader_version = choices[-1].version
        _safe_component(str(loader_version), "loader version")
        installer_url = self.loaders.installer_url(loader, minecraft_version, loader_version)
        _validate_loader_installer_url(installer_url)
        digest_algorithm, installer_digest = self._loader_installer_digest(installer_url)
        installer_path = self.config.paths.download_cache / "installers" / f"{_safe_component(loader, 'loader')}-{_safe_component(loader_version, 'loader version')}.jar"
        self.http.download(
            installer_url,
            installer_path,
            **{digest_algorithm: installer_digest},
            redirect_validator=_validate_loader_installer_url,
        )
        java = self.runtime.java_for(base.version_json)
        local_installer = base.instance_dir / f".trial-loader-installer-{loader}-{loader_version}.jar"
        if local_installer.is_symlink():
            raise LauncherError(f"Локальный installer является символьной ссылкой: {local_installer}")
        shutil.copy2(installer_path, local_installer)
        local_installer.chmod(0o600)
        _ensure_launcher_profiles(base.instance_dir)
        command = loader_installer_command(java, local_installer, base.instance_dir)
        process: subprocess.Popen[str] | None = None
        try:
            prepared_command, prepared_cwd, prepared_environment, _ = prepare_process(
                command,
                base.instance_dir,
                safe_process_environment(),
                {
                    "security_profile": "strict",
                    "security_permissions": ["network:game", "files:instance", "desktop:display", "desktop:audio"],
                },
            )
            deadline = time.monotonic() + 10 * 60
            process = subprocess.Popen(
                prepared_command,
                cwd=prepared_cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                env=prepared_environment,
                **process_start_options(),
            )
            while True:
                try:
                    stdout, stderr = process.communicate(timeout=0.25)
                    break
                except subprocess.TimeoutExpired:
                    self.http.check_cancelled()
                    if time.monotonic() >= deadline:
                        raise LauncherError(f"{loader} installer превысил лимит времени")
        except Exception:
            if process is not None:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            raise
        finally:
            try:
                local_installer.unlink()
            except OSError:
                pass
        if process is None:
            raise LauncherError(f"{loader} installer не запустился")
        process_result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
        if process_result.returncode != 0:
            output = (process_result.stdout + "\n" + process_result.stderr).strip()[-3000:]
            raise LauncherError(f"{loader} installer завершился с кодом {process_result.returncode}:\n{output}")
        candidates = list((base.instance_dir / "versions").glob("*/*.json"))
        candidates = [
            path
            for path in candidates
            if not path.is_symlink()
            and not path.parent.is_symlink()
            and _within(path.resolve(), base.instance_dir.resolve())
            and (loader in path.parent.name.lower() or loader in path.name.lower())
        ]
        if not candidates:
            raise LauncherError(f"{loader} installer не создал version JSON в {base.instance_dir / 'versions'}")
        local_json_path = max(candidates, key=lambda path: path.stat().st_mtime)
        if local_json_path.is_symlink():
            raise LauncherError(f"Результат {loader} installer является символьной ссылкой")
        try:
            if local_json_path.stat().st_size > MAX_INSTANCE_VERSION_JSON_BYTES:
                raise LauncherError(f"Результат {loader} installer слишком велик")
            local_json = json.loads(local_json_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Не удалось прочитать результат {loader} installer") from exc
        if not isinstance(local_json, dict):
            raise LauncherError(f"Результат {loader} installer имеет неожиданный формат")
        parent_id = local_json.get("inheritsFrom")
        resolved = merge_version_json(self.repository.resolve(str(parent_id)), local_json) if parent_id else local_json
        return self._materialize(local_json_path.stem, base.instance_dir, resolved, assets=assets)

    def _instance_dir(self, name: str) -> Path:
        return self.config.game_dir / "instances" / safe_instance_name(name)

    def _loader_installer_digest(self, installer_url: str) -> tuple[str, str]:
        for algorithm, digest_length in (("sha512", 128), ("sha256", 64), ("sha1", 40)):
            try:
                checksum = self.http.request(
                    f"{installer_url}.{algorithm}",
                    redirect_validator=_validate_loader_installer_url,
                ).decode("ascii", errors="ignore")
            except (LauncherError, OSError):
                continue
            match = re.search(rf"\b([A-Fa-f0-9]{{{digest_length}}})\b", checksum)
            if match:
                return algorithm, match.group(1)
        raise LauncherError("Loader installer не имеет корректного SHA-512, SHA-256 или SHA-1 checksum")

def loader_installer_command(java: Path | str, installer: Path, target: Path) -> list[str]:
    """командная строка Forge/NeoForge installer для установки клиента."""
    return [str(java), "-jar", str(installer), "--installClient", str(target)]

def _ensure_launcher_profiles(target: Path) -> None:
    """forge/NeoForge installer отказывается ставить клиент без launcher_profiles.json"""
    path = target / "launcher_profiles.json"
    if path.is_symlink():
        raise LauncherError("launcher_profiles.json экземпляра является символьной ссылкой")
    if path.is_file():
        return
    path.write_text(json.dumps({"profiles": {}, "selectedProfile": "", "version": 3}) + "\n", encoding="utf-8")

def _rebase_install_result(result: InstallResult, source: Path, destination: Path) -> InstallResult:
    def rebase(path: Path) -> Path:
        try:
            return destination / path.relative_to(source)
        except ValueError as exc:
            raise LauncherError(f"Путь результата установки находится вне staging: {path}") from exc

    if result.instance_dir != source:
        raise LauncherError("Результат установки не соответствует staging-каталогу")
    return InstallResult(
        version_id=result.version_id,
        instance_dir=destination,
        version_json=result.version_json,
        client_jar=rebase(result.client_jar),
        libraries=[rebase(path) for path in result.libraries],
        natives_dir=rebase(result.natives_dir),
        assets_root=rebase(result.assets_root),
        asset_index_name=result.asset_index_name,
        logging_file=rebase(result.logging_file) if result.logging_file else None,
    )

def _swap_staged_directory(staging: Path, target: Path) -> Path | None:
    if not os.path.lexists(target):
        os.replace(staging, target)
        return None
    if target.is_symlink() or not target.is_dir():
        raise LauncherError(f"Путь экземпляра не является каталогом: {target}")
    if _rename_exchange(staging, target):
        return staging

    backup = target.with_name(f".trial-backup-{uuid.uuid4().hex}")
    os.replace(target, backup)
    try:
        os.replace(staging, target)
    except BaseException:
        try:
            os.replace(backup, target)
        except OSError as rollback_error:
            raise LauncherError(f"Не удалось восстановить предыдущий экземпляр; он сохранён в {backup}") from rollback_error
        raise
    return backup

def _rename_exchange(source: Path, destination: Path) -> bool:
    if os.name != "posix":
        return False
    try:
        renameat2 = ctypes.CDLL(None, use_errno=True).renameat2
    except AttributeError:
        return False
    renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameat2.restype = ctypes.c_int
    result = renameat2(-100, os.fsencode(source), -100, os.fsencode(destination), 2)
    if result == 0:
        return True
    error = ctypes.get_errno()
    unsupported = {
        errno.ENOSYS,
        errno.EINVAL,
        getattr(errno, "ENOTSUP", errno.EINVAL),
        getattr(errno, "EOPNOTSUPP", errno.EINVAL),
    }
    if error in unsupported:
        return False
    raise OSError(error, os.strerror(error), f"{source} <-> {destination}")

def _remove_instance_path(path: Path) -> None:
    if not os.path.lexists(path):
        return
    if path.is_symlink() or not path.is_dir():
        path.unlink()
    else:
        shutil.rmtree(path)

def _integrity_metadata(info: dict[str, Any]) -> dict[str, Any] | None:
    metadata: dict[str, Any] = {}
    for algorithm in ("sha1", "sha256", "sha512"):
        digest = info.get(algorithm)
        if digest:
            metadata[algorithm] = str(digest).lower()
    if info.get("size") is not None:
        try:
            size = int(info["size"])
        except (TypeError, ValueError) as exc:
            raise LauncherError("Размер файла в metadata некорректен") from exc
        if size < 0:
            raise LauncherError("Размер файла в metadata некорректен")
        metadata["size"] = size
    return metadata or None

def _file_fingerprint(path: Path) -> dict[str, int]:
    value = path.stat()
    return {
        "device": value.st_dev,
        "inode": value.st_ino,
        "size": value.st_size,
        "mtime_ns": value.st_mtime_ns,
        "ctime_ns": value.st_ctime_ns,
    }

def _assert_safe_cached_file(path: Path, root: Path) -> None:
    if path.is_symlink() or _contains_symlink(path.parent, root):
        raise LauncherError(f"Небезопасный путь файла экземпляра: {path}")
    if os.path.lexists(path) and not path.is_file():
        raise LauncherError(f"Путь файла экземпляра не является обычным файлом: {path}")

def _integrity_cache_matches(
    instance: Path,
    path: Path,
    files: dict[str, Any],
    metadata: dict[str, Any],
) -> bool:
    _assert_safe_cached_file(path, instance)
    if not path.is_file():
        return False
    try:
        relative = path.relative_to(instance).as_posix()
    except ValueError:
        return False
    entry = files.get(relative)
    if not isinstance(entry, dict) or entry.get("metadata") != metadata:
        return False
    try:
        return entry.get("fingerprint") == _file_fingerprint(path)
    except OSError:
        return False

def _read_integrity_state(instance: Path, version_dir: Path) -> dict[str, Any]:
    _ensure_safe_directory(version_dir, instance, "каталог версии")
    marker = version_dir / ".trial-integrity"
    if marker.is_symlink() or _contains_symlink(marker.parent, instance):
        raise LauncherError("Файл проверки целостности является ссылкой")
    if not marker.exists():
        return {}
    if not marker.is_file() or marker.stat().st_size > 4 * 1024 * 1024:
        return {}
    try:
        value = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return {}
    files = value.get("files") if isinstance(value, dict) else None
    return files if isinstance(files, dict) else {}

def _write_integrity_state(instance: Path, version_dir: Path, files: dict[str, Any]) -> None:
    _ensure_safe_directory(version_dir, instance, "каталог версии")
    version_dir.mkdir(parents=True, exist_ok=True)
    marker = version_dir / ".trial-integrity"
    if marker.is_symlink() or (os.path.lexists(marker) and not marker.is_file()):
        raise LauncherError("Файл проверки целостности недоступен")
    temporary = marker.with_name(f".trial-integrity-{uuid.uuid4().hex}.part")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump({"files": files}, handle, ensure_ascii=False, separators=(",", ":"))
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, marker)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass

def _rules_allow(rules: Any) -> bool:
    if not rules:
        return True
    allowed = False
    for rule in rules:
        if not isinstance(rule, dict) or not _rule_matches(rule):
            continue
        allowed = rule.get("action") == "allow"
    return allowed

def _safe_version_id(value: str) -> None:
    if not value or len(value) > 200 or value in {".", ".."} or not re.fullmatch(r"[A-Za-z0-9._+-]+", value):
        raise LauncherError(f"Небезопасный идентификатор версии: {value}")

def _validate_loader_installer_url(url: str) -> None:
    try:
        parsed = urlsplit(url)
        port = parsed.port
    except ValueError as exc:
        raise LauncherError("Loader installer имеет некорректный URL") from exc
    if (
        parsed.scheme != "https"
        or parsed.username
        or parsed.password
        or port not in {None, 443}
        or parsed.fragment
        or parsed.hostname not in {"maven.minecraftforge.net", "maven.neoforged.net"}
    ):
        raise LauncherError("Loader installer должен загружаться с официального Maven endpoint")

def _safe_component(value: str, label: str) -> str:
    if not value or not re.fullmatch(r"[A-Za-z0-9._+-]+", value):
        raise LauncherError(f"Небезопасный {label}: {value}")
    return value

def _safe_relative_path(value: str, label: str) -> Path:
    relative = Path(str(value).replace("\\", "/"))
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise LauncherError(f"Небезопасный путь {label}: {value}")
    return relative

def _rule_matches(rule: dict[str, Any]) -> bool:
    operating_system = rule.get("os", {})
    if isinstance(operating_system, dict):
        name = operating_system.get("name")
        current_os = mojang_os_name()
        if name and name != current_os:
            return False
        arch = operating_system.get("arch")
        if arch:
            machine = platform.machine().lower()
            if machine in {"x86_64", "amd64"}:
                aliases = {"x86_64", "amd64", "64"}
            elif machine in {"aarch64", "arm64"}:
                aliases = {"arm64", "aarch64"}
            else:
                aliases = {machine}
            if str(arch).lower() not in aliases:
                return False
    features = rule.get("features", {})
    # релиз поддерживает обычный настольный клиент, без feature-флагов.
    if isinstance(features, dict) and any(bool(value) for value in features.values()):
        return False
    return True

def _safe_extract(archive: Path, destination: Path, excluded: set[str]) -> None:
    if os.name == "nt":
        _safe_extract_windows(archive, destination, excluded)
        return
    if destination.is_symlink():
        raise LauncherError("Каталог native archive является символьной ссылкой")
    destination.mkdir(parents=True, exist_ok=True)
    total_size = 0
    root_fd = os.open(destination, _DIRECTORY_OPEN_FLAGS)
    try:
        with zipfile.ZipFile(archive) as handle:
            for item in handle.infolist():
                name = item.filename.replace("\\", "/")
                if name.endswith("/") or name.startswith("META-INF/") or any(name == value or name.startswith(value.rstrip("/") + "/") for value in excluded):
                    continue
                if item.file_size > 256 * 1024 * 1024 or (total_size + item.file_size) > 1024 * 1024 * 1024:
                    raise LauncherError("Native archive слишком большой")
                if (item.external_attr >> 16) & 0o170000 == 0o120000:
                    raise LauncherError(f"Небезопасная ссылка в native archive: {name}")
                relative = Path(name)
                if relative.is_absolute() or not relative.parts or any(part in {"", ".", ".."} for part in relative.parts):
                    raise LauncherError(f"Небезопасный путь в native archive: {name}")
                parent_fd = _open_directory_chain(root_fd, tuple(relative.parts[:-1]), create=True)
                try:
                    fd = os.open(relative.name, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | _NOFOLLOW, 0o600, dir_fd=parent_fd)
                    with handle.open(item) as source, os.fdopen(fd, "wb") as output:
                        written = 0
                        while True:
                            chunk = source.read(1024 * 1024)
                            if not chunk:
                                break
                            written += len(chunk)
                            if written > 256 * 1024 * 1024 or total_size + written > 1024 * 1024 * 1024:
                                raise LauncherError("Native archive слишком большой")
                            output.write(chunk)
                    total_size += written
                finally:
                    os.close(parent_fd)
    except zipfile.BadZipFile as exc:
        raise LauncherError(f"Native library не является ZIP/JAR: {archive}") from exc
    finally:
        os.close(root_fd)

def _safe_extract_windows(archive: Path, destination: Path, excluded: set[str]) -> None:
    if is_reparse_point(destination):
        raise LauncherError("Каталог native archive является ссылкой или junction")
    destination.mkdir(parents=True, exist_ok=True)
    total_size = 0
    try:
        with zipfile.ZipFile(archive) as handle:
            for item in handle.infolist():
                name = item.filename.replace("\\", "/")
                if name.endswith("/") or name.startswith("META-INF/") or any(
                    name == value or name.startswith(value.rstrip("/") + "/") for value in excluded
                ):
                    continue
                if (item.external_attr >> 16) & 0o170000 == 0o120000:
                    raise LauncherError(f"Небезопасная ссылка в native archive: {name}")
                parts = tuple(name.split("/"))
                if name.startswith("/") or not parts or any(not part or part in {".", ".."} for part in parts):
                    raise LauncherError(f"Небезопасный путь в native archive: {name}")
                if item.file_size > 256 * 1024 * 1024 or total_size + item.file_size > 1024 * 1024 * 1024:
                    raise LauncherError("Native archive слишком большой")
                parent = safe_path(destination, parts[:-1])
                parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                parent = safe_path(destination, parts[:-1])
                target = safe_path(destination, parts)
                if os.path.lexists(target):
                    regular_file(target)
                descriptor, temporary_name = tempfile.mkstemp(prefix=".native-", suffix=".part", dir=parent)
                temporary = Path(temporary_name)
                written = 0
                try:
                    set_private_fd(descriptor)
                    with handle.open(item) as source, os.fdopen(descriptor, "wb") as output:
                        while True:
                            chunk = source.read(1024 * 1024)
                            if not chunk:
                                break
                            written += len(chunk)
                            if written > 256 * 1024 * 1024 or total_size + written > 1024 * 1024 * 1024:
                                raise LauncherError("Native archive слишком большой")
                            output.write(chunk)
                        output.flush()
                        os.fsync(output.fileno())
                    safe_path(destination, parts)
                    temporary.replace(target)
                finally:
                    try:
                        temporary.unlink()
                    except FileNotFoundError:
                        pass
                total_size += written
    except zipfile.BadZipFile as exc:
        raise LauncherError(f"Native library не является ZIP/JAR: {archive}") from exc

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

def _contains_symlink(path: Path, root: Path) -> bool:
    current = path
    while True:
        if current.is_symlink():
            return True
        if current == root:
            return False
        if current == current.parent or not _within(current, root):
            return True
        current = current.parent

def _ensure_safe_directory(path: Path, root: Path, label: str) -> None:
    if root.is_symlink():
        raise LauncherError(f"Корень {label} является символьной ссылкой")
    current = path
    while True:
        if current.is_symlink():
            raise LauncherError(f"Небезопасный путь {label}: {path}")
        if current == root:
            return
        if current == current.parent or not _within(current, root):
            raise LauncherError(f"Небезопасный путь {label}: {path}")
        current = current.parent

def _materialize_legacy_assets(
    instance: Path,
    assets_root: Path,
    index_name: str,
    index: dict[str, Any],
    cached: Any = None,
) -> dict[str, Any]:
    if index.get("virtual"):
        destination_root = assets_root / "virtual" / index_name
    elif index.get("map_to_resources"):
        destination_root = instance / "resources"
    else:
        return {}
    _ensure_safe_directory(destination_root, instance, "legacy assets")
    destination_root.mkdir(parents=True, exist_ok=True)
    objects = index.get("objects", {})
    if not isinstance(objects, dict):
        return {}
    cached_files = cached if isinstance(cached, dict) else {}
    materialized: dict[str, Any] = {}
    for logical_name, item in objects.items():
        if not isinstance(item, dict) or not item.get("hash"):
            continue
        relative = Path(str(logical_name))
        if relative.is_absolute() or ".." in relative.parts:
            raise LauncherError(f"Небезопасный virtual asset path: {logical_name}")
        digest = str(item["hash"])
        source = assets_root / "objects" / digest[:2] / digest
        target = destination_root / relative
        if target.is_symlink() or _contains_symlink(target.parent, instance):
            raise LauncherError(f"Небезопасная ссылка в virtual asset path: {logical_name}")
        if not source.is_file() or source.is_symlink() or _contains_symlink(source.parent, assets_root):
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target_key = target.relative_to(instance).as_posix()
        old_entry = cached_files.get(target_key)
        old_fingerprint = old_entry.get("fingerprint") if isinstance(old_entry, dict) and old_entry.get("digest") == digest else None
        if not _asset_fingerprint_matches(instance, target, item.get("size"), old_fingerprint):
            temporary = target.with_name(f".{target.name}-{uuid.uuid4().hex}.part")
            fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _NOFOLLOW, 0o600)
            try:
                with source.open("rb") as source_handle, os.fdopen(fd, "wb") as target_handle:
                    shutil.copyfileobj(source_handle, target_handle)
                os.replace(temporary, target)
            finally:
                try:
                    temporary.unlink()
                except FileNotFoundError:
                    pass
        materialized[target_key] = {"digest": digest, "fingerprint": _file_fingerprint(target)}
    return materialized

MAVEN_CENTRAL_BASE = "https://repo1.maven.org/maven2"
_INSTANCE_SETTINGS_FILE_FOR_OVERRIDES = ".trial-launcher.json"

def _instance_library_overrides(instance: Path) -> dict[str, str]:
    """Оверрайды версий LWJGL из настроек экземпляра (вкладка «Версии»)."""
    path = instance / _INSTANCE_SETTINGS_FILE_FOR_OVERRIDES
    if path.is_symlink() or not path.is_file():
        return {}
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    value = raw.get("library_overrides") if isinstance(raw, dict) else None
    if not isinstance(value, dict):
        return {}
    from .instance_workspace import _normalize_library_overrides

    return _normalize_library_overrides(value)

def _apply_library_overrides(version_json: dict[str, Any], overrides: dict[str, str]) -> dict[str, Any]:
    """подменяет версии библиотек из настроек экземпляра."""
    if not overrides:
        return version_json
    libraries: list[Any] = []
    for library in version_json.get("libraries", []):
        if isinstance(library, dict):
            name = str(library.get("name", ""))
            key = ":".join(name.split(":")[:2])
            override = overrides.get(key)
            if override:
                library = _rewrite_library_version(library, override)
        libraries.append(library)
    return {**version_json, "libraries": libraries}

def _rewrite_library_version(library: dict[str, Any], version: str) -> dict[str, Any]:
    parts = str(library.get("name", "")).split(":")
    if len(parts) < 3:
        return library
    rewritten = dict(library)
    rewritten["name"] = ":".join([parts[0], parts[1], version, *parts[4:]])
    downloads = library.get("downloads")
    if isinstance(downloads, dict):
        downloads = json.loads(json.dumps(downloads))
        artifact = downloads.get("artifact")
        if isinstance(artifact, dict):
            downloads["artifact"] = _maven_central_artifact(parts[0], parts[1], version)
        classifiers = downloads.get("classifiers")
        if isinstance(classifiers, dict):
            for classifier, info in classifiers.items():
                if isinstance(info, dict):
                    classifiers[classifier] = _maven_central_artifact(parts[0], parts[1], version, classifier)
        rewritten["downloads"] = downloads
    return rewritten

def _maven_central_artifact(group: str, artifact: str, version: str, classifier: str | None = None) -> dict[str, Any]:
    suffix = f"-{classifier}" if classifier else ""
    path = f"{group.replace('.', '/')}/{artifact}/{version}/{artifact}-{version}{suffix}.jar"
    return {"path": path, "url": f"{MAVEN_CENTRAL_BASE}/{path}"}

def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False

def _assets_marker_valid(
    marker: Path,
    asset_index: dict[str, Any],
    assets_root: Path,
    objects: dict[str, Any],
) -> bool:
    value = _read_asset_marker(marker)
    if not value:
        return False
    if value.get("asset_index_sha1") != asset_index.get("sha1"):
        return False
    cached_objects = value.get("objects")
    if not isinstance(cached_objects, dict) or len(cached_objects) != len(objects):
        return False
    fingerprints = value.get("fingerprints")
    if not isinstance(fingerprints, dict):
        return False
    for digest, item in objects.items():
        if not isinstance(item, dict) or cached_objects.get(digest) != item.get("size"):
            return False
        path = assets_root / "objects" / digest[:2] / digest
        if not _asset_fingerprint_matches(assets_root, path, item.get("size"), fingerprints.get(digest)):
            return False
    return True

def _read_asset_marker(marker: Path) -> dict[str, Any]:
    if marker.is_symlink() or _contains_symlink(marker.parent, marker.parent.parent) or not marker.is_file():
        return {}
    if marker.stat().st_size > 16 * 1024 * 1024:
        return {}
    try:
        value = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return {}
    return value if isinstance(value, dict) else {}

def _asset_marker_fingerprints(marker: Path, objects: dict[str, dict[str, Any]]) -> dict[str, Any]:
    value = _read_asset_marker(marker)
    cached_objects = value.get("objects")
    fingerprints = value.get("fingerprints")
    if not isinstance(cached_objects, dict) or not isinstance(fingerprints, dict):
        return {}
    return {
        digest: fingerprints[digest]
        for digest, item in objects.items()
        if cached_objects.get(digest) == item.get("size") and isinstance(fingerprints.get(digest), dict)
    }

def _asset_fingerprint_matches(
    root: Path,
    path: Path,
    expected_size: Any,
    fingerprint: Any,
) -> bool:
    if not isinstance(fingerprint, dict):
        return False
    try:
        _assert_safe_cached_file(path, root)
        if not path.is_file():
            return False
        if expected_size is not None and path.stat().st_size != int(expected_size):
            return False
        return _file_fingerprint(path) == fingerprint
    except (OSError, TypeError, ValueError, LauncherError):
        return False

def _write_asset_marker(
    marker: Path,
    asset_index: dict[str, Any],
    objects: dict[str, dict[str, Any]],
    fingerprints: dict[str, Any],
    materialized: dict[str, Any],
    index_metadata: dict[str, Any] | None,
    index_fingerprint: dict[str, int] | None,
) -> None:
    if marker.is_symlink():
        raise LauncherError("Файл состояния assets является символьной ссылкой")
    marker.write_text(
        json.dumps(
            {
                "asset_index_sha1": asset_index.get("sha1"),
                "asset_index_metadata": index_metadata,
                "asset_index_fingerprint": index_fingerprint,
                "objects": {digest: item.get("size") for digest, item in objects.items()},
                "fingerprints": fingerprints,
                "materialized": materialized,
            },
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )

def _find_external_version(
    instance: Path,
    loader: str,
    loader_version: str | None,
    minecraft_version: str,
) -> tuple[Path, dict[str, Any]] | None:
    versions_dir = instance / "versions"
    if instance.is_symlink() or not instance.is_dir():
        return None
    instance_root = instance.resolve()
    if versions_dir.is_symlink() or not versions_dir.is_dir():
        return None
    candidates: list[tuple[Path, dict[str, Any]]] = []
    for path in versions_dir.glob("*/*.json"):
        if path.is_symlink() or path.parent.is_symlink():
            continue
        try:
            if not _within(path.resolve(), instance_root):
                continue
        except OSError:
            continue
        lowered = path.parent.name.lower()
        if loader == "forge":
            matches_loader = "forge" in lowered and "neoforge" not in lowered
        else:
            matches_loader = loader in lowered
        if not matches_loader:
            continue
        if loader_version and loader_version.lower() not in lowered:
            continue
        try:
            if path.stat().st_size > MAX_INSTANCE_VERSION_JSON_BYTES:
                continue
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError, TypeError):
            continue
        if not isinstance(value, dict):
            continue
        parent = value.get("inheritsFrom") or value.get("minecraft")
        if parent != minecraft_version:
            continue
        candidates.append((path, value))
    if not candidates:
        return None
    path, value = max(candidates, key=lambda item: item[0].stat().st_mtime)
    return path, value
