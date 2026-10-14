from __future__ import annotations

import json
import base64
import os
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .accounts import Account
from .config import ConfigStore
from .errors import LauncherError
from .net import HttpClient

PROFILE_SKINS_URL = "https://api.minecraftservices.com/minecraft/profile/skins"
PROFILE_URL = "https://api.minecraftservices.com/minecraft/profile"
PROFILE_CAPE_URL = "https://api.minecraftservices.com/minecraft/profile/capes/active"
_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


@dataclass
class SkinInfo:
    skin_id: str
    name: str
    path: Path
    model: str = "classic"
    image: str = ""


class SkinStore:
    def __init__(self, config: ConfigStore) -> None:
        self.root = config.paths.data_home / "skins"
        self.index = self.root / "skins.json"
        if self.root.is_symlink():
            raise LauncherError("Каталог скинов является символьной ссылкой")
        self.root.mkdir(parents=True, exist_ok=True)
        if self.root.is_symlink() or not self.root.is_dir():
            raise LauncherError("Каталог скинов недоступен")

    def list(self) -> list[SkinInfo]:
        values: list[SkinInfo] = []
        for raw in self._read():
            if not isinstance(raw, dict):
                continue
            skin_id = str(raw.get("skin_id", ""))
            path = _safe_skin_path(self.root, str(raw.get("filename", "")))
            if skin_id and path and path.is_file() and path.stat().st_size <= 16 * 1024 * 1024:
                try:
                    _validate_png_skin(path)
                except LauncherError:
                    continue
                values.append(
                    SkinInfo(
                        skin_id,
                        str(raw.get("name", skin_id)),
                        path,
                        str(raw.get("model", "classic")),
                        _image_data_url(path),
                    )
                )
        return values

    def get(self, skin_id: str) -> SkinInfo:
        for value in self.list():
            if value.skin_id == skin_id:
                return value
        raise LauncherError(f"Скин не найден: {skin_id}")

    def import_file(self, source: Path, name: str | None = None, model: str = "classic") -> SkinInfo:
        source = source.expanduser().resolve()
        if not source.is_file():
            raise LauncherError(f"PNG-скин не найден: {source}")
        return self.import_bytes(source.read_bytes(), source.name, name=name, model=model)

    def import_bytes(self, content: bytes, filename: str, name: str | None = None, model: str = "classic") -> SkinInfo:
        _validate_png_skin_bytes(content)
        if model not in {"classic", "slim"}:
            raise LauncherError(f"Неизвестная модель скина: {model}")
        source_name = Path(filename).stem
        skin_id = uuid.uuid4().hex[:12]
        stored_filename = f"{skin_id}.png"
        target = self.root / stored_filename
        fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
        values = self._read()
        values.append({"skin_id": skin_id, "name": (name or source_name).strip()[:80] or skin_id, "filename": stored_filename, "model": model})
        self._write(values)
        return SkinInfo(skin_id, values[-1]["name"], target, model, _image_data_url(target))

    def _read(self) -> list[dict[str, Any]]:
        if not self.index.exists():
            return []
        try:
            value = json.loads(self.index.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Не удалось прочитать библиотеку скинов: {exc}") from exc
        return value if isinstance(value, list) else []

    def _write(self, values: list[dict[str, Any]]) -> None:
        if self.index.is_symlink():
            raise LauncherError("Индекс скинов является символьной ссылкой")
        fd = os.open(self.index, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(json.dumps(values, ensure_ascii=False, indent=2) + "\n")


def apply_skin(http: HttpClient, account: Account, skin: SkinInfo, model: str) -> None:
    if account.kind != "microsoft" or not account.access_token:
        raise LauncherError("Скин можно установить только на Microsoft-аккаунт")
    if model not in {"classic", "slim"}:
        raise LauncherError(f"Неизвестная модель скина: {model}")
    boundary = f"----TrialLauncher{uuid.uuid4().hex}"
    data = skin.path.read_bytes()
    body = (
        f"--{boundary}\r\n"
        'Content-Disposition: form-data; name="variant"\r\n\r\n'
        f"{model}\r\n"
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{skin.path.name}"\r\n'
        "Content-Type: image/png\r\n\r\n"
    ).encode("utf-8") + data + f"\r\n--{boundary}--\r\n".encode("utf-8")
    http.request(
        PROFILE_SKINS_URL,
        method="POST",
        data=body,
        headers={
            "Authorization": f"Bearer {account.access_token}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "Accept": "application/json",
        },
    )


def profile(
    http: HttpClient,
    account: Account,
    refresh_account: Callable[[], Account] | None = None,
) -> dict[str, Any]:
    if account.kind != "microsoft" or not account.access_token:
        raise LauncherError("Профиль скинов доступен только Microsoft-аккаунту")
    try:
        value = _profile_data(http, account)
    except LauncherError as exc:
        # A Minecraft token can be revoked before its local expiry timestamp.
        # Retry once with a freshly exchanged token instead of showing an empty profile.
        if refresh_account is None or "HTTP 401" not in str(exc):
            raise
        account = refresh_account()
        if account.kind != "microsoft" or not account.access_token:
            raise LauncherError("Не удалось обновить Microsoft-аккаунт для загрузки скинов") from exc
        value = _profile_data(http, account)
    _normalize_profile_entries(value)
    _hydrate_profile_images(http, value)
    return value


def _profile_data(http: HttpClient, account: Account) -> dict[str, Any]:
    value = http.get_json(PROFILE_URL, headers={"Authorization": f"Bearer {account.access_token}"})
    if not isinstance(value, dict):
        raise LauncherError("Minecraft Services вернул некорректный профиль")
    return value


def _normalize_profile_entries(profile_data: dict[str, Any]) -> None:
    for key in ("skins", "capes"):
        entries = profile_data.get(key)
        if not isinstance(entries, list):
            profile_data[key] = []
            continue
        normalized: list[dict[str, Any]] = []
        active_index: int | None = None
        for raw in entries:
            if not isinstance(raw, dict):
                continue
            entry = dict(raw)
            state = str(entry.get("state", "")).strip().upper()
            if state == "ACTIVE" and active_index is None:
                active_index = len(normalized)
            alias = str(entry.get("alias", "")).strip()
            name = str(entry.get("name", "")).strip()
            if key == "capes" and alias and not name:
                entry["name"] = alias
            normalized.append(entry)
        if normalized and active_index is None:
            normalized[0]["state"] = "ACTIVE"
        profile_data[key] = normalized


def _hydrate_profile_images(http: HttpClient, profile_data: dict[str, Any]) -> None:
    """Return same-origin data URLs so the WebGL preview is not blocked by CORS."""
    for key in ("skins", "capes"):
        entries = profile_data.get(key)
        if not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            url = str(entry.get("url", "")).strip()
            if not url:
                continue
            try:
                content = http.request(url, max_bytes=4 * 1024 * 1024)
                if key == "skins":
                    _validate_png_skin_bytes(content)
                elif not content.startswith(_PNG_SIGNATURE):
                    continue
            except (LauncherError, OSError):
                continue
            entry["url"] = f"data:image/png;base64,{base64.b64encode(content).decode('ascii')}"


def apply_cape(http: HttpClient, account: Account, cape_id: str) -> None:
    if account.kind != "microsoft" or not account.access_token:
        raise LauncherError("Плащ можно установить только на Microsoft-аккаунт")
    cape_id = str(cape_id or "").strip()
    if not cape_id:
        raise LauncherError("Выберите плащ из профиля Minecraft")
    profile_data = _profile_data(http, account)
    capes = profile_data.get("capes", [])
    if not isinstance(capes, list) or not any(
        isinstance(cape, dict) and str(cape.get("id", "")).strip() == cape_id
        for cape in capes
    ):
        raise LauncherError("Можно выбрать только плащ, доступный в профиле Minecraft")
    body = json.dumps({"capeId": cape_id}).encode("utf-8")
    http.request(
        PROFILE_CAPE_URL,
        method="PUT",
        data=body,
        headers={"Authorization": f"Bearer {account.access_token}", "Content-Type": "application/json"},
    )


def clear_cape(http: HttpClient, account: Account) -> None:
    if account.kind != "microsoft" or not account.access_token:
        raise LauncherError("Плащ можно снять только на Microsoft-аккаунте")
    http.request(PROFILE_CAPE_URL, method="DELETE", headers={"Authorization": f"Bearer {account.access_token}"})


def clear_skin(http: HttpClient, account: Account) -> None:
    if account.kind != "microsoft" or not account.access_token:
        raise LauncherError("Скин можно сбросить только на Microsoft-аккаунте")
    http.request(PROFILE_SKINS_URL + "/active", method="DELETE", headers={"Authorization": f"Bearer {account.access_token}"})


def _validate_png_skin(path: Path) -> None:
    try:
        _validate_png_skin_bytes(path.read_bytes())
    except OSError as exc:
        raise LauncherError(f"Не удалось прочитать PNG-скин: {exc}") from exc


def _validate_png_skin_bytes(content: bytes) -> None:
    header = content[:24]
    if len(header) < 24 or header[:8] != _PNG_SIGNATURE or header[12:16] != b"IHDR":
        raise LauncherError("Файл не является корректным PNG-скином")
    width = int.from_bytes(header[16:20], "big")
    height = int.from_bytes(header[20:24], "big")
    if (width, height) not in {(64, 32), (64, 64), (128, 64), (128, 128)}:
        raise LauncherError("Поддерживаются PNG-скины 64x32, 64x64, 128x64 или 128x128")


def _image_data_url(path: Path) -> str:
    try:
        encoded = base64.b64encode(path.read_bytes()).decode("ascii")
    except OSError:
        return ""
    return f"data:image/png;base64,{encoded}"


def _safe_skin_path(root: Path, filename: str) -> Path | None:
    candidate = Path(filename)
    if not filename or candidate.name != filename or candidate.is_absolute() or candidate.name in {".", ".."} or candidate.suffix.lower() != ".png":
        return None
    path = root / candidate.name
    try:
        if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
            return None
    except OSError:
        return None
    return path
