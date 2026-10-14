from __future__ import annotations

import hashlib
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from nivora_launcher.errors import LauncherError
from nivora_launcher.themes import GitHubThemeCatalog, THEME_GITHUB_TOPIC, ThemeStore


THEME_DOCUMENT = {
    "schema_version": 1,
    "id": "sample-theme",
    "name": "Sample Theme",
    "author": "Ignored from JSON",
    "description": "Ignored from JSON",
    "mode": "dark",
    "accent": "#7aa2ff",
    "accent_strong": "#b7c9ff",
    "background": "#080d16",
    "panel": "#101a29",
    "panel_soft": "#182941",
    "density": "comfortable",
    "sidebar_mode": "adaptive",
}
THEME_YML = (
    'author: "Ada Lovelace"\n'
    'description: "A quiet blue theme"\n'
    'version: "1.0.0"\n'
    'creators:\n'
    '  - "Ada Lovelace"\n'
    '  - "Grace Hopper"\n'
)
REPO = {
    "name": "sample-theme",
    "owner": {"login": "octocat"},
    "private": False,
    "visibility": "public",
    "topics": [THEME_GITHUB_TOPIC],
    "description": "A community theme",
    "stargazers_count": 4,
    "updated_at": "2026-09-01T12:00:00Z",
}


def _theme_zip(*, extra_file: str | None = None) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("theme.json", json.dumps(THEME_DOCUMENT))
        archive.writestr("theme.yml", THEME_YML)
        if extra_file:
            archive.writestr(extra_file, "unexpected")
    return output.getvalue()


class FakeHttp:
    def __init__(self, handler) -> None:
        self.handler = handler
        self.calls: list[dict] = []

    def request(self, url: str, *, headers: dict | None = None, max_bytes: int | None = None, redirect_validator=None) -> bytes:
        call = {"url": url, "headers": headers or {}, "max_bytes": max_bytes, "redirect_validator": redirect_validator}
        self.calls.append(call)
        return self.handler(call)


def _store(directory: str) -> ThemeStore:
    return ThemeStore(SimpleNamespace(paths=SimpleNamespace(data_home=Path(directory))))


class ThemeStoreTests(unittest.TestCase):
    def test_import_export_and_local_save_round_trip(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            imported = store.import_archive(_theme_zip())
            self.assertEqual(imported["source"], "local")
            self.assertEqual(imported["author"], "Ada Lovelace")
            self.assertEqual(imported["creators"], ["Ada Lovelace", "Grace Hopper"])
            self.assertEqual(store.list(), [imported])

            file_name, exported = store.export_archive("sample-theme")
            self.assertEqual(file_name, "sample-theme-1.0.0.zip")
            self.assertEqual(store.preview_archive(exported)["id"], "sample-theme")

    def test_import_rejects_extra_zip_files_and_bad_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            with self.assertRaisesRegex(LauncherError, "ровно theme.json и theme.yml"):
                store.import_archive(_theme_zip(extra_file="extra.txt"))

            bad = io.BytesIO()
            with zipfile.ZipFile(bad, "w") as archive:
                archive.writestr("theme.json", json.dumps(THEME_DOCUMENT))
                archive.writestr("theme.yml", 'author: "A"\ndescription: "B"\nversion: "1.0.0"\ncreators: []\n')
            with self.assertRaisesRegex(LauncherError, "список creators"):
                store.import_archive(bad.getvalue())

    def test_duplicate_theme_requires_explicit_replace(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            store.import_archive(_theme_zip())
            with self.assertRaisesRegex(LauncherError, "уже сохранена"):
                store.import_archive(_theme_zip())
            replaced = store.import_archive(_theme_zip(), replace=True)
            self.assertEqual(replaced["id"], "sample-theme")


class GitHubThemeCatalogTests(unittest.TestCase):
    def test_search_is_public_and_topic_scoped(self) -> None:
        http = FakeHttp(lambda _call: json.dumps({"items": [REPO, REPO | {"name": "private", "private": True}]}).encode())
        results = GitHubThemeCatalog(http, SimpleNamespace()).search("blue theme")

        self.assertEqual([item["repo"] for item in results], ["sample-theme"])
        query = parse_qs(urlsplit(http.calls[0]["url"]).query)["q"][0]
        self.assertIn(f"topic:{THEME_GITHUB_TOPIC}", query)
        self.assertIn("is:public", query)
        with self.assertRaises(LauncherError):
            http.calls[0]["redirect_validator"]("https://evil.example/themes")

    def test_install_checks_release_and_persists_verified_theme(self) -> None:
        content = _theme_zip()
        release = {
            "tag_name": "v1.0.0",
            "draft": False,
            "prerelease": False,
            "assets": [{"id": 91, "name": "sample-theme-v1.0.0.zip", "size": len(content)}],
        }

        def handler(call: dict) -> bytes:
            path = urlsplit(call["url"]).path
            if path == "/repos/octocat/sample-theme":
                return json.dumps(REPO).encode()
            if path == "/repos/octocat/sample-theme/releases/latest":
                return json.dumps(release).encode()
            if path == "/repos/octocat/sample-theme/releases/assets/91":
                self.assertEqual(call["headers"]["Accept"], "application/octet-stream")
                return content
            self.fail(f"Unexpected request: {call['url']}")

        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            catalog = GitHubThemeCatalog(FakeHttp(handler), store)
            theme, metadata = catalog.install("octocat", "sample-theme")
            self.assertEqual(theme["source"], "github")
            self.assertEqual(theme["source_url"], "https://github.com/octocat/sample-theme")
            self.assertEqual(metadata["sha256"], hashlib.sha256(content).hexdigest())
            self.assertEqual(store.list(), [theme])


if __name__ == "__main__":
    unittest.main()
