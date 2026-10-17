"""Резервные копии миров по расписанию: локально и на WebDAV.

Копируется только игровое содержимое (saves/ у клиента, world* у сервера),
без служебных файлов лаунчера. Расписание — глобальные настройки по умолчанию
плюс переопределение на экземпляр.
"""
from __future__ import annotations

import json
import time
import zipfile
from pathlib import Path
from typing import Any, Callable

from .config import ConfigStore, DEFAULT_BACKUP_SCHEDULE, _atomic_json_write, _normalize_backup_schedule
from .errors import LauncherError
from .instances import InstanceStore
from .platform_support import is_reparse_point

BACKUP_CONFIG_FILE = ".trial-launcher-backups.json"
BACKUP_FORMAT = "nivora-world-backup/1"
MAX_BACKUP_BYTES = 2 * 1024 * 1024 * 1024
MAX_BACKUP_FILE_BYTES = 512 * 1024 * 1024
MAX_BACKUP_FILES = 100_000
SERVER_WORLD_NAMES = ("world", "world_nether", "world_the_end")


class WorldBackupStore:
    def __init__(self, config: ConfigStore, instances: InstanceStore) -> None:
        self.config = config
        self.instances = instances

    # ── расписание ────────────────────────────────────────────────────────

    @staticmethod
    def _normalize_instance_schedule(value: dict[str, Any]) -> dict[str, Any]:
        merged = _normalize_backup_schedule(value)
        try:
            merged["last_run"] = float(value.get("last_run", 0.0) or 0.0)
        except (TypeError, ValueError):
            merged["last_run"] = 0.0
        merged["last_status"] = str(value.get("last_status", ""))[:200]
        merged["last_error"] = str(value.get("last_error", ""))[:400]
        return merged

    def schedule(self, name: str) -> dict[str, Any]:
        info = self.instances.get(name)
        merged = _normalize_backup_schedule(getattr(self.config.settings, "backup_schedule", {}))
        stored = self._read_config(info.path)
        if stored.get("override"):
            instance_values = {**merged, **{key: stored[key] for key in stored if key in {"enabled", "interval_hours", "keep", "destination", "last_run", "last_status", "last_error"}}}
            merged = self._normalize_instance_schedule(instance_values)
        merged["override"] = bool(stored.get("override", False))
        return merged

    def set_schedule(self, name: str, values: dict[str, Any], *, override: bool | None = None) -> dict[str, Any]:
        info = self.instances.get(name)
        stored = self._read_config(info.path)
        if override is not None:
            stored["override"] = override
        if not values:
            return self.schedule(name)
        current = self._normalize_instance_schedule({**self.schedule(name), **stored, **{key: value for key, value in values.items() if key in self._normalize_instance_schedule(stored) or key in {"enabled", "interval_hours", "keep", "destination", "last_run", "last_status", "last_error"}}})
        current["override"] = bool(stored.get("override", False))
        self._write_config(info.path, current)
        return self.schedule(name)

    # ── копии ─────────────────────────────────────────────────────────────

    def local_root(self, name: str) -> Path:
        info = self.instances.get(name)
        root = self.config.game_dir / "backups" / info.name
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        return root

    def list(self, name: str) -> list[dict[str, Any]]:
        info = self.instances.get(name)
        result: list[dict[str, Any]] = []
        for path in sorted(self.local_root(name).glob("*.zip")):
            if path.is_symlink() or not path.is_file():
                continue
            result.append(
                {
                    "id": path.stem,
                    "created_at": path.stat().st_mtime,
                    "size_bytes": path.stat().st_size,
                    "destination": "local",
                }
            )
        try:
            from .cloud_storage import CloudStorageError, WebDavStorage

            storage = WebDavStorage(self.config)
            prefix = f"backups/{info.name}/"
            for item in storage.list_files():
                if item.key.startswith(prefix) and item.key.endswith(".zip"):
                    result.append(
                        {
                            "id": Path(item.key).stem + "@cloud",
                            "created_at": item.modified_at or 0.0,
                            "size_bytes": item.size_bytes or 0,
                            "destination": "cloud",
                        }
                    )
        except (LauncherError, OSError):
            pass
        return sorted(result, key=lambda entry: entry["created_at"], reverse=True)

    def create(self, name: str, *, destination: str = "local", reason: str = "manual") -> dict[str, Any]:
        info = self.instances.get(name)
        if info.running:
            raise LauncherError("Остановите экземпляр перед созданием резервной копии мира")
        worlds = self._world_directories(info)
        if not worlds:
            raise LauncherError("В этом экземпляре нет миров для резервного копирования")
        timestamp = time.strftime("%Y%m%d-%H%M%S")
        backup_id = f"{timestamp}-{reason}"
        destination_root = self.local_root(name)
        archive_path = destination_root / f"{backup_id}.zip"
        total = self._write_archive(archive_path, info, worlds)
        if destination == "cloud":
            from .cloud_storage import WebDavStorage

            key = f"backups/{info.name}/{archive_path.name}"
            WebDavStorage(self.config).put_file(key, archive_path)
        self._prune(name, destination)
        schedule = self.schedule(name)
        schedule["last_run"] = time.time()
        schedule["last_status"] = f"сохранено {total} файлов"
        schedule["last_error"] = ""
        self.set_schedule(name, {"last_run": schedule["last_run"], "last_status": schedule["last_status"]})
        return {"backup_id": backup_id, "size_bytes": archive_path.stat().st_size, "files": total, "backups": self.list(name)}

    def restore(self, name: str, backup_id: str) -> dict[str, Any]:
        info = self.instances.get(name)
        if info.running:
            raise LauncherError("Остановите экземпляр перед восстановлением мира")
        archive_path = self._resolve_backup(name, backup_id)
        if archive_path is None:
            raise LauncherError("Резервная копия не найдена")
        worlds_root = self._worlds_root(info)
        worlds_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        restored: list[str] = []
        suffix = time.strftime(".pre-restore-%Y%m%d-%H%M%S")
        with zipfile.ZipFile(archive_path) as archive:
            names = [entry for entry in archive.namelist() if not entry.endswith("/")]
            if len(names) > MAX_BACKUP_FILES:
                raise LauncherError("Резервная копия слишком велика для восстановления")
            for entry in names:
                if entry.startswith("__") or ".." in Path(entry).parts or entry.startswith("/"):
                    raise LauncherError("Резервная копия содержит небезопасные пути")
            restored = self._restore_entries(archive, names, worlds_root, suffix)
        return {"restored": restored, "backups": self.list(name)}

    def delete(self, name: str, backup_id: str) -> dict[str, Any]:
        info = self.instances.get(name)
        if backup_id.endswith("@cloud"):
            from .cloud_storage import WebDavStorage

            WebDavStorage(self.config).delete(f"backups/{info.name}/{backup_id.removesuffix('@cloud')}.zip")
        else:
            path = self.local_root(name) / f"{backup_id}.zip"
            if path.is_symlink() or not path.is_file():
                raise LauncherError("Резервная копия не найдена")
            path.unlink()
        return {"backups": self.list(name)}

    # ── планировщик ───────────────────────────────────────────────────────

    def run_due(self, *, running_check: Callable[[str], bool], emit: Callable[[dict[str, Any]], None] | None = None) -> None:
        for info in self.instances.list():
            if info.game_edition != "java":
                continue
            schedule = self.schedule(info.name)
            if not schedule.get("enabled"):
                continue
            try:
                interval = float(schedule.get("interval_hours", 24)) * 3600.0
            except (TypeError, ValueError):
                continue
            last_run = float(schedule.get("last_run", 0.0) or 0.0)
            if time.time() - last_run < interval:
                continue
            if running_check(info.name):
                continue  # запущенный экземпляр пропускаем, вернёмся на следующем тике
            try:
                self.create(info.name, destination=str(schedule.get("destination", "local")), reason="scheduled")
                if emit:
                    emit({"kind": "status", "event": "world-backup-created", "instance_name": info.name, "message": f"Резервная копия мира «{info.display_name or info.name}» создана по расписанию."})
            except LauncherError as exc:
                current = self.schedule(info.name)
                self.set_schedule(info.name, {"last_run": time.time(), "last_error": str(exc)})
                if emit:
                    emit({"kind": "status", "event": "world-backup-failed", "instance_name": info.name, "message": f"Резервная копия не создана: {exc}"})

    # ── внутреннее ────────────────────────────────────────────────────────

    def _read_config(self, instance_path: Path) -> dict[str, Any]:
        path = instance_path / BACKUP_CONFIG_FILE
        try:
            if path.is_symlink() or not path.is_file() or path.stat().st_size > 16 * 1024:
                return {}
            value = json.loads(path.read_text(encoding="utf-8"))
            return value if isinstance(value, dict) else {}
        except (OSError, ValueError, TypeError):
            return {}

    def _write_config(self, instance_path: Path, values: dict[str, Any]) -> None:
        payload = self._normalize_instance_schedule(values)
        payload["override"] = bool(values.get("override", False))
        _atomic_json_write(instance_path / BACKUP_CONFIG_FILE, payload, private=True)

    @staticmethod
    def _world_directories(info: Any) -> list[Path]:
        if getattr(info, "kind", "client") == "server":
            roots = []
            for name in SERVER_WORLD_NAMES:
                path = info.path / name
                if path.is_dir() and not path.is_symlink():
                    roots.append(path)
            return roots
        saves = info.path / "saves"
        if saves.is_symlink() or not saves.is_dir():
            return []
        return sorted((item for item in saves.iterdir() if item.is_dir() and not item.is_symlink()), key=lambda item: item.name.lower())

    @staticmethod
    def _worlds_root(info: Any) -> Path:
        # у клиента миры лежат в saves/<имя>, у сервера — прямо в корне (world, world_nether…)
        return info.path if getattr(info, "kind", "client") == "server" else info.path / "saves"

    def _write_archive(self, archive_path: Path, info: Any, worlds: list[Path]) -> int:
        temporary = archive_path.with_name(archive_path.name + ".part")
        count = 0
        total_bytes = 0
        manifest = {
            "format": BACKUP_FORMAT,
            "instance": info.name,
            "created_at": time.time(),
            "kind": getattr(info, "kind", "client"),
            "worlds": [world.name for world in worlds],
        }
        try:
            with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
                archive.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False))
                for world in worlds:
                    for path in world.rglob("*"):
                        if count >= MAX_BACKUP_FILES or total_bytes > MAX_BACKUP_BYTES:
                            raise LauncherError("Мир слишком велик для резервной копии (лимит 2 ГиБ)")
                        if path.is_symlink() or is_reparse_point(path) or not path.is_file():
                            continue
                        size = path.stat().st_size
                        if size > MAX_BACKUP_FILE_BYTES:
                            continue
                        archive.write(path, f"worlds/{world.name}/{path.relative_to(world)}")
                        count += 1
                        total_bytes += size
                temporary.replace(archive_path)
        finally:
            if temporary.exists():
                temporary.unlink()
        return count

    def _resolve_backup(self, name: str, backup_id: str) -> Path | None:
        if backup_id.endswith("@cloud"):
            from .cloud_storage import WebDavStorage

            storage = WebDavStorage(self.config)
            content = storage.get(f"backups/{self.instances.get(name).name}/{backup_id.removesuffix('@cloud')}.zip")
            if len(content) > MAX_BACKUP_BYTES:
                raise LauncherError("Резервная копия слишком велика для восстановления")
            import tempfile

            handle, raw_path = tempfile.mkstemp(prefix="nivora-backup-", suffix=".zip")
            import os as _os

            with _os.fdopen(handle, "wb") as target:
                target.write(content)
            return Path(raw_path)
        path = self.local_root(name) / f"{backup_id}.zip"
        return path if path.is_file() and not path.is_symlink() else None

    def _restore_entries(self, archive: zipfile.ZipFile, names: list[str], worlds_root: Path, suffix: str) -> list[str]:
        restored: list[str] = []
        for entry in names:
            if entry == "manifest.json":
                continue
            relative = Path(entry)
            parts = relative.parts  # worlds/<name>/...
            if len(parts) < 3 or parts[0] != "worlds":
                continue
            world_name = parts[1]
            if world_name in {".", ".."} or "/" in world_name:
                raise LauncherError("Резервная копия содержит небезопасное имя мира")
            destination = worlds_root / world_name
            if destination.exists() and not destination.is_symlink() and world_name not in restored:
                destination.rename(worlds_root / f"{world_name}{suffix}")
                restored.append(world_name)
            destination.mkdir(mode=0o700, parents=True, exist_ok=True)
            target = destination.joinpath(*parts[2:])
            if target.is_symlink():
                raise LauncherError("Резервная копия содержит небезопасные пути")
            if not any(part in {"..", ""} for part in parts[2:]):
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(entry) as source, open(target, "wb") as output:
                    output.write(source.read(MAX_BACKUP_FILE_BYTES + 1))
        return restored

    def _prune(self, name: str, destination: str) -> None:
        schedule = self.schedule(name)
        keep = int(schedule.get("keep", 5))
        if destination == "cloud":
            try:
                from .cloud_storage import WebDavStorage

                storage = WebDavStorage(self.config)
                prefix = f"backups/{self.instances.get(name).name}/"
                keys = sorted((item for item in storage.list_files() if item.key.startswith(prefix)), key=lambda item: item.modified_at or 0)
                for item in keys[:-keep] if len(keys) > keep else []:
                    storage.delete(item.key)
            except LauncherError:
                pass
            return
        archives = sorted(self.local_root(name).glob("*.zip"), key=lambda path: path.stat().st_mtime)
        for path in archives[:-keep] if len(archives) > keep else []:
            if not path.is_symlink():
                path.unlink()
