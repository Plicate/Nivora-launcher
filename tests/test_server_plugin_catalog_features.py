from __future__ import annotations

import copy
import hashlib
import tempfile
import unittest
from collections import deque
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from nivora_launcher.errors import LauncherError
from nivora_launcher.plugin_catalogs import PluginCatalog, installed_plugin_metadata_index, remove_installed_plugin


class FakeHttp:
    def __init__(self, responses: dict[str, object], downloads: dict[str, bytes] | None = None) -> None:
        self.responses = responses
        self.download_bodies = downloads or {}
        self.get_urls: list[str] = []
        self.download_calls: list[tuple[str, Path]] = []
        self.fail_download = False
        self.fail_download_urls: set[str] = set()
        self.before_download = None

    def get_json(self, url: str):
        self.get_urls.append(url)
        for fragment in sorted(self.responses, key=len, reverse=True):
            if fragment not in url:
                continue
            value = self.responses[fragment]
            if isinstance(value, deque):
                value = value.popleft()
            return copy.deepcopy(value)
        raise AssertionError(f"No fake response configured for {url}")

    def download(self, url: str, destination: Path, **_kwargs) -> bool:
        self.download_calls.append((url, destination))
        if self.fail_download or url in self.fail_download_urls:
            raise LauncherError("fake download failed")
        if self.before_download:
            self.before_download(url)
        destination.write_bytes(self.download_bodies.get(url, b"plugin-jar"))
        return True


def catalog(kind: str, catalog_id: str = "test-catalog", api_url: str = "https://catalog.example/api") -> dict[str, str]:
    return {"id": catalog_id, "kind": kind, "name": "Test catalog", "api_url": api_url}


def modrinth_version(
    version_id: str,
    version_number: str,
    body: bytes,
    published: str,
    *,
    dependencies: list[dict[str, object]] | None = None,
    file_name: str = "plugin.jar",
) -> dict[str, object]:
    return {
        "id": version_id,
        "version_number": version_number,
        "date_published": published,
        "game_versions": ["1.21.1"],
        "loaders": ["paper"],
        "files": [
            {
                "primary": True,
                "filename": file_name,
                "url": f"https://cdn.example/{version_id}.jar",
                "size": len(body),
                "hashes": {"sha256": hashlib.sha256(body).hexdigest()},
            }
        ],
        "dependencies": dependencies or [],
    }


