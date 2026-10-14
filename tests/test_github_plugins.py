from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import unittest
import zipfile
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.github_plugins import (
    GITHUB_ACCESS_TOKEN_URL,
    GITHUB_CLIENT_ID_ENV,
    GITHUB_DEVICE_CODE_URL,
    GITHUB_PLUGIN_TOPIC,
    GitHubPluginService,
    GitHubPluginSetupError,
    SecretToolTokenProvider,
    _validate_plugin_archive,
)


SECRET_TOKEN = "gho_test_access_token_do_not_return"
PLUGIN_MANIFEST = "id: sample-plugin\nname: Sample Plugin\nversion: 1.2.3\nentrypoint: main.js\n"
REPO = {
    "name": "sample-plugin",
    "owner": {"login": "octocat"},
    "private": False,
    "visibility": "public",
    "topics": [GITHUB_PLUGIN_TOPIC],
    "description": "A sample plugin",
    "stargazers_count": 7,
    "updated_at": "2026-09-01T12:00:00Z",
}


def _json(value: object) -> bytes:
    return json.dumps(value).encode("utf-8")


def _plugin_zip(
    *,
    manifest: str = PLUGIN_MANIFEST,
    license_content: bytes | None = b"MIT License\n",
    entries: dict[str, bytes] | None = None,
) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("plugin.yml", manifest)
        archive.writestr("main.js", "export default {};\n")
        if license_content is not None:
            archive.writestr("LICENSE", license_content)
        for name, content in (entries or {}).items():
            archive.writestr(name, content)
    return output.getvalue()


def _plugin_json_zip(
    *,
    manifest_updates: dict | None = None,
    entries: dict[str, bytes] | None = None,
) -> bytes:
    package_files = {
        "LICENSE": b"MIT License\n",
        "main.js": b"export default {};\n",
    }
    manifest = {
        "id": "sample-plugin",
        "name": "Sample Plugin",
        "version": "1.2.3",
        "api_version": "2",
        "entrypoint": "main.js",
        "permissions": [],
        "integrity": {
            "files": {
                name: hashlib.sha256(content).hexdigest()
                for name, content in package_files.items()
            }
        },
    }
    manifest.update(manifest_updates or {})
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("plugin.json", json.dumps(manifest))
        for name, content in package_files.items():
            archive.writestr(name, content)
        for name, content in (entries or {}).items():
            archive.writestr(name, content)
    return output.getvalue()


class FakeHttp:
    def __init__(self, handler) -> None:
        self.handler = handler
        self.calls: list[dict] = []

    def request(
        self,
        url: str,
        *,
        method: str = "GET",
        data: bytes | None = None,
        headers: dict[str, str] | None = None,
        max_bytes: int | None = None,
        redirect_validator=None,
    ) -> bytes:
        call = {
            "url": url,
            "method": method,
            "data": data,
            "headers": headers or {},
            "max_bytes": max_bytes,
            "redirect_validator": redirect_validator,
        }
        self.calls.append(call)
        return self.handler(call)


class MemoryTokenProvider:
    def __init__(self, token: str | None = None) -> None:
        self.token = token
        self.set_calls: list[str] = []

    def get_token(self) -> str | None:
        return self.token

    def set_token(self, token: str) -> None:
        self.set_calls.append(token)
        self.token = token


def _catalog_release(content: bytes) -> dict:
    name = "sample-plugin-v1.2.3.zip"
    return {
        "id": 77,
        "tag_name": "v1.2.3",
        "name": "Sample Plugin 1.2.3",
        "published_at": "2026-09-01T12:00:00Z",
        "draft": False,
        "prerelease": False,
        "assets": [
            {
                "id": 90,
                "name": name,
                "size": len(content),
                "browser_download_url": (
                    "https://github.com/octocat/sample-plugin/releases/download/"
                    f"v1.2.3/{name}"
                ),
            }
        ],
    }


