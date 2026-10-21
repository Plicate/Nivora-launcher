"""пресеты глобальных настроек: снимок, применение, JSON-импорт/экспорт."""
from __future__ import annotations

import json
import re
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any

from .config import ConfigStore
from .errors import LauncherError

PROFILE_FORMAT = "nivora-settings-profile/1"
PROFILE_DIR_NAME = "settings-profiles"
MAX_PROFILE_JSON_BYTES = 1024 * 1024
EXCLUDED_KEYS = frozenset({"game_dir", "client_id", "launcher_version"})

def _settings_field_names() -> frozenset[str]:
    from dataclasses import fields as dc_fields

    from .config import Settings

    return frozenset(item.name for item in dc_fields(Settings))

def _profile_dir(config: ConfigStore) -> Path:
    directory = config.paths.config_home / PROFILE_DIR_NAME
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    return directory

def _safe_profile_name(name: str) -> str:
    value = re.sub(r"[^A-Za-z0-9А-Яа-яЁё0-9._ -]+", "-", str(name).strip())[:48].strip(" .-")
    if not value:
        raise LauncherError("Имя профиля настроек не может быть пустым")
    return value

def _profile_path(config: ConfigStore, name: str) -> Path:
    return _profile_dir(config) / f"{_safe_profile_name(name)}.json"

def _dump_profile(document: dict[str, Any]) -> str:
    return json.dumps(document, ensure_ascii=False, indent=2)

def _read_profile(path: Path) -> dict[str, Any]:
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_PROFILE_JSON_BYTES:
            raise LauncherError("Файл профиля настроек недоступен")
        document = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise LauncherError(f"Не удалось прочитать профиль настроек: {exc}") from exc
    except ValueError as exc:
        raise LauncherError(f"Профиль настроек повреждён: {exc}") from exc
    if not isinstance(document, dict) or document.get("format") != PROFILE_FORMAT or not isinstance(document.get("settings"), dict):
        raise LauncherError("Файл не похож на профиль настроек Nivora")
    return document

def list_profiles(config: ConfigStore) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    for path in sorted(_profile_dir(config).glob("*.json")):
        try:
            document = _read_profile(path)
        except LauncherError:
            continue
        settings = document.get("settings") or {}
        result.append(
            {
                "name": str(document.get("name") or path.stem),
                "created_at": float(document.get("created_at") or 0.0),
                "updated_at": float(document.get("updated_at") or 0.0),
                "key_count": len(settings),
            }
        )
    return result

def save_profile(config: ConfigStore, name: str) -> dict[str, Any]:
    payload = {key: value for key, value in asdict(config.settings).items() if key not in EXCLUDED_KEYS}
    now = time.time()
    path = _profile_path(config, name)
    document: dict[str, Any] = {"format": PROFILE_FORMAT, "name": _safe_profile_name(name), "created_at": now, "updated_at": now, "settings": payload}
    if path.exists():
        try:
            document["created_at"] = _read_profile(path).get("created_at", now)
        except LauncherError:
            pass
    path.write_text(_dump_profile(document), encoding="utf-8")
    return {"profiles": list_profiles(config)}

def apply_profile(config: ConfigStore, name: str) -> dict[str, Any]:
    document = _read_profile(_profile_path(config, name))
    valid = _settings_field_names()
    payload = {key: value for key, value in document["settings"].items() if key in valid and key not in EXCLUDED_KEYS}
    if not payload:
        raise LauncherError("Профиль настроек пуст")
    config.set_values(payload)
    return {**payload}

def delete_profile(config: ConfigStore, name: str) -> dict[str, Any]:
    path = _profile_path(config, name)
    if path.is_symlink() or not path.exists():
        raise LauncherError("Профиль настроек не найден")
    path.unlink()
    return {"profiles": list_profiles(config)}

def export_profile(config: ConfigStore, name: str) -> dict[str, str]:
    path = _profile_path(config, name)
    document = _read_profile(path)
    return {
        "name": str(document.get("name") or path.stem),
        "file_name": f"{path.stem}.json",
        "json": _dump_profile(document),
    }

def import_profile(config: ConfigStore, json_text: str, *, name: str = "") -> dict[str, Any]:
    if not json_text.strip() or len(json_text.encode("utf-8")) > MAX_PROFILE_JSON_BYTES:
        raise LauncherError("Пустой или слишком большой файл профиля")
    try:
        document = json.loads(json_text)
    except ValueError as exc:
        raise LauncherError(f"Некорректный JSON: {exc}") from exc
    if not isinstance(document, dict) or document.get("format") != PROFILE_FORMAT or not isinstance(document.get("settings"), dict):
        raise LauncherError("Файл не похож на профиль настроек Nivora")
    valid = _settings_field_names()
    unknown = sorted(key for key in document["settings"] if key not in valid)
    if unknown:
        raise LauncherError(f"Профиль содержит неизвестные настройки: {', '.join(unknown[:8])}")
    profile_name = _safe_profile_name(name or str(document.get("name") or "imported"))
    document["name"] = profile_name
    document["updated_at"] = time.time()
    _profile_path(config, profile_name).write_text(_dump_profile(document), encoding="utf-8")
    return {"profiles": list_profiles(config)}