class ServerPluginCatalogFeatureTests(unittest.TestCase):
    def test_remove_managed_plugin_deletes_jar_and_metadata_only(self) -> None:
        http = FakeHttp(
            {
                "https://custom.example/plugins?": {
                    "results": [{
                        "project_id": "sample",
                        "name": "Sample",
                        "version": "1.0",
                        "file_name": "sample.jar",
                        "download_url": "https://cdn.example/sample.jar",
                    }]
                }
            },
            {"https://cdn.example/sample.jar": b"catalog-jar"},
        )
        provider = PluginCatalog(http, catalog("custom", api_url="https://custom.example/plugins"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            provider.install("sample", plugins, "1.20.4", "paper", confirm_unverified=True)
            (plugins / "manual.jar").write_bytes(b"manual")

            self.assertTrue(remove_installed_plugin(plugins, "sample.jar"))
            self.assertFalse((plugins / "sample.jar").exists())
            self.assertFalse((plugins / ".sample.jar.trial-plugin.json").exists())
            self.assertTrue((plugins / "manual.jar").exists())
            self.assertFalse(remove_installed_plugin(plugins, "sample.jar"))

    def test_search_validates_sort_and_sends_modrinth_index(self) -> None:
        http = FakeHttp(
            {
                "/search?": {
                    "hits": [
                        {
                            "project_id": "older",
                            "title": "Older",
                            "downloads": 10,
                            "date_created": "2024-01-01T00:00:00Z",
                            "date_modified": "2025-01-01T00:00:00Z",
                        },
                        {
                            "project_id": "popular",
                            "title": "Popular",
                            "downloads": 90,
                            "date_created": "2025-01-01T00:00:00Z",
                            "date_modified": "2024-01-01T00:00:00Z",
                        },
                    ]
                }
            }
        )
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        results = provider.search("sample", "paper", "1.21.1", sort_by="downloads")

        self.assertEqual([item.project_id for item in results], ["popular", "older"])
        self.assertEqual(results[0].as_dict()["created_at"], "2025-01-01T00:00:00Z")
        query = parse_qs(urlsplit(http.get_urls[0]).query)
        self.assertEqual(query["index"], ["downloads"])
        self.assertEqual(query["facets"], ['[["project_type:plugin"],["versions:1.21.1"],["categories:paper"]]'])
        newest = provider.search("sample", "paper", "1.21.1", sort_by="newest")
        updated = provider.search("sample", "paper", "1.21.1", sort_by="updated")
        self.assertEqual([item.project_id for item in newest], ["popular", "older"])
        self.assertEqual([item.project_id for item in updated], ["older", "popular"])
        self.assertEqual(parse_qs(urlsplit(http.get_urls[-2]).query)["index"], ["newest"])
        self.assertEqual(parse_qs(urlsplit(http.get_urls[-1]).query)["index"], ["updated"])
        with self.assertRaises(LauncherError):
            provider.search("sample", "paper", "1.21.1", sort_by="unknown")

    def test_unsupported_custom_sort_keeps_default_or_sorts_available_values(self) -> None:
        rows = [
            {"id": "low", "name": "Low", "downloads": 2},
            {"id": "high", "name": "High", "downloads": 30},
        ]
        http = FakeHttp({"https://custom.example/plugins?": {"results": rows}})
        provider = PluginCatalog(http, catalog("custom", api_url="https://custom.example/plugins"))

        relevance = provider.search("x", "paper", "1.20.4")
        downloads = provider.search("x", "paper", "1.20.4", sort_by="downloads")

        self.assertEqual([item.project_id for item in relevance], ["low", "high"])
        self.assertEqual([item.project_id for item in downloads], ["high", "low"])
        self.assertNotIn("sort", parse_qs(urlsplit(http.get_urls[1]).query))

    def test_spigot_sort_dates_and_hangar_provider_sort(self) -> None:
        spigot_http = FakeHttp(
            {
                "/search/resources/example": [
                    {"id": 1, "name": "Old", "downloads": 4, "releaseDate": 1700000000000, "updateDate": 1700000000000},
                    {"id": 2, "name": "New", "downloads": 4, "releaseDate": 1710000000000, "updateDate": 1710000000000},
                ]
            }
        )
        spigot = PluginCatalog(spigot_http, catalog("spigot", api_url="https://api.spiget.org/v2"))
        spigot_results = spigot.search("example", "spigot", "1.20.4", sort_by="updated")
        self.assertEqual([item.project_id for item in spigot_results], ["2", "1"])
        self.assertIn("2024-03-09", spigot_results[0].updated_at)
        self.assertEqual(parse_qs(urlsplit(spigot_http.get_urls[0]).query)["sort"], ["-updateDate"])

        hangar_http = FakeHttp({"/projects?": {"result": []}})
        hangar = PluginCatalog(hangar_http, catalog("hangar", api_url="https://hangar.example/api/v1"))
        hangar.search("", "paper", "1.20.4", sort_by="downloads")
        self.assertEqual(parse_qs(urlsplit(hangar_http.get_urls[0]).query)["sort"], ["POPULARITY"])

    def test_details_include_dates_and_version_dependencies(self) -> None:
        body = b"plugin-v2"
        version = modrinth_version(
            "ver-2",
            "2.0",
            body,
            "2025-02-01T00:00:00Z",
            dependencies=[{"project_id": "dependency", "dependency_type": "required"}],
        )
        http = FakeHttp(
            {
                "/project/example/version": [version],
                "/project/example": {
                    "id": "example",
                    "project_type": "plugin",
                    "title": "Example plugin",
                    "description": "Details",
                    "date_created": "2024-01-01T00:00:00Z",
                    "date_modified": "2025-01-15T00:00:00Z",
                },
            }
        )
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        details = provider.details("example", "paper", "1.21.1")

        self.assertEqual(details["created_at"], "2024-01-01T00:00:00Z")
        self.assertEqual(details["updated_at"], "2025-01-15T00:00:00Z")
        self.assertEqual(details["version"], "2.0")
        self.assertEqual(details["dependencies"], [{"project_id": "dependency", "dependency_type": "required"}])
        version_query = next(url for url in http.get_urls if "/version?" in url)
        params = parse_qs(urlsplit(version_query).query)
        self.assertEqual(params["game_versions"], ['["1.21.1"]'])
        self.assertEqual(params["loaders"], ['["paper"]'])

    def test_hangar_uses_latest_version_compatible_with_core_and_version(self) -> None:
        http = FakeHttp(
            {
                "/projects/Owner/Example/versions": {
                    "result": [
                        {
                            "name": "2.0",
                            "createdAt": "2025-02-01T00:00:00Z",
                            "platformDependencies": {"PAPER": ["1.20.4"]},
                        },
                        {
                            "name": "1.5",
                            "createdAt": "2025-01-01T00:00:00Z",
                            "platformDependencies": {"PAPER": ["1.21"]},
                            "pluginDependencies": [{"name": "Library", "required": True}],
                        },
                    ]
                }
            },
            {"https://hangar.example/api/v1/projects/Owner/Example/versions/1.5/download": b"hangar-jar"},
        )
        provider = PluginCatalog(http, catalog("hangar", catalog_id="hangar-test", api_url="https://hangar.example/api/v1"))

        with tempfile.TemporaryDirectory() as directory:
            result = provider.install("Owner/Example", Path(directory) / "plugins", "1.21.1", "paper", confirm_unverified=True)

        self.assertEqual(result["version"], "1.5")
        self.assertEqual(result["dependencies"], [{"name": "Library", "required": True}])
        self.assertEqual(len(http.download_calls), 1)

    def test_catalog_install_index_excludes_manual_or_modified_jars(self) -> None:
        http = FakeHttp(
            {
                "https://custom.example/plugins?": {
                    "results": [
                        {
                            "project_id": "sample",
                            "name": "Sample",
                            "version": "1.0",
                            "file_name": "sample.jar",
                            "download_url": "https://cdn.example/sample.jar",
                            "dependencies": [{"project_id": "library", "dependency_type": "required"}],
                        }
                    ]
                },
                "https://custom.example/plugins?query=library": {
                    "results": [
                        {
                            "project_id": "library",
                            "name": "Library",
                            "version": "1.0",
                            "file_name": "library.jar",
                            "download_url": "https://cdn.example/library.jar",
                        }
                    ]
                },
            },
            {
                "https://cdn.example/sample.jar": b"catalog-jar",
                "https://cdn.example/library.jar": b"library-jar",
            },
        )
        provider = PluginCatalog(http, catalog("custom", api_url="https://custom.example/plugins"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            with self.assertRaisesRegex(LauncherError, "unverified-download"):
                provider.install("sample", plugins, "1.20.4", "paper")
            installed = provider.install("sample", plugins, "1.20.4", "paper", confirm_unverified=True)
            (plugins / "manual-upload.jar").write_bytes(b"manual")
            index = installed_plugin_metadata_index(plugins)

            self.assertEqual({entry["file_name"] for entry in index}, {"sample.jar", "library.jar"})
            self.assertEqual(installed["dependencies"], [{"project_id": "library", "dependency_type": "required"}])
            self.assertEqual(len(http.download_calls), 2)
            self.assertNotEqual(http.download_calls[0][1], plugins / "sample.jar")

            (plugins / "sample.jar").write_bytes(b"manually replaced")
            self.assertEqual(
                [entry["project_id"] for entry in installed_plugin_metadata_index(plugins)],
                ["library"],
            )
            with self.assertRaisesRegex(LauncherError, "только JAR"):
                provider.update_installed(plugins, "sample.jar", "paper", "1.20.4", confirm_unverified=True)

    def test_install_includes_required_dependency_at_pinned_modrinth_version(self) -> None:
        root_body = b"root-plugin"
        library_body = b"pinned-library"
        root_version = modrinth_version(
            "root-v1",
            "1.0",
            root_body,
            "2025-01-01T00:00:00Z",
            dependencies=[
                {"project_id": "library", "version_id": "library-v1", "dependency_type": "required"}
            ],
            file_name="root.jar",
        )
        pinned_library = {
            **modrinth_version(
                "library-v1",
                "1.0",
                library_body,
                "2025-01-01T00:00:00Z",
                file_name="library.jar",
            ),
            "project_id": "library",
        }
        http = FakeHttp(
            {
                "/project/root/version": [root_version],
                "/project/root": {"id": "root", "project_type": "plugin", "title": "Root"},
                "/project/library": {"id": "library", "project_type": "plugin", "title": "Library"},
                "/version/library-v1": pinned_library,
            },
            {
                "https://cdn.example/root-v1.jar": root_body,
                "https://cdn.example/library-v1.jar": library_body,
            },
        )
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            installed = provider.install("root", plugins, "1.21.1", "paper")
            entries = installed_plugin_metadata_index(plugins)

        self.assertEqual(installed["project_id"], "root")
        self.assertEqual({entry["project_id"] for entry in entries}, {"root", "library"})
        self.assertIn("https://api.modrinth.com/v2/version/library-v1", http.get_urls)
        self.assertEqual(len(http.download_calls), 2)

    def test_version_only_modrinth_dependency_must_resolve_and_be_compatible(self) -> None:
        root_body = b"root-plugin"
        library_body = b"incompatible-library"
        root_version = modrinth_version(
            "root-v1",
            "1.0",
            root_body,
            "2025-01-01T00:00:00Z",
            dependencies=[{"version_id": "library-v1", "dependency_type": "required"}],
            file_name="root.jar",
        )
        pinned_library = {
            **modrinth_version(
                "library-v1",
                "1.0",
                library_body,
                "2025-01-01T00:00:00Z",
                file_name="library.jar",
            ),
            "project_id": "library",
            "game_versions": ["1.20.4"],
        }
        http = FakeHttp(
            {
                "/project/root/version": [root_version],
                "/project/root": {"id": "root", "project_type": "plugin", "title": "Root"},
                "/project/library": {"id": "library", "project_type": "plugin", "title": "Library"},
                "/version/library-v1": pinned_library,
            },
            {
                "https://cdn.example/root-v1.jar": root_body,
                "https://cdn.example/library-v1.jar": library_body,
            },
        )
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            with self.assertRaisesRegex(LauncherError, "совместимость обязательной зависимости library"):
                provider.install("root", plugins, "1.21.1", "paper")

            self.assertFalse(plugins.exists())
            self.assertEqual(http.download_calls, [])
        self.assertGreaterEqual(sum("/version/library-v1" in url for url in http.get_urls), 2)

    def test_install_omits_optional_and_name_only_dependencies(self) -> None:
        body = b"root-plugin"
        root_version = modrinth_version(
            "root-v1",
            "1.0",
            body,
            "2025-01-01T00:00:00Z",
            dependencies=[
                {"project_id": "optional-library", "dependency_type": "optional"},
                {"project_id": "also-optional", "required": True, "optional": True},
                {"name": "Display Name Only", "required": True},
            ],
            file_name="root.jar",
        )
        http = FakeHttp(
            {
                "/project/root/version": [root_version],
                "/project/root": {"id": "root", "project_type": "plugin", "title": "Root"},
            },
            {"https://cdn.example/root-v1.jar": body},
        )
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            provider.install("root", plugins, "1.21.1", "paper")
            entries = installed_plugin_metadata_index(plugins)

        self.assertEqual([entry["project_id"] for entry in entries], ["root"])
        self.assertEqual(len(http.download_calls), 1)
        self.assertFalse(any("optional-library" in url or "also-optional" in url for url in http.get_urls))

    def test_install_deduplicates_shared_dependencies_and_cycles(self) -> None:
        project_ids = ["root", "branch-a", "branch-b", "shared"]
        bodies = {project_id: f"{project_id}-jar".encode() for project_id in project_ids}
        versions = {
            "root": modrinth_version(
                "root-v1",
                "1.0",
                bodies["root"],
                "2025-01-01T00:00:00Z",
                dependencies=[
                    {"project_id": "branch-a", "dependency_type": "required"},
                    {"project_id": "branch-b", "dependency_type": "required"},
                ],
                file_name="root.jar",
            ),
            "branch-a": modrinth_version(
                "branch-a-v1",
                "1.0",
                bodies["branch-a"],
                "2025-01-01T00:00:00Z",
                dependencies=[{"project_id": "shared", "dependency_type": "required"}],
                file_name="branch-a.jar",
            ),
            "branch-b": modrinth_version(
                "branch-b-v1",
                "1.0",
                bodies["branch-b"],
                "2025-01-01T00:00:00Z",
                dependencies=[{"project_id": "shared", "dependency_type": "required"}],
                file_name="branch-b.jar",
            ),
            "shared": modrinth_version(
                "shared-v1",
                "1.0",
                bodies["shared"],
                "2025-01-01T00:00:00Z",
                dependencies=[{"project_id": "root", "dependency_type": "required"}],
                file_name="shared.jar",
            ),
        }
        responses: dict[str, object] = {}
        downloads: dict[str, bytes] = {}
        for project_id in project_ids:
            responses[f"/project/{project_id}"] = {
                "id": project_id,
                "project_type": "plugin",
                "title": project_id,
            }
            responses[f"/project/{project_id}/version"] = [versions[project_id]]
            downloads[f"https://cdn.example/{versions[project_id]['id']}.jar"] = bodies[project_id]
        http = FakeHttp(responses, downloads)
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            provider.install("root", plugins, "1.21.1", "paper")
            entries = installed_plugin_metadata_index(plugins)

        self.assertEqual({entry["project_id"] for entry in entries}, set(project_ids))
        self.assertEqual(len(entries), len(project_ids))
        self.assertEqual(len(http.download_calls), len(project_ids))
        self.assertEqual(sum("/project/shared/version" in url for url in http.get_urls), 1)

    def test_failed_dependency_download_rolls_back_all_staged_files(self) -> None:
        bodies = {project_id: f"{project_id}-jar".encode() for project_id in ("root", "first", "failing")}
        root_version = modrinth_version(
            "root-v1",
            "1.0",
            bodies["root"],
            "2025-01-01T00:00:00Z",
            dependencies=[
                {"project_id": "first", "dependency_type": "required"},
                {"project_id": "failing", "dependency_type": "required"},
            ],
            file_name="root.jar",
        )
        first_version = modrinth_version("first-v1", "1.0", bodies["first"], "2025-01-01T00:00:00Z", file_name="first.jar")
        failing_version = modrinth_version(
            "failing-v1", "1.0", bodies["failing"], "2025-01-01T00:00:00Z", file_name="failing.jar"
        )
        http = FakeHttp(
            {
                "/project/root/version": [root_version],
                "/project/root": {"id": "root", "project_type": "plugin", "title": "Root"},
                "/project/first/version": [first_version],
                "/project/first": {"id": "first", "project_type": "plugin", "title": "First"},
                "/project/failing/version": [failing_version],
                "/project/failing": {"id": "failing", "project_type": "plugin", "title": "Failing"},
            },
            {
                "https://cdn.example/root-v1.jar": bodies["root"],
                "https://cdn.example/first-v1.jar": bodies["first"],
                "https://cdn.example/failing-v1.jar": bodies["failing"],
            },
        )
        http.fail_download_urls.add("https://cdn.example/failing-v1.jar")
        provider = PluginCatalog(http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            with self.assertRaisesRegex(LauncherError, "fake download failed"):
                provider.install("root", plugins, "1.21.1", "paper")

            self.assertEqual(list(plugins.iterdir()), [])
            self.assertEqual(installed_plugin_metadata_index(plugins), [])
            self.assertEqual(len(http.download_calls), 3)

    def test_modrinth_update_replaces_in_place_only_after_verified_download(self) -> None:
        old_body = b"old-plugin"
        new_body = b"new-plugin"
        version_queue = deque(
            [
                [modrinth_version("v1", "1.0", old_body, "2024-01-01T00:00:00Z", file_name="plugin-v1.jar")],
                [modrinth_version("v2", "2.0", new_body, "2025-01-01T00:00:00Z", file_name="plugin-v2.jar")],
                [modrinth_version("v2", "2.0", new_body, "2025-01-01T00:00:00Z", file_name="plugin-v2.jar")],
            ]
        )
        http = FakeHttp(
            {
                "/project/demo/version": version_queue,
                "/project/demo": {"id": "demo", "project_type": "plugin", "title": "Demo", "date_created": "2023-01-01T00:00:00Z"},
            },
            {
                "https://cdn.example/v1.jar": old_body,
                "https://cdn.example/v2.jar": new_body,
            },
        )
        provider = PluginCatalog(http, catalog("modrinth", catalog_id="modrinth-test", api_url="https://api.modrinth.com/v2"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            installed = provider.install("demo", plugins, "1.21.1", "paper")
            current_path = plugins / installed["file_name"]
            checked = provider.check_updates(plugins, "paper", "1.21.1")
            self.assertEqual(len(checked), 1)
            self.assertEqual(checked[0]["version"], "2.0")
            self.assertEqual(current_path.read_bytes(), old_body)

            updated = provider.update_installed(plugins, current_path.name, "paper", "1.21.1")

            self.assertTrue(updated["updated"])
            self.assertEqual(current_path.read_bytes(), new_body)
            self.assertEqual(provider.installed(plugins)[0]["version"], "2.0")
            self.assertTrue(all(path != current_path for _, path in http.download_calls))

    def test_failed_update_and_unverified_update_leave_existing_jar_unchanged(self) -> None:
        rows = deque(
            [
                {"results": [{"project_id": "sample", "version": "1.0", "file_name": "sample.jar", "download_url": "https://cdn.example/one.jar"}]},
                {"results": [{"project_id": "sample", "version": "2.0", "file_name": "sample.jar", "download_url": "https://cdn.example/two.jar"}]},
            ]
        )
        http = FakeHttp(
            {"https://custom.example/plugins?": rows},
            {"https://cdn.example/one.jar": b"old", "https://cdn.example/two.jar": b"new"},
        )
        provider = PluginCatalog(http, catalog("custom", api_url="https://custom.example/plugins"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            installed = provider.install("sample", plugins, "1.20.4", "paper", confirm_unverified=True)
            path = plugins / installed["file_name"]
            before_calls = len(http.download_calls)
            with self.assertRaisesRegex(LauncherError, "unverified-download"):
                provider.update_installed(plugins, path.name, "paper", "1.20.4")
            self.assertEqual(path.read_bytes(), b"old")
            self.assertEqual(len(http.download_calls), before_calls)

        version_queue = deque(
            [
                [modrinth_version("v1", "1.0", b"old", "2024-01-01T00:00:00Z")],
                [modrinth_version("v2", "2.0", b"new", "2025-01-01T00:00:00Z")],
            ]
        )
        verified_http = FakeHttp(
            {
                "/project/demo/version": version_queue,
                "/project/demo": {"id": "demo", "project_type": "plugin", "title": "Demo"},
            },
            {"https://cdn.example/v1.jar": b"old", "https://cdn.example/v2.jar": b"new"},
        )
        verified_provider = PluginCatalog(verified_http, catalog("modrinth", api_url="https://api.modrinth.com/v2"))
        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            installed = verified_provider.install("demo", plugins, "1.21.1", "paper")
            path = plugins / installed["file_name"]
            verified_http.fail_download = True
            with self.assertRaises(LauncherError):
                verified_provider.update_installed(plugins, path.name, "paper", "1.21.1")
            self.assertEqual(path.read_bytes(), b"old")

    def test_update_does_not_overwrite_a_jar_modified_during_download(self) -> None:
        responses = deque(
            [
                {"results": [{"project_id": "sample", "version": "1.0", "file_name": "sample.jar", "download_url": "https://cdn.example/one.jar"}]},
                {"results": [{"project_id": "sample", "version": "2.0", "file_name": "sample.jar", "download_url": "https://cdn.example/two.jar"}]},
            ]
        )
        http = FakeHttp(
            {"https://custom.example/plugins?": responses},
            {"https://cdn.example/one.jar": b"catalog-old", "https://cdn.example/two.jar": b"catalog-new"},
        )
        provider = PluginCatalog(http, catalog("custom", api_url="https://custom.example/plugins"))

        with tempfile.TemporaryDirectory() as directory:
            plugins = Path(directory) / "plugins"
            installed = provider.install("sample", plugins, "1.20.4", "paper", confirm_unverified=True)
            jar = plugins / installed["file_name"]
            http.before_download = lambda _url: jar.write_bytes(b"manual replacement")

            with self.assertRaisesRegex(LauncherError, "изменился во время загрузки"):
                provider.update_installed(plugins, jar.name, "paper", "1.20.4", confirm_unverified=True)

            self.assertEqual(jar.read_bytes(), b"manual replacement")

    def test_plugin_metadata_index_rejects_symlink_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plugins = root / "real-plugins"
            plugins.mkdir()
            link = root / "plugins-link"
            link.symlink_to(plugins, target_is_directory=True)
            with self.assertRaises(LauncherError):
                installed_plugin_metadata_index(link)

            external_jar = root / "external.jar"
            external_jar.write_bytes(b"manual")
            (plugins / "linked.jar").symlink_to(external_jar)
            self.assertEqual(installed_plugin_metadata_index(plugins), [])


if __name__ == "__main__":
    unittest.main()
