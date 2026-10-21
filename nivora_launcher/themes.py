from __future__ import annotations

import hashlib
import io
import json
import os
import re
import shutil
import tempfile
import uuid
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urlencode, urlsplit

from .config import ConfigStore, _atomic_json_write
from .errors import LauncherError
from .net import HttpClient

THEME_GITHUB_TOPIC = "trial-launcher-theme"
THEME_MAX_ARCHIVE_BYTES = 8 * 1024 * 1024
THEME_MAX_JSON_BYTES = 64 * 1024
THEME_MAX_METADATA_BYTES = 16 * 1024
THEME_MAX_FILES = 2
THEME_MAX_UNPACKED_BYTES = THEME_MAX_JSON_BYTES + THEME_MAX_METADATA_BYTES
GITHUB_API_URL = "https://api.github.com"
GITHUB_API_VERSION = "2022-11-28"
_THEME_ID_RE = re.compile(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")
_THEME_VERSION_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$")
_THEME_COLOR_RE = re.compile(r"^#[0-9a-fA-F]{6}$")
_SEARCH_RE = re.compile(r"^[A-Za-z0-9._+ -]{0,120}$")
_OWNER_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$")
_REPO_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
_API_HOSTS = frozenset({"api.github.com"})
_ASSET_HOSTS = frozenset(
    {
        "api.github.com",
        "github.com",
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
    }
)

def _text(value: Any, field: str, limit: int) -> str:
    result = str(value or "").strip()
    if not result or len(result) > limit:
        raise LauncherError(f"Некорректное поле темы: {field}")
    return result

def normalize_theme(
    value: Any,
    metadata: Any,
    *,
    source: str = "local",
    source_url: str = "",
) -> dict[str, Any]:
    if not isinstance(value, dict) or not isinstance(metadata, dict):
        raise LauncherError("ZIP темы должен содержать theme.json и theme.yml с объектами темы и описания")
    if value.get("schema_version", 1) != 1:
        raise LauncherError("Эта версия схемы темы не поддерживается")
    theme_id = _text(value.get("id"), "id", 48).lower()
    if not _THEME_ID_RE.fullmatch(theme_id):
        raise LauncherError("id темы должен быть в формате kebab-case")
    name = _text(value.get("name"), "name", 80)
    author = _text(metadata.get("author"), "author", 120)
    description = _text(metadata.get("description"), "description", 500)
    version = _text(metadata.get("version"), "version", 64)
    if not _THEME_VERSION_RE.fullmatch(version):
        raise LauncherError("Версия темы содержит недопустимые символы")
    creators = metadata.get("creators")
    if not isinstance(creators, list) or not creators or len(creators) > 16:
        raise LauncherError("theme.yml должен содержать список creators от 1 до 16 авторов")
    creators = list(dict.fromkeys(_text(item, "creators", 120) for item in creators))

    colors: dict[str, str] = {}
    for field in ("accent", "accent_strong", "background", "panel", "panel_soft"):
        color = str(value.get(field, "")).strip()
        if not _THEME_COLOR_RE.fullmatch(color):
            raise LauncherError(f"Цвет темы {field} должен иметь формат #rrggbb")
        colors[field] = color.lower()

    mode = str(value.get("mode", "")).strip().lower()
    density = str(value.get("density", "")).strip().lower()
    sidebar_mode = str(value.get("sidebar_mode", "")).strip().lower()
    if mode not in {"system", "light", "dark", "glass"}:
        raise LauncherError("Некорректный режим темы")
    if density not in {"compact", "comfortable", "spacious"}:
        raise LauncherError("Некорректная плотность темы")
    if sidebar_mode not in {"expanded", "adaptive", "compact"}:
        raise LauncherError("Некорректный режим боковой панели темы")
    if source not in {"local", "github"}:
        raise LauncherError("Неизвестный источник темы")

    return {
        "schema_version": 1,
        "id": theme_id,
        "name": name,
        "author": author,
        "description": description,
        "version": version,
        "creators": creators,
        "source": source,
        "source_url": source_url if source == "github" else "",
        "mode": mode,
        **colors,
        "density": density,
        "sidebar_mode": sidebar_mode,
        "featured": value.get("featured") is True,
    }

def _strip_yaml_comment(line: str) -> str:
    quote = ""
    escaped = False
    for index, char in enumerate(line):
        if escaped:
            escaped = False
        elif char == "\\" and quote == '"':
            escaped = True
        elif char in {"'", '"'}:
            if not quote:
                quote = char
            elif quote == char:
                quote = ""
        elif char == "#" and not quote and (index == 0 or line[index - 1].isspace()):
            return line[:index]
    return line

def _yaml_scalar(raw: str) -> Any:
    value = raw.strip()
    if not value or value.startswith(("!", "&", "*", "|", ">")):
        raise LauncherError("theme.yml содержит неподдерживаемое YAML-значение")
    if value.startswith('"'):
        try:
            result = json.loads(value)
        except ValueError as exc:
            raise LauncherError("Некорректная строка в theme.yml") from exc
        if not isinstance(result, str):
            raise LauncherError("Значения метаданных theme.yml должны быть строками")
        return result
    if value.startswith("'") and value.endswith("'"):
        return value[1:-1].replace("''", "'")
    if value.startswith("[") and value.endswith("]"):
        try:
            result = json.loads(value)
        except ValueError as exc:
            raise LauncherError("Список creators в theme.yml должен быть JSON-подобным") from exc
        if not isinstance(result, list) or any(not isinstance(item, str) for item in result):
            raise LauncherError("Список creators в theme.yml должен содержать строки")
        return result
    return value

def parse_theme_yml(content: bytes) -> dict[str, Any]:
    if not content or len(content) > THEME_MAX_METADATA_BYTES:
        raise LauncherError("theme.yml пустой или слишком большой")
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise LauncherError("theme.yml должен быть в UTF-8") from exc
    result: dict[str, Any] = {}
    creators_list_open = False
    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        if "\t" in raw_line:
            raise LauncherError(f"theme.yml содержит tab на строке {line_number}")
        line = _strip_yaml_comment(raw_line).rstrip()
        if not line.strip() or line.lstrip().startswith("---"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        value = line.strip()
        if indent == 0:
            creators_list_open = False
            if ":" not in value:
                raise LauncherError(f"Некорректная строка theme.yml: {line_number}")
            key, raw_value = value.split(":", 1)
            key = key.strip()
            if key not in {"author", "description", "version", "creators"} or key in result:
                raise LauncherError(f"Недопустимое или повторяющееся поле theme.yml: {key}")
            if key == "creators" and not raw_value.strip():
                result[key] = []
                creators_list_open = True
            else:
                result[key] = _yaml_scalar(raw_value)
            continue
        if indent == 2 and creators_list_open and value.startswith("- "):
            result["creators"].append(_yaml_scalar(value[2:]))
            continue
        raise LauncherError(f"Некорректный отступ в theme.yml на строке {line_number}")
    if set(result) != {"author", "description", "version", "creators"}:
        raise LauncherError("theme.yml должен содержать author, description, version и creators")
    if not isinstance(result["creators"], list):
        raise LauncherError("creators в theme.yml должен быть списком")
    return result

def _theme_document(theme: dict[str, Any]) -> dict[str, Any]:
    return {
        key: theme[key]
        for key in (
            "schema_version",
            "id",
            "name",
            "author",
            "description",
            "mode",
            "accent",
            "accent_strong",
            "background",
            "panel",
            "panel_soft",
            "density",
            "sidebar_mode",
        )
    }

def _theme_metadata(theme: dict[str, Any]) -> dict[str, Any]:
    return {key: theme[key] for key in ("author", "description", "version", "creators")}

def _metadata_yaml(theme: dict[str, Any]) -> str:
    metadata = _theme_metadata(theme)
    lines = [
        f"author: {json.dumps(metadata['author'], ensure_ascii=False)}",
        f"description: {json.dumps(metadata['description'], ensure_ascii=False)}",
        f"version: {json.dumps(metadata['version'], ensure_ascii=False)}",
        "creators:",
    ]
    lines.extend(f"  - {json.dumps(creator, ensure_ascii=False)}" for creator in metadata["creators"])
    return "\n".join(lines) + "\n"

def _read_archive(content: bytes) -> tuple[dict[str, Any], dict[str, Any]]:
    if not content or len(content) > THEME_MAX_ARCHIVE_BYTES:
        raise LauncherError("ZIP темы пустой или превышает 8 MiB")
    try:
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            members = archive.infolist()
            if len(members) != THEME_MAX_FILES:
                raise LauncherError("ZIP темы должен содержать ровно theme.json и theme.yml")
            files: dict[str, zipfile.ZipInfo] = {}
            total_size = 0
            for member in members:
                name = member.filename.replace("\\", "/")
                path = PurePosixPath(name)
                if member.is_dir() or path.is_absolute() or len(path.parts) != 1 or any(part in {"", ".", ".."} for part in path.parts):
                    raise LauncherError("ZIP темы может содержать только файлы в корне архива")
                if name not in {"theme.json", "theme.yml"} or name in files:
                    raise LauncherError("ZIP темы должен содержать ровно theme.json и theme.yml")
                if ((member.external_attr >> 16) & 0o170000) == 0o120000:
                    raise LauncherError("ZIP темы не может содержать символьные ссылки")
                limit = THEME_MAX_JSON_BYTES if name == "theme.json" else THEME_MAX_METADATA_BYTES
                if member.file_size > limit:
                    raise LauncherError(f"Файл {name} в ZIP темы слишком большой")
                total_size += member.file_size
                files[name] = member
            if set(files) != {"theme.json", "theme.yml"} or total_size > THEME_MAX_UNPACKED_BYTES:
                raise LauncherError("ZIP темы неполный или слишком большой")
            raw_json = archive.read(files["theme.json"])
            raw_yaml = archive.read(files["theme.yml"])
    except LauncherError:
        raise
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as exc:
        raise LauncherError("Не удалось прочитать ZIP темы") from exc
    try:
        theme = json.loads(raw_json.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise LauncherError("theme.json должен содержать корректный JSON в UTF-8") from exc
    if not isinstance(theme, dict):
        raise LauncherError("theme.json должен содержать объект темы")
    return theme, parse_theme_yml(raw_yaml)

class ThemeStore:
    def __init__(self, config: ConfigStore) -> None:
        self.root = config.paths.data_home / "themes"
        if self.root.is_symlink():
            raise LauncherError("Каталог тем является символьной ссылкой")
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        if self.root.is_symlink() or not self.root.is_dir():
            raise LauncherError("Каталог тем недоступен")

    def list(self) -> list[dict[str, Any]]:
        result: list[dict[str, Any]] = []
        for directory in sorted(self.root.iterdir(), key=lambda item: item.name.lower())[:256]:
            if directory.name.startswith(".") or directory.is_symlink() or not directory.is_dir():
                continue
            try:
                result.append(self._read_theme(directory))
            except (LauncherError, OSError, UnicodeError, ValueError, TypeError):
                continue
        return result

    def preview_archive(self, content: bytes, *, source: str = "local", source_url: str = "") -> dict[str, Any]:
        theme, metadata = _read_archive(content)
        return normalize_theme(theme, metadata, source=source, source_url=source_url)

    def save_theme(
        self,
        value: Any,
        metadata: Any,
        *,
        source: str = "local",
        source_url: str = "",
        replace: bool = False,
    ) -> dict[str, Any]:
        theme = normalize_theme(value, metadata, source=source, source_url=source_url)
        return self._persist(theme, replace=replace)

    def import_archive(
        self,
        content: bytes,
        *,
        source: str = "local",
        source_url: str = "",
        replace: bool = False,
    ) -> dict[str, Any]:
        theme = self.preview_archive(content, source=source, source_url=source_url)
        return self._persist(theme, replace=replace)

    def delete(self, theme_id: str) -> None:
        if not _THEME_ID_RE.fullmatch(str(theme_id)):
            raise LauncherError("Некорректный id темы")
        target = self.root / theme_id
        if target.is_symlink():
            raise LauncherError("Каталог темы является символьной ссылкой")
        if not target.is_dir():
            raise LauncherError("Локальная тема не найдена")
        shutil.rmtree(target)

    def export_archive(self, theme_id: str) -> tuple[str, bytes]:
        theme = self._read_theme(self._existing_directory(theme_id))
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("theme.json", json.dumps(_theme_document(theme), ensure_ascii=False, indent=2) + "\n")
            archive.writestr("theme.yml", _metadata_yaml(theme))
        content = output.getvalue()
        if len(content) > THEME_MAX_ARCHIVE_BYTES:
            raise LauncherError("ZIP темы превышает ограничение размера")
        return f"{theme['id']}-{theme['version']}.zip", content

    def _persist(self, theme: dict[str, Any], *, replace: bool) -> dict[str, Any]:
        target = self.root / theme["id"]
        if target.is_symlink():
            raise LauncherError("Каталог темы является символьной ссылкой")
        exists = target.exists()
        if exists and not target.is_dir():
            raise LauncherError("Путь локальной темы занят не каталогом")
        if exists and not replace:
            raise LauncherError(f"Тема «{theme['id']}» уже сохранена")

        staging = Path(tempfile.mkdtemp(prefix=".theme-install-", dir=self.root))
        backup: Path | None = None
        try:
            _atomic_json_write(staging / "theme.json", _theme_document(theme), private=True)
            _write_private_text(staging / "theme.yml", _metadata_yaml(theme))
            _atomic_json_write(
                staging / "origin.json",
                {"source": theme["source"], "source_url": theme["source_url"]},
                private=True,
            )
            if exists:
                backup = self.root / f".theme-backup-{uuid.uuid4().hex}"
                target.rename(backup)
            try:
                staging.rename(target)
            except OSError:
                if backup is not None and backup.exists():
                    backup.rename(target)
                    backup = None
                raise
            if backup is not None:
                shutil.rmtree(backup)
                backup = None
        except OSError as exc:
            raise LauncherError(f"Не удалось сохранить тему: {exc}") from exc
        finally:
            if staging.exists():
                shutil.rmtree(staging, ignore_errors=True)
            if backup is not None and backup.exists():
                shutil.rmtree(backup, ignore_errors=True)
        return self._read_theme(target)

    def _existing_directory(self, theme_id: str) -> Path:
        if not _THEME_ID_RE.fullmatch(str(theme_id)):
            raise LauncherError("Некорректный id темы")
        directory = self.root / theme_id
        if directory.is_symlink() or not directory.is_dir():
            raise LauncherError("Локальная тема не найдена")
        return directory

    def _read_theme(self, directory: Path) -> dict[str, Any]:
        theme_path = directory / "theme.json"
        metadata_path = directory / "theme.yml"
        for path in (theme_path, metadata_path):
            if path.is_symlink() or not path.is_file():
                raise LauncherError("Пакет локальной темы повреждён")
        if theme_path.stat().st_size > THEME_MAX_JSON_BYTES or metadata_path.stat().st_size > THEME_MAX_METADATA_BYTES:
            raise LauncherError("Пакет локальной темы превышает ограничение размера")
        try:
            raw_theme = json.loads(theme_path.read_text(encoding="utf-8"))
            metadata = parse_theme_yml(metadata_path.read_bytes())
        except (OSError, UnicodeError, ValueError) as exc:
            raise LauncherError("Не удалось прочитать локальную тему") from exc
        origin_path = directory / "origin.json"
        origin = {"source": "local", "source_url": ""}
        if origin_path.exists():
            if origin_path.is_symlink() or not origin_path.is_file() or origin_path.stat().st_size > 4096:
                raise LauncherError("Метаданные источника темы повреждены")
            try:
                stored_origin = json.loads(origin_path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, ValueError) as exc:
                raise LauncherError("Метаданные источника темы повреждены") from exc
            if isinstance(stored_origin, dict) and stored_origin.get("source") in {"local", "github"}:
                origin = stored_origin
        theme = normalize_theme(raw_theme, metadata, source=origin["source"], source_url=str(origin.get("source_url") or ""))
        if theme["id"] != directory.name:
            raise LauncherError("id темы не совпадает с каталогом")
        return theme

def _write_private_text(path: Path, content: str) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(content)
        handle.flush()
        os.fsync(handle.fileno())

def _validate_https_url(url: str, allowed_hosts: frozenset[str]) -> str:
    try:
        parsed = urlsplit(url)
        port = parsed.port
    except ValueError as exc:
        raise LauncherError("Некорректный URL GitHub") from exc
    host = (parsed.hostname or "").lower().rstrip(".")
    if parsed.scheme.lower() != "https" or host not in allowed_hosts or parsed.username or parsed.password or port not in {None, 443}:
        raise LauncherError("Разрешены только HTTPS-адреса GitHub")
    return host

class GitHubThemeCatalog:
    def __init__(self, http: HttpClient, store: ThemeStore) -> None:
        self.http = http
        self.store = store

    def search(self, query: str = "", *, page: int = 1) -> list[dict[str, Any]]:
        query = " ".join(str(query or "").split())
        if len(query) > 120 or not _SEARCH_RE.fullmatch(query):
            raise LauncherError("Поисковый запрос содержит недопустимые символы или слишком длинный")
        try:
            page = int(page)
        except (TypeError, ValueError) as exc:
            raise LauncherError("Некорректный номер страницы") from exc
        if page < 1 or page > 10:
            raise LauncherError("Номер страницы должен быть от 1 до 10")
        terms = [f"topic:{THEME_GITHUB_TOPIC}", "is:public"]
        if query:
            terms.append(query)
        url = f"{GITHUB_API_URL}/search/repositories?{urlencode({'q': ' '.join(terms), 'sort': 'updated', 'order': 'desc', 'page': page, 'per_page': 30})}"
        response = self._api_json(url)
        items = response.get("items") if isinstance(response, dict) else None
        if not isinstance(items, list):
            raise LauncherError("GitHub вернул некорректный список тем")
        result: list[dict[str, Any]] = []
        for item in items[:30]:
            if not isinstance(item, dict) or item.get("private") is True:
                continue
            topics = item.get("topics")
            if isinstance(topics, list) and THEME_GITHUB_TOPIC not in topics:
                continue
            owner = item.get("owner") if isinstance(item.get("owner"), dict) else {}
            owner_name = str(owner.get("login") or "")
            repo = str(item.get("name") or "")
            if not _OWNER_RE.fullmatch(owner_name) or not _REPO_RE.fullmatch(repo):
                continue
            result.append(
                {
                    "owner": owner_name,
                    "repo": repo,
                    "full_name": f"{owner_name}/{repo}",
                    "name": str(item.get("name") or repo)[:100],
                    "description": str(item.get("description") or "")[:500],
                    "url": f"https://github.com/{owner_name}/{repo}",
                    "stars": _bounded_int(item.get("stargazers_count", 0), 0, 2_000_000_000),
                    "updated_at": str(item.get("updated_at") or "")[:40],
                    "topics": [str(topic)[:100] for topic in topics if isinstance(topic, str)] if isinstance(topics, list) else [],
                }
            )
        return result

    def install(self, owner: str, repo: str, *, replace: bool = False) -> tuple[dict[str, Any], dict[str, Any]]:
        _validate_repository_name(owner, repo)
        repo_path = f"{owner}/{repo}"
        repository = self._api_json(f"{GITHUB_API_URL}/repos/{repo_path}")
        if not isinstance(repository, dict) or repository.get("private") is True or str(repository.get("visibility", "public")).lower() != "public":
            raise LauncherError("Репозиторий с темой должен быть публичным")
        topics = repository.get("topics")
        if not isinstance(topics, list):
            topic_data = self._api_json(f"{GITHUB_API_URL}/repos/{repo_path}/topics")
            topics = topic_data.get("names", []) if isinstance(topic_data, dict) else []
        if THEME_GITHUB_TOPIC not in {str(topic).lower() for topic in topics if isinstance(topic, str)}:
            raise LauncherError("В репозитории отсутствует тема GitHub topic trial-launcher-theme")

        release = self._api_json(f"{GITHUB_API_URL}/repos/{repo_path}/releases/latest")
        if not isinstance(release, dict) or release.get("draft") is True or release.get("prerelease") is True:
            raise LauncherError("У темы нет опубликованного стабильного GitHub Release")
        tag = str(release.get("tag_name") or "").strip()
        if not tag or len(tag) > 128:
            raise LauncherError("GitHub Release не содержит корректный тег версии")
        asset = _select_theme_asset(release.get("assets"), repo, tag)
        asset_id = _bounded_int(asset.get("id"), 1, 2_147_483_647)
        asset_size = _bounded_int(asset.get("size"), 1, THEME_MAX_ARCHIVE_BYTES)
        asset_url = f"{GITHUB_API_URL}/repos/{repo_path}/releases/assets/{asset_id}"
        content = self._request(asset_url, max_bytes=THEME_MAX_ARCHIVE_BYTES, asset=True)
        if len(content) != asset_size:
            raise LauncherError("Размер ZIP темы не совпадает с данными GitHub")

        source_url = f"https://github.com/{owner}/{repo}"
        theme = self.store.preview_archive(content, source="github", source_url=source_url)
        if theme["id"].lower() != repo.lower():
            raise LauncherError("id в theme.json должен совпадать с именем репозитория GitHub")
        release_version = tag[1:] if tag[:1].lower() == "v" else tag
        if theme["version"] != release_version:
            raise LauncherError("Версия в theme.yml должна совпадать с тегом GitHub Release")
        installed = self.store._persist(theme, replace=replace)
        metadata = {
            "full_name": f"{owner}/{repo}",
            "version": theme["version"],
            "size": len(content),
            "sha256": hashlib.sha256(content).hexdigest(),
            "url": source_url,
        }
        return installed, metadata

    def _api_json(self, url: str) -> Any:
        raw = self._request(url, max_bytes=4 * 1024 * 1024, asset=False)
        try:
            return json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, ValueError) as exc:
            raise LauncherError("GitHub вернул некорректный JSON") from exc

    def _request(self, url: str, *, max_bytes: int, asset: bool) -> bytes:
        allowed_hosts = _ASSET_HOSTS if asset else _API_HOSTS
        _validate_https_url(url, allowed_hosts)

        def validate_redirect(redirect_url: str) -> None:
            _validate_https_url(redirect_url, allowed_hosts)

        headers = {
            "Accept": "application/octet-stream" if asset else "application/vnd.github+json",
            "X-GitHub-Api-Version": GITHUB_API_VERSION,
        }
        content = self.http.request(url, headers=headers, max_bytes=max_bytes, redirect_validator=validate_redirect)
        if not isinstance(content, (bytes, bytearray)) or len(content) > max_bytes:
            raise LauncherError("GitHub вернул слишком большой или некорректный ответ")
        return bytes(content)

def _validate_repository_name(owner: str, repo: str) -> None:
    if not _OWNER_RE.fullmatch(str(owner)) or not _REPO_RE.fullmatch(str(repo)) or repo in {".", ".."}:
        raise LauncherError("Некорректный адрес GitHub-репозитория")

def _select_theme_asset(value: Any, repo: str, tag: str) -> dict[str, Any]:
    if not isinstance(value, list):
        raise LauncherError("GitHub Release не содержит ZIP темы")
    assets = [
        asset
        for asset in value
        if isinstance(asset, dict)
        and str(asset.get("name") or "").lower().endswith(".zip")
        and not asset.get("state") == "starter"
    ]
    expected = {f"{repo}-{tag}.zip".lower(), f"{repo}-v{tag.lstrip('vV')}.zip".lower()}
    preferred = [asset for asset in assets if str(asset.get("name") or "").lower() in expected]
    candidates = preferred or assets
    if len(candidates) != 1:
        raise LauncherError("В GitHub Release должен быть один ZIP темы с однозначным именем")
    return candidates[0]

def _bounded_int(value: Any, minimum: int, maximum: int) -> int:
    if isinstance(value, bool):
        raise LauncherError("GitHub вернул некорректное числовое значение")
    try:
        number = int(value)
    except (TypeError, ValueError) as exc:
        raise LauncherError("GitHub вернул некорректное числовое значение") from exc
    if number < minimum or number > maximum:
        raise LauncherError("GitHub вернул числовое значение вне допустимого диапазона")
    return number