class GitHubPluginTests(unittest.TestCase):
    def test_v2_plugin_zip_validates_manifest_and_integrity(self) -> None:
        content = _plugin_json_zip()

        manifest, digest = _validate_plugin_archive(content)

        self.assertEqual(manifest["api_version"], "2")
        self.assertEqual(manifest["id"], "sample-plugin")
        self.assertEqual(digest, hashlib.sha256(content).hexdigest())

    def test_v3_plugin_zip_validates_manifest_and_integrity(self) -> None:
        content = _plugin_json_zip(
            manifest_updates={
                "api_version": "3",
                "permissions": ["runtime:observe", "network:https"],
                "network_hosts": ["example.com"],
                "sandbox_profile": "strict",
                "compatibility_matrix": {"min_launcher_version": "0.1.0"},
            }
        )

        manifest, digest = _validate_plugin_archive(content)

        self.assertEqual(manifest["api_version"], "3")
        self.assertEqual(manifest["id"], "sample-plugin")
        self.assertEqual(digest, hashlib.sha256(content).hexdigest())

    def test_v2_plugin_zip_rejects_bad_schema_integrity_and_conflicting_manifests(self) -> None:
        invalid_hashes = {
            "LICENSE": hashlib.sha256(b"MIT License\n").hexdigest(),
            "main.js": "0" * 64,
        }
        invalid_archives = (
            _plugin_json_zip(manifest_updates={"events": ["unsupported.event"]}),
            _plugin_json_zip(manifest_updates={"integrity": {"files": invalid_hashes}}),
            _plugin_json_zip(entries={"plugin.yml": PLUGIN_MANIFEST.encode("utf-8")}),
        )
        for content in invalid_archives:
            with self.subTest(size=len(content)), self.assertRaises(LauncherError):
                _validate_plugin_archive(content)

    def test_oauth_requires_configured_client_id(self) -> None:
        http = FakeHttp(lambda _call: self.fail("OAuth must not issue a request without a client ID"))
        with patch.dict(os.environ, {}, clear=True):
            service = GitHubPluginService(http=http)
        with self.assertRaisesRegex(GitHubPluginSetupError, "TRIAL_LAUNCHER_GITHUB_CLIENT_ID"):
            service.start_oauth_device_flow()
        self.assertEqual(http.calls, [])

    def test_search_is_topic_and_public_scoped_and_redirect_checked(self) -> None:
        def handler(call: dict) -> bytes:
            self.assertEqual(call["method"], "GET")
            return _json(
                {
                    "items": [
                        REPO,
                        REPO | {"name": "private-plugin", "private": True},
                        REPO | {"name": "wrong-topic", "topics": ["other-topic"]},
                        REPO | {"name": "unlisted-plugin", "topics": None},
                    ]
                }
            )

        http = FakeHttp(handler)
        service = GitHubPluginService(http=http)
        results = service.search("voice chat", page=2, per_page=5)

        self.assertEqual([item["repo"] for item in results], ["sample-plugin", "unlisted-plugin"])
        query = parse_qs(urlsplit(http.calls[0]["url"]).query)
        self.assertIn(f"topic:{GITHUB_PLUGIN_TOPIC}", query["q"][0])
        self.assertIn("is:public", query["q"][0])
        self.assertEqual(query["page"], ["2"])
        self.assertEqual(query["per_page"], ["5"])
        with self.assertRaises(LauncherError):
            http.calls[0]["redirect_validator"]("https://evil.example/redirect")

        listed = service.list_plugins()
        self.assertTrue(listed)
        list_query = parse_qs(urlsplit(http.calls[1]["url"]).query)
        self.assertEqual(list_query["q"], [f"topic:{GITHUB_PLUGIN_TOPIC} is:public"])

    def test_search_rejects_search_qualifier_injection(self) -> None:
        http = FakeHttp(lambda _call: b"{}")
        with self.assertRaisesRegex(LauncherError, "unsupported characters"):
            GitHubPluginService(http=http).search("topic:private")
        self.assertEqual(http.calls, [])

    def test_details_and_download_metadata_validate_release_archive_and_hash(self) -> None:
        content = _plugin_zip()
        release = _catalog_release(content)

        def handler(call: dict) -> bytes:
            path = urlsplit(call["url"]).path
            if path == "/repos/octocat/sample-plugin":
                return _json(REPO)
            if path == "/repos/octocat/sample-plugin/releases/latest":
                return _json(release)
            if path == "/repos/octocat/sample-plugin/releases/assets/90":
                self.assertEqual(call["headers"]["Accept"], "application/octet-stream")
                self.assertLessEqual(call["max_bytes"], 64 * 1024 * 1024)
                return content
            self.fail(f"Unexpected GitHub request: {call['url']}")

        http = FakeHttp(handler)
        service = GitHubPluginService(http=http)
        details = service.details("octocat", "sample-plugin")
        metadata = service.download_metadata("octocat", "sample-plugin")
        install_metadata, archive = service.download_plugin_archive("octocat", "sample-plugin")

        self.assertEqual(details["manifest"]["id"], "sample-plugin")
        self.assertEqual(metadata["sha256"], hashlib.sha256(content).hexdigest())
        self.assertEqual(install_metadata["sha256"], metadata["sha256"])
        self.assertEqual(archive, content)
        self.assertEqual(metadata["size"], len(content))
        self.assertEqual(
            metadata["download_url"],
            "https://github.com/octocat/sample-plugin/releases/download/v1.2.3/sample-plugin-v1.2.3.zip",
        )
        self.assertNotIn("access_token", metadata)
        asset_call = next(call for call in http.calls if call["url"].endswith("/releases/assets/90"))
        validator = asset_call["redirect_validator"]
        validator("https://release-assets.githubusercontent.com/asset")
        with self.assertRaises(LauncherError):
            validator("https://github.com.evil.example/asset")

    def test_status_reports_configuration_and_never_exposes_the_token(self) -> None:
        provider = MemoryTokenProvider("gho_status_test_token")
        status = GitHubPluginService(http=FakeHttp(lambda _call: b"{}"), token_provider=provider, client_id="client-id").status()

        self.assertEqual(status, {"client_configured": True, "authorized": True, "secure_storage_available": True})
        self.assertNotIn("gho_status_test_token", repr(status))

    def test_secret_tool_token_provider_stores_and_reads_without_putting_token_in_arguments(self) -> None:
        token = "gho_keyring_test_token"
        calls = []

        def run(arguments, **kwargs):
            calls.append((arguments, kwargs))
            if "lookup" in arguments:
                return subprocess.CompletedProcess(arguments, 0, stdout=f"{token}\n", stderr="")
            if "clear" in arguments:
                return subprocess.CompletedProcess(arguments, 1, stdout="", stderr="not found")
            return subprocess.CompletedProcess(arguments, 0, stdout="", stderr="")

        with patch("nivora_launcher.github_plugins.shutil.which", return_value="/usr/bin/secret-tool"), patch("nivora_launcher.github_plugins.subprocess.run", side_effect=run):
            provider = SecretToolTokenProvider()
            self.assertIsNone(provider.clear_token())
            self.assertIsNone(provider.set_token(token))
            self.assertEqual(provider.get_token(), token)

        store_call = next((arguments, kwargs) for arguments, kwargs in calls if "store" in arguments)
        self.assertNotIn(token, store_call[0])
        self.assertEqual(store_call[1]["input"], token)

    def test_details_rejects_unmarked_repositories_and_unsafe_names(self) -> None:
        http = FakeHttp(lambda _call: _json(REPO | {"topics": ["not-a-plugin"]}))
        service = GitHubPluginService(http=http)
        with self.assertRaisesRegex(LauncherError, "not tagged"):
            service.details("octocat", "sample-plugin")
        with self.assertRaises(LauncherError):
            service.details("octocat/../../evil", "sample-plugin")
        self.assertEqual(len(http.calls), 1)

    def test_details_rejects_non_github_asset_download_urls(self) -> None:
        content = _plugin_zip()
        release = _catalog_release(content)
        release["assets"][0]["browser_download_url"] = "https://evil.example/plugin.zip"

        def handler(call: dict) -> bytes:
            path = urlsplit(call["url"]).path
            if path == "/repos/octocat/sample-plugin":
                return _json(REPO)
            if path == "/repos/octocat/sample-plugin/releases/latest":
                return _json(release)
            if path == "/repos/octocat/sample-plugin/releases/assets/90":
                return content
            self.fail(f"Unexpected GitHub request: {call['url']}")

        with self.assertRaisesRegex(LauncherError, "HTTPS GitHub"):
            GitHubPluginService(http=FakeHttp(handler)).download_metadata("octocat", "sample-plugin")

    def test_oauth_device_flow_requests_public_repo_and_never_returns_access_token(self) -> None:
        provider = MemoryTokenProvider()
        poll_count = 0

        def handler(call: dict) -> bytes:
            nonlocal poll_count
            values = parse_qs((call["data"] or b"").decode("utf-8"))
            if call["url"] == GITHUB_DEVICE_CODE_URL:
                self.assertEqual(values["scope"], ["public_repo"])
                self.assertEqual(values["client_id"], ["configured-client-id"])
                return _json(
                    {
                        "device_code": "device-code-value",
                        "user_code": "ABCD-EFGH",
                        "verification_uri": "https://github.com/login/device",
                        "expires_in": 900,
                        "interval": 5,
                    }
                )
            self.assertEqual(call["url"], GITHUB_ACCESS_TOKEN_URL)
            self.assertEqual(values["grant_type"], ["urn:ietf:params:oauth:grant-type:device_code"])
            self.assertEqual(values["device_code"], ["device-code-value"])
            poll_count += 1
            if poll_count == 1:
                error = HTTPError(
                    call["url"],
                    400,
                    "Bad Request",
                    {},
                    io.BytesIO(_json({"error": "authorization_pending"})),
                )
                raise LauncherError("HTTP 400") from error
            return _json({"access_token": SECRET_TOKEN, "token_type": "bearer", "scope": "public_repo"})

        http = FakeHttp(handler)
        service = GitHubPluginService(http=http, token_provider=provider, client_id="configured-client-id")
        start = service.start_oauth_device_flow()
        pending = service.poll_oauth_device_flow(start["device_code"])
        authorized = service.poll_oauth_device_flow(start["device_code"])

        self.assertEqual(start["verification_uri"], "https://github.com/login/device")
        self.assertEqual(start["user_code"], "ABCD-EFGH")
        self.assertEqual(pending, {"status": "pending"})
        self.assertEqual(authorized, {"status": "authorized"})
        self.assertNotIn(SECRET_TOKEN, repr(start))
        self.assertNotIn(SECRET_TOKEN, repr(pending))
        self.assertNotIn(SECRET_TOKEN, repr(authorized))
        self.assertEqual(provider.set_calls, [SECRET_TOKEN])

    def test_oauth_rejects_non_github_verification_url(self) -> None:
        http = FakeHttp(
            lambda _call: _json(
                {
                    "device_code": "device-code-value",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://evil.example/login/device",
                    "expires_in": 900,
                    "interval": 5,
                }
            )
        )
        with self.assertRaisesRegex(LauncherError, "HTTPS GitHub"):
            GitHubPluginService(http=http, client_id="client-id").start_oauth_device_flow()

    def test_oauth_reads_client_id_from_environment(self) -> None:
        http = FakeHttp(
            lambda _call: _json(
                {
                    "device_code": "device-code-value",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 5,
                }
            )
        )
        with patch.dict(os.environ, {GITHUB_CLIENT_ID_ENV: "env-client-id"}, clear=False):
            service = GitHubPluginService(http=http)
        service.start_oauth_device_flow()
        values = parse_qs(http.calls[0]["data"].decode("utf-8"))
        self.assertEqual(values["client_id"], ["env-client-id"])

    def test_publish_creates_public_repo_topic_release_and_asset(self) -> None:
        content = _plugin_zip()
        digest = hashlib.sha256(content).hexdigest()
        provider = MemoryTokenProvider(SECRET_TOKEN)

        def handler(call: dict) -> bytes:
            path = urlsplit(call["url"]).path
            if path == "/user/repos":
                body = json.loads(call["data"])
                self.assertEqual(body["name"], "sample-plugin")
                self.assertFalse(body["private"])
                self.assertTrue(body["auto_init"])
                return _json(
                    {
                        "name": "sample-plugin",
                        "owner": {"login": "octocat"},
                        "private": False,
                        "default_branch": "main",
                    }
                )
            if path == "/repos/octocat/sample-plugin/topics":
                self.assertEqual(call["method"], "PUT")
                self.assertEqual(json.loads(call["data"]), {"names": [GITHUB_PLUGIN_TOPIC]})
                return _json({"names": [GITHUB_PLUGIN_TOPIC]})
            if path == "/repos/octocat/sample-plugin/releases":
                body = json.loads(call["data"])
                self.assertEqual(body["tag_name"], "v1.2.3")
                return _json(
                    {
                        "id": 77,
                        "tag_name": "v1.2.3",
                        "upload_url": "https://uploads.github.com/repos/octocat/sample-plugin/releases/77/assets{?name,label}",
                    }
                )
            if urlsplit(call["url"]).hostname == "uploads.github.com":
                self.assertEqual(call["method"], "POST")
                self.assertEqual(call["data"], content)
                self.assertEqual(call["headers"]["Authorization"], f"Bearer {SECRET_TOKEN}")
                self.assertEqual(parse_qs(urlsplit(call["url"]).query)["name"], ["sample-plugin-v1.2.3.zip"])
                return _json(
                    {
                        "size": len(content),
                        "digest": f"sha256:{digest}",
                        "browser_download_url": "https://github.com/octocat/sample-plugin/releases/download/v1.2.3/sample-plugin-v1.2.3.zip",
                    }
                )
            self.fail(f"Unexpected GitHub request: {call['url']}")

        http = FakeHttp(handler)
        service = GitHubPluginService(http=http, token_provider=provider)
        result = service.publish_plugin(content)

        self.assertEqual(result["sha256"], digest)
        self.assertEqual(result["full_name"], "octocat/sample-plugin")
        self.assertEqual(result["download_url"], "https://github.com/octocat/sample-plugin/releases/download/v1.2.3/sample-plugin-v1.2.3.zip")
        self.assertNotIn(SECRET_TOKEN, repr(result))
        self.assertTrue(all(call["headers"].get("Authorization") == f"Bearer {SECRET_TOKEN}" for call in http.calls))
        upload_call = next(call for call in http.calls if urlsplit(call["url"]).hostname == "uploads.github.com")
        with self.assertRaises(LauncherError):
            upload_call["redirect_validator"]("https://evil.example/upload")

    def test_publish_rejects_unsafe_upload_url_before_uploading(self) -> None:
        content = _plugin_zip()
        provider = MemoryTokenProvider(SECRET_TOKEN)

        def handler(call: dict) -> bytes:
            path = urlsplit(call["url"]).path
            if path == "/user/repos":
                return _json(
                    {
                        "name": "sample-plugin",
                        "owner": {"login": "octocat"},
                        "private": False,
                    }
                )
            if path.endswith("/topics"):
                return _json({"names": [GITHUB_PLUGIN_TOPIC]})
            if path.endswith("/releases"):
                return _json(
                    {
                        "id": 77,
                        "tag_name": "v1.2.3",
                        "upload_url": "https://evil.example/upload",
                    }
                )
            self.fail("The service must reject the unsafe upload URL before making an upload request")

        http = FakeHttp(handler)
        with self.assertRaisesRegex(LauncherError, "unsafe release upload URL"):
            GitHubPluginService(http=http, token_provider=provider).publish_plugin(content)
        self.assertFalse(any(urlsplit(call["url"]).hostname == "evil.example" for call in http.calls))

    def test_publish_validates_manifest_license_paths_and_archive_size_before_network(self) -> None:
        provider = MemoryTokenProvider(SECRET_TOKEN)
        invalid_archives = [
            _plugin_zip(license_content=None),
            _plugin_zip(license_content=b"  \n"),
            _plugin_zip(license_content=b"x" * (1024 * 1024 + 1)),
            _plugin_zip(entries={"../escape.txt": b"bad"}),
            _plugin_zip(manifest="id: bad\nname: Bad\nversion: 1\nentrypoint: ../main.js\n"),
        ]
        for archive in invalid_archives:
            with self.subTest(size=len(archive)):
                http = FakeHttp(lambda _call: self.fail("Invalid archives must be rejected before network calls"))
                with self.assertRaises(LauncherError):
                    GitHubPluginService(http=http, token_provider=provider).publish_plugin(archive)
                self.assertEqual(http.calls, [])

    def test_token_provider_errors_and_missing_tokens_are_sanitized(self) -> None:
        class BrokenProvider:
            def get_token(self) -> str:
                raise RuntimeError(SECRET_TOKEN)

            def set_token(self, _token: str) -> None:
                return None

        service = GitHubPluginService(http=FakeHttp(lambda _call: b"{}"), token_provider=BrokenProvider())
        with self.assertRaises(LauncherError) as raised:
            service.publish_plugin(_plugin_zip())
        self.assertNotIn(SECRET_TOKEN, str(raised.exception))
        self.assertNotIn(SECRET_TOKEN, repr(raised.exception))


if __name__ == "__main__":
    unittest.main()
