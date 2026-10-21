"""фолбэк-проверка обновлений через GitHub Releases API."""

from __future__ import annotations

import re
from typing import Any

from .constants import APP_VERSION, GITHUB_RELEASES_API_URL, GITHUB_RELEASES_PAGE_URL
from .errors import LauncherError
from .net import HttpClient

# «1.1», «v1.1.0», «1.0.3-1» (пакетные суффиксы) — всё это версии лаунчера.
_VERSION_RE = re.compile(r"^[vV]?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:[-+][0-9A-Za-z.-]+)?$")

MAX_RELEASE_ASSETS = 12
MAX_RELEASE_NOTES = 4000

def app_version_key(version: str) -> tuple[int, int, int]:
    """Числовой ключ версии: «1.1» == «1.1.0», мусор -> (0, 0, 0)."""
    match = _VERSION_RE.fullmatch(str(version or "").strip())
    if not match:
        return (0, 0, 0)
    return tuple(int(match.group(index) or 0) for index in (1, 2, 3))  # type: ignore[return-value]

def is_newer_version(candidate: str, current: str) -> bool:
    return app_version_key(candidate) > app_version_key(current)

def check_remote_release(http: HttpClient) -> dict[str, Any]:
    """Вернуть информацию о последнем релизе и наличие обновления."""
    try:
        payload = http.get_json(
            GITHUB_RELEASES_API_URL,
            headers={"Accept": "application/vnd.github+json"},
        )
    except LauncherError as exc:
        raise LauncherError(f"GitHub не ответил: {exc}") from exc
    if not isinstance(payload, dict) or not str(payload.get("tag_name", "")).strip():
        raise LauncherError("GitHub вернул некорректный ответ о последнем релизе")
    tag = str(payload["tag_name"]).strip()
    version = tag.lstrip("vV")
    release_url = str(payload.get("html_url", "")).strip() or GITHUB_RELEASES_PAGE_URL
    assets: list[dict[str, str]] = []
    for asset in payload.get("assets") or []:
        if not isinstance(asset, dict):
            continue
        name = str(asset.get("name", "")).strip()
        url = str(asset.get("browser_download_url", "")).strip()
        if name and url:
            assets.append({"name": name, "url": url})
    return {
        "update_available": is_newer_version(version, APP_VERSION),
        "latest_version": version,
        "current_version": APP_VERSION,
        "release_url": release_url,
        "published_at": str(payload.get("published_at", "") or ""),
        "notes": str(payload.get("body", "") or "")[:MAX_RELEASE_NOTES],
        "assets": assets[:MAX_RELEASE_ASSETS],
    }
