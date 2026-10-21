"""лёгкие Python-хелперы Minecraft Bedrock."""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import sys
from pathlib import Path
from typing import TYPE_CHECKING

from .config import ConfigStore
from .errors import LauncherError
from .instances import InstanceInfo, InstanceStore

if TYPE_CHECKING:
    from .operations import CancellationToken

MAX_CONTENT_UPLOAD_BYTES = 128 * 1024 * 1024

BEDROCK_ON_LINUX_URL = "https://github.com/Wyze3306/BedrockOnLinux"

_PROFILE_FILE = "profile.json"
_PROFILE_NAME_RE = re.compile(r"nivora-[0-9a-f]{16}\Z")
_MAX_PROFILE_JSON_BYTES = 64 * 1024

# windows: официальные пакеты Microsoft Store и их каталог контента.
_WINDOWS_PACKAGE_FAMILIES = {
    "release": "Microsoft.MinecraftUWP_8wekyb3d8bbwe",
    "preview": "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe",
}

BOL_CONTENT_ROOT_PARTS = (
    "compatdata", "pfx", "drive_c", "users", "steamuser", "AppData", "Roaming",
    "Minecraft Bedrock", "Users",
)

def _profile_name(instance_name: str) -> str:
    digest = hashlib.sha256(instance_name.encode("utf-8")).hexdigest()
    return f"nivora-{digest[:16]}"

def windows_content_root(edition: str = "release") -> Path:
    """Каталог com.mojang официальной игры Windows (данные пакета Store)."""
    family = _WINDOWS_PACKAGE_FAMILIES.get(edition, _WINDOWS_PACKAGE_FAMILIES["release"])
    local_appdata = os.environ.get("LOCALAPPDATA", "")
    if not local_appdata:
        raise LauncherError("Не удалось определить каталог данных Minecraft (LOCALAPPDATA)")
    return Path(local_appdata) / "Packages" / family / "LocalState" / "games" / "com.mojang"

def _require_profile_idle(profile_path: Path) -> None:
    """профиль не должен использоваться запущенной игрой (env BoL + flock)."""
    if sys.platform != "linux":
        raise LauncherError("Управление профилем Minecraft Bedrock доступно только в Linux")
    resolved = str(profile_path.resolve())
    prefix = str((profile_path / "compatdata" / "pfx").resolve())
    for process_dir in Path("/proc").glob("[0-9]*"):
        if process_dir.name == str(os.getpid()):
            continue
        try:
            entries = process_dir.joinpath("environ").read_bytes().split(b"\0")
        except OSError:
            continue
        if any(
            entry in {b"BOL_HOME=" + os.fsencode(resolved), b"WINEPREFIX=" + os.fsencode(prefix)}
            for entry in entries
        ):
            raise LauncherError("Профиль Minecraft Bedrock ещё используется")

    lock_path = profile_path / ".launch.lock"
    if lock_path.is_symlink():
        raise LauncherError("Блокировка профиля Bedrock является ссылкой")
    if not lock_path.exists():
        return
    try:
        import fcntl

        fd = os.open(lock_path, os.O_RDWR | getattr(os, "O_NOFOLLOW", 0))
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as exc:
                raise LauncherError("Профиль Bedrock используется запущенной игрой") from exc
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError("Не удалось проверить блокировку профиля Bedrock") from exc

