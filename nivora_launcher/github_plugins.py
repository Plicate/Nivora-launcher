from __future__ import annotations

import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import zipfile
from typing import Any, Protocol
from urllib.error import HTTPError
from urllib.parse import quote, unquote, urlencode, urlsplit

from .errors import LauncherError
from .net import HttpClient
from .plugins import (
    PLUGIN_MAX_ARCHIVE_BYTES,
    PLUGIN_MAX_FILES,
    PLUGIN_MAX_MANIFEST_BYTES,
    PLUGIN_MAX_UNPACKED_BYTES,
    PLUGIN_MANIFEST,
    PLUGIN_V2_MANIFEST,
    _parse_v2_manifest_bytes,
    _verify_unsigned_v2_integrity,
    parse_plugin_yml,
    validate_manifest,
    validate_plugin_json_manifest,
    validate_v2_manifest,
    verify_manifest,
)

GITHUB_PLUGIN_TOPIC = "trial-launcher-plugin"
GITHUB_API_URL = "https://api.github.com"
GITHUB_DEVICE_CODE_URL = "https://github.com/login/device/code"
GITHUB_ACCESS_TOKEN_URL = "https://github.com/login/oauth/access_token"
GITHUB_CLIENT_ID_ENV = "TRIAL_LAUNCHER_GITHUB_CLIENT_ID"
GITHUB_OAUTH_SCOPE = "public_repo"
GITHUB_API_VERSION = "2022-11-28"
MAX_LICENSE_BYTES = 1024 * 1024
MAX_GITHUB_JSON_BYTES = 4 * 1024 * 1024

_API_HOSTS = frozenset({"api.github.com"})
_OAUTH_HOSTS = frozenset({"github.com"})
_UPLOAD_HOSTS = frozenset({"uploads.github.com"})
_ASSET_HOSTS = frozenset(
    {
        "api.github.com",
        "github.com",
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
    }
)
_ASSET_REDIRECTS = {
    "api.github.com": frozenset(
        {
            "api.github.com",
            "github.com",
            "objects.githubusercontent.com",
            "release-assets.githubusercontent.com",
        }
    ),
    "github.com": frozenset(
        {"github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"}
    ),
    "objects.githubusercontent.com": frozenset({"objects.githubusercontent.com"}),
    "release-assets.githubusercontent.com": frozenset({"release-assets.githubusercontent.com"}),
}
_OWNER_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$")
_REPO_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
_SEARCH_QUERY_RE = re.compile(r"^[A-Za-z0-9._+ -]{0,120}$")
_RELEASE_VERSION_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$")

class GitHubPluginSetupError(LauncherError):
    """GitHub OAuth or token-provider setup is missing."""

class TokenProvider(Protocol):
    """The host application owns secure token storage; this service does not."""

    def get_token(self) -> str | None: ...

    def set_token(self, token: str) -> None: ...

class SecretToolTokenProvider:
    """Stores the GitHub token in the platform's secure keyring."""

    _SERVICE = "trial-launcher-github"
    _ATTRIBUTES = ("service", _SERVICE, "purpose", "plugin-publishing")

    @staticmethod
    def available() -> bool:
        if os.name != "nt":
            return shutil.which("secret-tool") is not None
        try:
            return _windows_keyring().get_keyring().priority > 0
        except Exception:
            return False

    def get_token(self) -> str | None:
        if os.name == "nt":
            try:
                return _windows_keyring().get_password(self._SERVICE, "plugin-publishing") or None
            except Exception as exc:
                raise GitHubPluginSetupError("GitHub token storage is unavailable in Windows Credential Manager.") from exc
        result = self._run(["lookup", *self._ATTRIBUTES], allow_missing=True)
        token = result.stdout.strip()
        return token or None

    def set_token(self, token: str) -> None:
        if not _valid_bearer_token(token):
            raise GitHubPluginSetupError("GitHub returned an invalid access token.")
        self.clear_token()
        if os.name == "nt":
            try:
                _windows_keyring().set_password(self._SERVICE, "plugin-publishing", token)
            except Exception as exc:
                raise GitHubPluginSetupError("GitHub token storage is unavailable in Windows Credential Manager.") from exc
            return
        self._run(
            ["store", "--label", "Nivora Launcher GitHub", *self._ATTRIBUTES],
            input_text=token,
        )

    def clear_token(self) -> None:
        if os.name == "nt":
            try:
                keyring = _windows_keyring()
                if keyring.get_password(self._SERVICE, "plugin-publishing") is not None:
                    keyring.delete_password(self._SERVICE, "plugin-publishing")
            except Exception as exc:
                raise GitHubPluginSetupError("GitHub token storage is unavailable in Windows Credential Manager.") from exc
            return
        self._run(["clear", *self._ATTRIBUTES], allow_missing=True)

    @staticmethod
    def _run(arguments: list[str], *, input_text: str | None = None, allow_missing: bool = False):
        command = shutil.which("secret-tool")
        if not command:
            raise GitHubPluginSetupError("GitHub sign-in needs Secret Service and the secret-tool command.")
        try:
            result = subprocess.run(
                [command, *arguments],
                input=input_text,
                capture_output=True,
                text=True,
                check=False,
                timeout=10,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise GitHubPluginSetupError("GitHub token storage is unavailable in the system keyring.") from exc
        if result.returncode != 0 and not (allow_missing and result.returncode == 1):
            raise GitHubPluginSetupError("GitHub token storage could not access the system keyring.")
        return result

def _windows_keyring():
    try:
        import keyring
        if keyring.get_keyring().priority <= 0:
            raise RuntimeError("no active keyring backend")
        return keyring
    except Exception as exc:
        raise GitHubPluginSetupError("Windows Credential Manager is unavailable.") from exc

class GitHubPluginService:
    def __init__(
        self,
        http: HttpClient | None = None,
        token_provider: TokenProvider | None = None,
        *,
        client_id: str | None = None,
    ) -> None:
        self.http = http if http is not None else HttpClient()
        self.token_provider = token_provider
        configured_client_id = client_id if client_id is not None else os.getenv(GITHUB_CLIENT_ID_ENV, "")
        self.client_id = str(configured_client_id).strip()

    def status(self) -> dict[str, bool]:
        provider = self.token_provider
        storage_available = provider is not None
        if provider is not None and hasattr(provider, "available"):
            storage_available = bool(provider.available())
        authorized = bool(provider.get_token()) if provider is not None and storage_available else False
        return {
            "client_configured": bool(self.client_id),
            "authorized": authorized,
            "secure_storage_available": storage_available,
        }

    def search(self, query: str = "", *, page: int = 1, per_page: int = 30) -> list[dict[str, Any]]:
        query = " ".join(str(query or "").split())
        if len(query) > 120 or not _SEARCH_QUERY_RE.fullmatch(query):
            raise LauncherError("GitHub plugin search text contains unsupported characters or is too long.")
        page = _bounded_int(page, 1, 10, "page")
        per_page = _bounded_int(per_page, 1, 100, "per_page")
        terms = [f"topic:{GITHUB_PLUGIN_TOPIC}", "is:public"]
        if query:
            terms.append(query)
        params = urlencode(
            {
                "q": " ".join(terms),
                "sort": "updated",
                "order": "desc",
                "page": page,
                "per_page": per_page,
            }
        )
        response, _ = self._api_json(f"/search/repositories?{params}")
        if not isinstance(response, dict) or not isinstance(response.get("items"), list):
            raise LauncherError("GitHub returned an invalid plugin search response.")

        result: list[dict[str, Any]] = []
        for item in response["items"][:per_page]:
            if not isinstance(item, dict) or item.get("private") is True:
                continue
            visibility = str(item.get("visibility", "")).lower()
            if visibility and visibility != "public":
                continue
            topics = item.get("topics")
            if isinstance(topics, list) and GITHUB_PLUGIN_TOPIC not in topics:
                continue
            try:
                result.append(_repository_summary(item, topics=topics))
            except LauncherError:
                continue
        return result

    def list_plugins(self, *, page: int = 1, per_page: int = 30) -> list[dict[str, Any]]:
        return self.search(page=page, per_page=per_page)

    def details(self, owner: str, repo: str) -> dict[str, Any]:
        context = self._load_catalog_release(owner, repo)
        download = self._verified_download_metadata(context)
        repository = context["repository"]
        release = context["release"]
        manifest = download["manifest"]
        return {
            **_repository_summary(repository, topics=context["topics"]),
            "manifest": manifest,
            "latest_release": {
                "tag_name": context["tag_name"],
                "name": str(release.get("name") or "")[:200],
                "published_at": str(release.get("published_at") or ""),
                "url": _release_url(context["owner"], context["repo"], context["tag_name"]),
            },
            "download": {key: value for key, value in download.items() if key != "manifest"},
        }

    def download_metadata(self, owner: str, repo: str) -> dict[str, Any]:
        return self._verified_download_metadata(self._load_catalog_release(owner, repo))

    def download_plugin_archive(self, owner: str, repo: str) -> tuple[dict[str, Any], bytes]:
        return self._verified_download_asset(self._load_catalog_release(owner, repo))

    def start_oauth_device_flow(self) -> dict[str, Any]:
        client_id = self._require_client_id()
        response, _ = self._form_json(
            GITHUB_DEVICE_CODE_URL,
            {"client_id": client_id, "scope": GITHUB_OAUTH_SCOPE},
            allowed_hosts=_OAUTH_HOSTS,
        )
        if not isinstance(response, dict):
            raise LauncherError("GitHub returned an invalid device authorization response.")
        device_code = _required_text(response.get("device_code"), "device_code", maximum=512)
        user_code = _required_text(response.get("user_code"), "user_code", maximum=64)
        if any(char.isspace() or ord(char) < 32 for char in device_code):
            raise LauncherError("GitHub returned an invalid device authorization code.")
        if any(char.isspace() or ord(char) < 32 for char in user_code):
            raise LauncherError("GitHub returned an invalid user code.")
        verification_uri = _verification_uri(response.get("verification_uri"))
        expires_in = _bounded_int(response.get("expires_in"), 1, 86400, "expires_in")
        interval = _bounded_int(response.get("interval", 5), 1, 60, "interval")
        return {
            "device_code": device_code,
            "user_code": user_code,
            "verification_uri": verification_uri,
            "expires_in": expires_in,
            "interval": interval,
        }

    def poll_oauth_device_flow(self, device_code: str) -> dict[str, Any]:
        client_id = self._require_client_id()
        provider = self._require_token_provider(require_get=False)
        device_code = _required_text(device_code, "device_code", maximum=512)
        if any(char.isspace() or ord(char) < 32 for char in device_code):
            raise LauncherError("The GitHub device code is invalid.")
        response, _ = self._form_json(
            GITHUB_ACCESS_TOKEN_URL,
            {
                "client_id": client_id,
                "device_code": device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            },
            allowed_hosts=_OAUTH_HOSTS,
            allowed_statuses=(400,),
        )
        if not isinstance(response, dict):
            raise LauncherError("GitHub returned an invalid OAuth response.")
        oauth_error = str(response.get("error", ""))
        if oauth_error == "authorization_pending":
            return {"status": "pending"}
        if oauth_error == "slow_down":
            interval = _bounded_int(response.get("interval", 5), 1, 60, "interval")
            return {"status": "slow_down", "interval": interval + 5}
        if oauth_error == "access_denied":
            return {"status": "denied"}
        if oauth_error == "expired_token":
            return {"status": "expired"}
        if oauth_error:
            raise LauncherError("GitHub device authorization failed.")

        access_token = _required_text(response.get("access_token"), "access_token", maximum=4096)
        if not _valid_bearer_token(access_token):
            raise LauncherError("GitHub returned an invalid access token.")
        try:
            provider.set_token(access_token)
        except Exception:
            raise LauncherError("GitHub authorization could not be saved by the configured token provider.") from None
        return {"status": "authorized"}

    def publish_plugin(self, archive: bytes, *, description: str | None = None) -> dict[str, Any]:
        token = self._get_access_token()
        manifest, digest = _validate_plugin_archive(archive)
        version = manifest["version"]
        if not _RELEASE_VERSION_RE.fullmatch(version):
            raise LauncherError("Plugin version must use letters, numbers, dots, underscores, plus signs, or hyphens.")

        repo_name = manifest["id"]
        repo_description = manifest["description"] if description is None else str(description)
        repo_description = repo_description.strip()[:350]
        created, _ = self._api_json(
            "/user/repos",
            method="POST",
            body={
                "name": repo_name,
                "description": repo_description,
                "private": False,
                "auto_init": True,
                "has_issues": False,
                "has_projects": False,
                "has_wiki": False,
            },
            token=token,
        )
        if (
            not isinstance(created, dict)
            or created.get("private") is not False
            or str(created.get("visibility", "public")).lower() != "public"
        ):
            raise LauncherError("GitHub did not create a public plugin repository.")
        owner_data = created.get("owner") if isinstance(created.get("owner"), dict) else {}
        owner = str(owner_data.get("login") or "")
        repo = str(created.get("name") or "")
        _validate_repository_name(owner, repo)
        if repo.lower() != repo_name.lower():
            raise LauncherError("GitHub created a repository with an unexpected name.")

        repo_path = _repository_path(owner, repo)
        topic_response, _ = self._api_json(
            f"/repos/{repo_path}/topics",
            method="PUT",
            body={"names": [GITHUB_PLUGIN_TOPIC]},
            token=token,
        )
        names = topic_response.get("names") if isinstance(topic_response, dict) else None
        if not isinstance(names, list) or GITHUB_PLUGIN_TOPIC not in names:
            raise LauncherError("GitHub did not apply the plugin catalog topic.")

        tag_name = f"v{version}"
        release, _ = self._api_json(
            f"/repos/{repo_path}/releases",
            method="POST",
            body={
                "tag_name": tag_name,
                "target_commitish": str(created.get("default_branch") or "main"),
                "name": f"{manifest['name']} {version}"[:200],
                "body": repo_description,
                "draft": False,
                "prerelease": False,
            },
            token=token,
        )
        if not isinstance(release, dict):
            raise LauncherError("GitHub returned an invalid release response.")
        if release.get("tag_name") != tag_name or release.get("draft") is True or release.get("prerelease") is True:
            raise LauncherError("GitHub did not create the expected published release.")
        release_id = _positive_int(release.get("id"), "release id")
        asset_name = f"{manifest['id']}-{tag_name}.zip"
        upload_url = _release_upload_url(release.get("upload_url"), owner, repo, release_id)
        upload_url = f"{upload_url}?{urlencode({'name': asset_name})}"
        uploaded_raw, _ = self._request_raw(
            upload_url,
            method="POST",
            data=archive,
            headers=self._github_headers(token=token)
            | {"Content-Type": "application/zip", "Accept": "application/vnd.github+json"},
            max_bytes=MAX_GITHUB_JSON_BYTES,
            allowed_hosts=_UPLOAD_HOSTS,
        )
        uploaded = _parse_json_object(uploaded_raw, "release asset")
        reported_size = _optional_int(uploaded.get("size"))
        if reported_size is not None and reported_size != len(archive):
            raise LauncherError("GitHub reported an unexpected release asset size.")
        reported_digest = str(uploaded.get("digest") or "")
        if reported_digest.startswith("sha256:") and reported_digest[7:].lower() != digest:
            raise LauncherError("GitHub reported a different SHA-256 for the uploaded plugin.")

        download_url = _asset_download_url(owner, repo, tag_name, asset_name)
        reported_url = uploaded.get("browser_download_url")
        if reported_url:
            _validate_reported_download_url(reported_url, download_url)
        result = {
            "owner": owner,
            "repo": repo,
            "full_name": f"{owner}/{repo}",
            "repository_url": f"https://github.com/{quote(owner, safe='')}/{quote(repo, safe='')}",
            "release_url": _release_url(owner, repo, tag_name),
            "download_url": download_url,
            "tag_name": tag_name,
            "file_name": asset_name,
            "size": len(archive),
            "sha256": digest,
            "manifest": manifest,
        }
        del token
        return result

    def _load_catalog_release(self, owner: str, repo: str) -> dict[str, Any]:
        _validate_repository_name(owner, repo)
        requested_path = _repository_path(owner, repo)
        repository, _ = self._api_json(f"/repos/{requested_path}")
        if not isinstance(repository, dict) or repository.get("private") is True:
            raise LauncherError("This GitHub repository is not public.")
        visibility = str(repository.get("visibility", "public")).lower()
        if visibility != "public":
            raise LauncherError("This GitHub repository is not public.")
        actual_owner, actual_repo = _repository_identity(repository)
        _validate_repository_name(actual_owner, actual_repo)
        topics = repository.get("topics")
        if not isinstance(topics, list):
            topic_response, _ = self._api_json(f"/repos/{_repository_path(actual_owner, actual_repo)}/topics")
            topics = topic_response.get("names", []) if isinstance(topic_response, dict) else []
        normalized_topics = [str(topic).lower() for topic in topics if isinstance(topic, str)]
        if GITHUB_PLUGIN_TOPIC not in normalized_topics:
            raise LauncherError("This repository is not tagged as a Nivora Launcher plugin.")

        repo_path = _repository_path(actual_owner, actual_repo)
        release, _ = self._api_json(f"/repos/{repo_path}/releases/latest")
        if not isinstance(release, dict) or release.get("draft") is True or release.get("prerelease") is True:
            raise LauncherError("This plugin has no published latest release.")
        tag_name = _required_text(release.get("tag_name"), "release tag", maximum=128)
        asset = _select_zip_asset(release.get("assets"))
        return {
            "repository": repository,
            "owner": actual_owner,
            "repo": actual_repo,
            "topics": normalized_topics,
            "release": release,
            "tag_name": tag_name,
            "asset": asset,
        }

    def _verified_download_metadata(self, context: dict[str, Any]) -> dict[str, Any]:
        return self._verified_download_asset(context)[0]

    def _verified_download_asset(self, context: dict[str, Any]) -> tuple[dict[str, Any], bytes]:
        owner = context["owner"]
        repo = context["repo"]
        asset = context["asset"]
        asset_id = _positive_int(asset["id"], "release asset id")
        asset_url = f"{GITHUB_API_URL}/repos/{_repository_path(owner, repo)}/releases/assets/{asset_id}"
        content, _ = self._request_raw(
            asset_url,
            headers={
                "Accept": "application/octet-stream",
                "X-GitHub-Api-Version": GITHUB_API_VERSION,
            },
            max_bytes=PLUGIN_MAX_ARCHIVE_BYTES,
            allowed_hosts=_ASSET_HOSTS,
            redirect_transitions=_ASSET_REDIRECTS,
        )
        if not content:
            raise LauncherError("The latest plugin release ZIP is empty.")
        if len(content) != asset["size"]:
            raise LauncherError("The latest plugin release ZIP size does not match GitHub metadata.")
        manifest, digest = _validate_plugin_archive(content)
        download_url = _asset_download_url(owner, repo, context["tag_name"], asset["name"])
        reported_url = asset.get("browser_download_url")
        if reported_url:
            _validate_reported_download_url(reported_url, download_url)
        return (
            {
                "owner": owner,
                "repo": repo,
                "full_name": f"{owner}/{repo}",
                "tag_name": context["tag_name"],
                "version": manifest["version"],
                "file_name": asset["name"],
                "download_url": download_url,
                "size": len(content),
                "sha256": digest,
                "manifest": manifest,
            },
            content,
        )

    def _api_json(
        self,
        path: str,
        *,
        method: str = "GET",
        body: dict[str, Any] | None = None,
        token: str | None = None,
        headers: dict[str, str] | None = None,
        allowed_statuses: tuple[int, ...] = (),
    ) -> tuple[Any, int]:
        url = f"{GITHUB_API_URL}{path}"
        request_headers = self._github_headers(token=token) | (headers or {})
        data = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf-8") if body is not None else None
        if data is not None:
            request_headers.setdefault("Content-Type", "application/json")
        raw, status = self._request_raw(
            url,
            method=method,
            data=data,
            headers=request_headers,
            max_bytes=MAX_GITHUB_JSON_BYTES,
            allowed_hosts=_API_HOSTS,
            allowed_statuses=allowed_statuses,
        )
        return _parse_json(raw, "GitHub"), status

    def _form_json(
        self,
        url: str,
        values: dict[str, str],
        *,
        allowed_hosts: frozenset[str],
        allowed_statuses: tuple[int, ...] = (),
    ) -> tuple[Any, int]:
        raw, status = self._request_raw(
            url,
            method="POST",
            data=urlencode(values).encode("utf-8"),
            headers={"Accept": "application/json", "Content-Type": "application/x-www-form-urlencoded"},
            max_bytes=MAX_GITHUB_JSON_BYTES,
            allowed_hosts=allowed_hosts,
            allowed_statuses=allowed_statuses,
        )
        return _parse_json(raw, "GitHub OAuth"), status

    def _request_raw(
        self,
        url: str,
        *,
        method: str = "GET",
        data: bytes | None = None,
        headers: dict[str, str] | None = None,
        max_bytes: int,
        allowed_hosts: frozenset[str],
        allowed_statuses: tuple[int, ...] = (),
        redirect_transitions: dict[str, frozenset[str]] | None = None,
    ) -> tuple[bytes, int]:
        initial_host = _validate_https_url(url, allowed_hosts)
        validator = _redirect_validator(initial_host, allowed_hosts, redirect_transitions)
        try:
            raw = self.http.request(
                url,
                method=method,
                data=data,
                headers=headers,
                max_bytes=max_bytes,
                redirect_validator=validator,
            )
            status = 200
        except (LauncherError, HTTPError) as exc:
            http_error = _find_http_error(exc)
            if http_error is not None and http_error.code in allowed_statuses:
                try:
                    raw = http_error.read(max_bytes + 1)
                finally:
                    http_error.close()
                status = http_error.code
            elif http_error is not None and not isinstance(exc, LauncherError):
                raise LauncherError(f"GitHub returned HTTP {http_error.code}.") from None
            else:
                raise
        if not isinstance(raw, (bytes, bytearray)):
            raise LauncherError("GitHub returned a non-binary response.")
        if len(raw) > max_bytes:
            raise LauncherError("GitHub response is too large.")
        return bytes(raw), status

    @staticmethod
    def _github_headers(*, token: str | None = None) -> dict[str, str]:
        headers = {
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": GITHUB_API_VERSION,
        }
        if token is not None:
            headers["Authorization"] = f"Bearer {token}"
        return headers

    def _require_client_id(self) -> str:
        if not self.client_id:
            raise GitHubPluginSetupError(
                f"GitHub OAuth is not configured. Set {GITHUB_CLIENT_ID_ENV} or pass client_id to GitHubPluginService."
            )
        return self.client_id

    def _require_token_provider(self, *, require_get: bool) -> TokenProvider:
        provider = self.token_provider
        required = ("set_token", "get_token") if require_get else ("set_token",)
        if provider is None or any(not callable(getattr(provider, name, None)) for name in required):
            raise GitHubPluginSetupError(
                "Configure a secure GitHub token provider with the required get_token/set_token methods."
            )
        return provider

    def _get_access_token(self) -> str:
        provider = self._require_token_provider(require_get=True)
        try:
            token = provider.get_token()
        except Exception:
            raise LauncherError("Could not retrieve GitHub authorization from the configured token provider.") from None
        if not isinstance(token, str) or not token or len(token) > 4096 or token != token.strip():
            raise GitHubPluginSetupError("Sign in to GitHub before publishing a plugin.")
        if not _valid_bearer_token(token):
            raise GitHubPluginSetupError("The configured GitHub authorization is invalid.")
        return token

def _validate_plugin_archive(content: bytes) -> tuple[dict[str, Any], str]:
    if not isinstance(content, bytes) or not content or len(content) > PLUGIN_MAX_ARCHIVE_BYTES:
        raise LauncherError("Plugin ZIP is empty or exceeds the archive size limit.")
    try:
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            members = archive.infolist()
            if not members or len(members) > PLUGIN_MAX_FILES:
                raise LauncherError("Plugin ZIP has no files or contains too many entries.")
            files: dict[str, zipfile.ZipInfo] = {}
            seen: set[str] = set()
            total_size = 0
            for member in members:
                name = _normalized_archive_name(member.filename, is_dir=member.is_dir())
                if name in seen:
                    raise LauncherError("Plugin ZIP contains duplicate paths.")
                seen.add(name)
                mode = (member.external_attr >> 16) & 0o170000
                if mode == 0o120000:
                    raise LauncherError("Plugin ZIP may not contain symbolic links.")
                if member.flag_bits & 0x1:
                    raise LauncherError("Encrypted files are not allowed in a plugin ZIP.")
                if member.is_dir():
                    if mode not in {0, 0o040000}:
                        raise LauncherError("Plugin ZIP contains a non-directory entry.")
                    continue
                if mode not in {0, 0o100000}:
                    raise LauncherError("Plugin ZIP contains a non-regular file.")
                total_size += member.file_size
                if member.file_size < 0 or member.file_size > PLUGIN_MAX_UNPACKED_BYTES or total_size > PLUGIN_MAX_UNPACKED_BYTES:
                    raise LauncherError("Plugin ZIP expands beyond the unpacked size limit.")
                files[name] = member

            for name in files:
                parts = name.split("/")
                if any("/".join(parts[:index]) in files for index in range(1, len(parts))):
                    raise LauncherError("Plugin ZIP contains a file path used as a directory.")

            legacy_manifest_info = files.get(PLUGIN_MANIFEST)
            v2_manifest_info = files.get(PLUGIN_V2_MANIFEST)
            if (legacy_manifest_info is None) == (v2_manifest_info is None):
                raise LauncherError("Plugin ZIP must contain exactly one root plugin.yml or plugin.json.")
            license_info = files.get("LICENSE")
            if license_info is None or not 0 < license_info.file_size <= MAX_LICENSE_BYTES:
                raise LauncherError("Plugin ZIP must contain a nonempty root LICENSE of at most 1 MiB.")
            if legacy_manifest_info is not None:
                manifest_text = _read_zip_member(
                    archive, legacy_manifest_info, PLUGIN_MAX_MANIFEST_BYTES
                ).decode("utf-8")
                manifest = validate_manifest(parse_plugin_yml(manifest_text))
                license_content = _read_zip_member(archive, license_info, MAX_LICENSE_BYTES)
            else:
                assert v2_manifest_info is not None
                raw_manifest = _parse_v2_manifest_bytes(
                    _read_zip_member(archive, v2_manifest_info, PLUGIN_MAX_MANIFEST_BYTES)
                )
                manifest = validate_plugin_json_manifest(raw_manifest)
                package_files = {
                    name: _read_zip_member(archive, member, PLUGIN_MAX_UNPACKED_BYTES)
                    for name, member in files.items()
                    if name != PLUGIN_V2_MANIFEST
                }
                license_content = package_files["LICENSE"]
                if "signature" in raw_manifest:
                    verification = verify_manifest(raw_manifest, package_files)
                    if not isinstance(verification, dict) or verification.get("valid") is not True:
                        reason = verification.get("error") if isinstance(verification, dict) else None
                        raise LauncherError(
                            f"Plugin ZIP signature or integrity verification failed: {reason or 'invalid package'}."
                        )
                else:
                    _verify_unsigned_v2_integrity(raw_manifest, package_files)
            if not license_content.strip():
                raise LauncherError("Plugin ZIP root LICENSE must not be empty.")
            if manifest["entrypoint"] not in files:
                raise LauncherError("Plugin ZIP does not contain the manifest entrypoint.")
    except LauncherError:
        raise
    except Exception as exc:
        raise LauncherError("Plugin ZIP is invalid or contains unreadable files.") from exc
    return manifest, hashlib.sha256(content).hexdigest()

def _normalized_archive_name(name: str, *, is_dir: bool) -> str:
    if not isinstance(name, str) or not name or "\x00" in name:
        raise LauncherError("Plugin ZIP contains an invalid file path.")
    normalized = name.replace("\\", "/")
    if normalized.startswith("/") or re.match(r"^[A-Za-z]:", normalized):
        raise LauncherError("Plugin ZIP contains an absolute file path.")
    if is_dir and normalized.endswith("/"):
        normalized = normalized[:-1]
    parts = normalized.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        raise LauncherError("Plugin ZIP contains an unsafe file path.")
    return "/".join(parts)

def _read_zip_member(archive: zipfile.ZipFile, member: zipfile.ZipInfo, limit: int) -> bytes:
    with archive.open(member, "r") as source:
        content = source.read(limit + 1)
    if len(content) > limit or len(content) != member.file_size:
        raise LauncherError("A required plugin ZIP file exceeds its size limit.")
    return content

def _select_zip_asset(assets: Any) -> dict[str, Any]:
    if not isinstance(assets, list):
        raise LauncherError("The latest plugin release has no ZIP asset.")
    for asset in assets:
        if not isinstance(asset, dict):
            continue
        name = asset.get("name")
        if not isinstance(name, str) or not name.lower().endswith(".zip"):
            continue
        if not name or len(name) > 200 or "/" in name or "\\" in name or any(ord(char) < 32 for char in name):
            continue
        size = _optional_int(asset.get("size"))
        if size is None or size < 1 or size > PLUGIN_MAX_ARCHIVE_BYTES:
            continue
        try:
            asset_id = _positive_int(asset.get("id"), "release asset id")
        except LauncherError:
            continue
        return {**asset, "id": asset_id, "name": name, "size": size}
    raise LauncherError("The latest plugin release must contain a bounded ZIP asset.")

def _repository_summary(repository: dict[str, Any], *, topics: Any = None) -> dict[str, Any]:
    owner, repo = _repository_identity(repository)
    topic_values = topics if isinstance(topics, list) else repository.get("topics", [])
    if not isinstance(topic_values, list):
        topic_values = []
    normalized_topics = [str(topic).lower() for topic in topic_values if isinstance(topic, str)]
    if GITHUB_PLUGIN_TOPIC not in normalized_topics:
        normalized_topics.append(GITHUB_PLUGIN_TOPIC)
    return {
        "owner": owner,
        "repo": repo,
        "full_name": f"{owner}/{repo}",
        "name": str(repository.get("name") or repo)[:100],
        "description": str(repository.get("description") or "")[:500],
        "repository_url": f"https://github.com/{quote(owner, safe='')}/{quote(repo, safe='')}",
        "stars": _optional_int(repository.get("stargazers_count")) or 0,
        "updated_at": str(repository.get("updated_at") or ""),
        "topics": normalized_topics,
    }

def _repository_identity(repository: dict[str, Any]) -> tuple[str, str]:
    owner_data = repository.get("owner") if isinstance(repository.get("owner"), dict) else {}
    owner = str(owner_data.get("login") or "")
    repo = str(repository.get("name") or "")
    _validate_repository_name(owner, repo)
    return owner, repo

def _repository_path(owner: str, repo: str) -> str:
    _validate_repository_name(owner, repo)
    return f"{quote(owner, safe='')}/{quote(repo, safe='')}"

def _validate_repository_name(owner: Any, repo: Any) -> None:
    if not isinstance(owner, str) or not _OWNER_RE.fullmatch(owner):
        raise LauncherError("GitHub repository owner is invalid.")
    if not isinstance(repo, str) or repo in {".", ".."} or not _REPO_RE.fullmatch(repo):
        raise LauncherError("GitHub repository name is invalid.")

def _asset_download_url(owner: str, repo: str, tag: str, name: str) -> str:
    path = "/".join(
        quote(part, safe="") for part in (owner, repo, "releases", "download", tag, name)
    )
    return f"https://github.com/{path}"

def _release_url(owner: str, repo: str, tag: str) -> str:
    return f"https://github.com/{quote(owner, safe='')}/{quote(repo, safe='')}/releases/tag/{quote(tag, safe='')}"

def _release_upload_url(value: Any, owner: str, repo: str, release_id: int) -> str:
    if not isinstance(value, str):
        raise LauncherError("GitHub returned an invalid release upload URL.")
    template = "{?name,label}"
    clean_url = value[:-len(template)] if value.endswith(template) else value
    try:
        host = _validate_https_url(clean_url, _UPLOAD_HOSTS)
    except LauncherError:
        raise LauncherError("GitHub returned an unsafe release upload URL.") from None
    parsed = urlsplit(clean_url)
    expected_path = f"/repos/{_repository_path(owner, repo)}/releases/{release_id}/assets"
    if host != "uploads.github.com" or parsed.query or parsed.fragment or parsed.path != expected_path:
        raise LauncherError("GitHub returned an unsafe release upload URL.")
    return f"https://uploads.github.com{expected_path}"

def _validate_reported_download_url(value: Any, expected: str) -> None:
    if not isinstance(value, str):
        raise LauncherError("GitHub returned an invalid release download URL.")
    _validate_https_url(value, frozenset({"github.com"}))
    parsed = urlsplit(value)
    expected_parsed = urlsplit(expected)
    if parsed.query or parsed.fragment or unquote(parsed.path) != unquote(expected_parsed.path):
        raise LauncherError("GitHub returned an unexpected release download URL.")

def _verification_uri(value: Any) -> str:
    if not isinstance(value, str):
        raise LauncherError("GitHub returned an invalid device verification URL.")
    _validate_https_url(value, _OAUTH_HOSTS)
    parsed = urlsplit(value)
    if parsed.path.rstrip("/") != "/login/device" or parsed.query or parsed.fragment:
        raise LauncherError("GitHub returned an unexpected device verification URL.")
    return "https://github.com/login/device"

def _validate_https_url(url: str, allowed_hosts: frozenset[str]) -> str:
    try:
        parsed = urlsplit(url)
        host = (parsed.hostname or "").lower().rstrip(".")
        port = parsed.port
    except (TypeError, ValueError) as exc:
        raise LauncherError("A GitHub URL is malformed.") from exc
    if (
        parsed.scheme.lower() != "https"
        or not host
        or parsed.username
        or parsed.password
        or parsed.fragment
        or port not in (None, 443)
        or host not in allowed_hosts
    ):
        raise LauncherError("Only HTTPS GitHub service URLs are allowed.")
    return host

def _redirect_validator(
    initial_host: str,
    allowed_hosts: frozenset[str],
    transitions: dict[str, frozenset[str]] | None,
):
    current_host = initial_host
    redirect_count = 0
    allowed_transitions = transitions or {initial_host: frozenset({initial_host})}

    def validate(target: str) -> None:
        nonlocal current_host, redirect_count
        target_host = _validate_https_url(target, allowed_hosts)
        redirect_count += 1
        if redirect_count > 4 or target_host not in allowed_transitions.get(current_host, frozenset({current_host})):
            raise LauncherError("GitHub returned an unsafe or excessive redirect.")
        current_host = target_host

    return validate

def _parse_json(raw: bytes, source: str) -> Any:
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError):
        raise LauncherError(f"{source} returned invalid JSON.") from None