class BedrockProfileStore:
    """лёгкий доступ к путям профилей Bedrock."""

    def __init__(
        self,
        config: ConfigStore,
        instances: InstanceStore | None = None,
        cancellation: "CancellationToken | None" = None,
    ) -> None:
        self.config = config
        self.instances = instances or InstanceStore(config)
        self.cancellation = cancellation

    @property
    def root(self) -> Path:
        return Path(self.config.game_dir).expanduser() / "bedrock"

    @property
    def profiles_root(self) -> Path:
        return self.root / "profiles"

    def profile_path(self, info: InstanceInfo) -> Path:
        """корень данных игры для экземпляра Bedrock."""
        if info.game_edition != "bedrock":
            raise LauncherError("Операция доступна только для Minecraft Bedrock")
        if sys.platform == "win32":
            # официальной версии профиль среды не нужен; выбор канала
            edition = "release"
            for metadata_name in (".nivora-launcher-instance.json", ".trial-launcher-instance.json"):
                metadata_path = info.path / metadata_name
                try:
                    if metadata_path.is_file() and not metadata_path.is_symlink():
                        raw = json.loads(metadata_path.read_text(encoding="utf-8"))
                        value = str(raw.get("bedrock_edition", "")) if isinstance(raw, dict) else ""
                        if value in {"release", "preview"}:
                            edition = value
                            break
                except (OSError, ValueError):
                    continue
            return windows_content_root(edition)
        expected_name = _profile_name(info.name)
        if info.bedrock_profile != expected_name:
            raise LauncherError("Профиль Bedrock не совпадает с экземпляром")
        profile_path = self.profiles_root / expected_name
        self._validate_profile_path(profile_path, expected_name)
        return profile_path

    def delete_profile(self, info: InstanceInfo) -> None:
        """Удалить профиль BoL (Linux). Windows-официальной версии чистить нечего."""
        if info.game_edition != "bedrock":
            return
        if sys.platform == "win32":
            return
        profile_name = _profile_name(info.name)
        if info.bedrock_profile != profile_name:
            raise LauncherError("Профиль Bedrock не совпадает с экземпляром")
        profile_path = self.profiles_root / profile_name
        if not os.path.lexists(profile_path):
            return
        self._validate_profile_path(profile_path, profile_name)
        _require_profile_idle(profile_path)
        self._remove_profile_path(profile_path, profile_name)

    def snapshot_root(self, info: InstanceInfo) -> Path:
        """каталог контента com.mojang для снапшотов/скриншотов Bedrock."""
        profile = self.profile_path(info)
        if sys.platform == "win32":
            return profile
        users_root = profile.joinpath(*BOL_CONTENT_ROOT_PARTS)
        try:
            if users_root.is_dir():
                scopes = sorted(
                    (item for item in users_root.iterdir() if item.is_dir() and not item.is_symlink()),
                    key=lambda item: item.name.lower(),
                )
                for scope in scopes:
                    if not re.fullmatch(r"[A-Za-z0-9_-]+", scope.name):
                        continue
                    candidate = scope / "games" / "com.mojang"
                    if candidate.is_dir():
                        return candidate
        except OSError:
            pass
        return profile / "games" / "com.mojang"

    def _ensure_root(self) -> Path:
        root = self.root
        profiles = self.profiles_root
        if root.is_symlink() or profiles.is_symlink():
            raise LauncherError("Каталог Bedrock является символьной ссылкой")
        profiles.mkdir(parents=True, exist_ok=True)
        if root.is_symlink() or not profiles.is_dir():
            raise LauncherError("Каталог Bedrock недоступен")
        os.chmod(root, 0o700)
        os.chmod(profiles, 0o700)
        return root

    def _validate_profile_path(self, profile_path: Path, profile_name: str) -> None:
        if not _PROFILE_NAME_RE.fullmatch(profile_name):
            raise LauncherError("Недопустимый идентификатор профиля Bedrock")
        root = self.profiles_root
        if profile_path.is_symlink() or not profile_path.is_dir():
            raise LauncherError("Профиль Bedrock не найден")
        if profile_path.resolve().parent != root.resolve():
            raise LauncherError("Путь профиля Bedrock выходит за пределы хранилища")
        metadata_path = profile_path / _PROFILE_FILE
        if metadata_path.is_symlink() or not metadata_path.is_file():
            raise LauncherError("Метаданные профиля Bedrock не найдены")
        if metadata_path.stat().st_size > _MAX_PROFILE_JSON_BYTES:
            raise LauncherError("Метаданные профиля Bedrock повреждены")
        try:
            metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise LauncherError("Не удалось прочитать профиль Bedrock") from exc
        if not isinstance(metadata, dict) or metadata.get("slug") != profile_name:
            raise LauncherError("Идентификатор профиля Bedrock не совпадает")

    def _remove_profile_path(self, profile_path: Path, profile_name: str) -> None:
        data_root = self.root / ".nivora-xdg-data" / "applications"
        shortcuts = [
            data_root / f"bedrock-on-linux-profile-{profile_name}.desktop",
            data_root / f"bedrock-on-linux-play-{profile_name}.desktop",
        ]
        for shortcut in shortcuts:
            if shortcut.is_symlink():
                raise LauncherError("Ярлык профиля Bedrock является ссылкой")
        shutil.rmtree(profile_path)
        for shortcut in shortcuts:
            if shortcut.is_file():
                shortcut.unlink()