def _parse_json_object(raw: bytes, source: str) -> dict[str, Any]:
    value = _parse_json(raw, source)
    if not isinstance(value, dict):
        raise LauncherError(f"{source} response must be an object.")
    return value

def _find_http_error(error: BaseException) -> HTTPError | None:
    current: BaseException | None = error
    seen: set[int] = set()
    while current is not None and id(current) not in seen:
        seen.add(id(current))
        if isinstance(current, HTTPError):
            return current
        current = current.__cause__ or current.__context__
    return None

def _required_text(value: Any, field: str, *, maximum: int) -> str:
    if not isinstance(value, str) or not value or len(value) > maximum:
        raise LauncherError(f"GitHub returned an invalid {field}.")
    return value

def _bounded_int(value: Any, minimum: int, maximum: int, field: str) -> int:
    try:
        result = int(value)
    except (TypeError, ValueError, OverflowError):
        raise LauncherError(f"GitHub returned an invalid {field} value.") from None
    return min(maximum, max(minimum, result))

def _positive_int(value: Any, field: str) -> int:
    try:
        result = int(value)
    except (TypeError, ValueError, OverflowError):
        raise LauncherError(f"GitHub returned an invalid {field}.") from None
    if result <= 0:
        raise LauncherError(f"GitHub returned an invalid {field}.")
    return result

def _optional_int(value: Any) -> int | None:
    try:
        return max(0, int(value)) if value is not None else None
    except (TypeError, ValueError, OverflowError):
        return None

def _valid_bearer_token(value: str) -> bool:
    return bool(value) and all(33 <= ord(char) <= 126 for char in value)
