from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.parse import parse_qs, unquote, urlsplit

from nivora_launcher.errors import LauncherError
from nivora_launcher.mods import ModrinthProvider, PROJECT_METADATA_SUFFIX


API_URL = "https://api.modrinth.com/v2"
GAME_VERSION = "1.20.1"
LOADER = "fabric"


class FakeHttp:
    def __init__(self, projects: dict[str, dict] | None = None, *, fail_url: str = "") -> None:
        self.projects = projects or {}
        self.fail_url = fail_url
        self.json_urls: list[str] = []
        self.downloads: list[tuple[str, Path, dict]] = []
        self.version_projects = {
            str(version.get("id")): project_id
            for project_id, project in self.projects.items()
            for version in project.get("versions", [])
            if version.get("id")
        }

    def get_json(self, url: str):
        self.json_urls.append(url)
        path = urlsplit(url).path.removeprefix("/v2/")
        parts = path.split("/")
        if parts[0] == "search":
            return {"hits": []}
        if parts[0] == "project" and len(parts) >= 2:
            project_id = unquote(parts[1])
            project = self.projects.get(project_id, {})
            if len(parts) >= 3 and parts[2] == "version":
                return project.get("versions", [])
            return project.get("details", {})
        if parts[0] == "version" and len(parts) >= 2:
            version_id = unquote(parts[1])
            return {"project_id": self.version_projects.get(version_id, "")}
        return {}

    def download(self, url: str, destination: Path, **kwargs) -> bool:
        self.downloads.append((url, destination, kwargs))
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(b"partial" if url == self.fail_url else _PAYLOADS.get(url, b"downloaded"))
        if url == self.fail_url:
            raise LauncherError("simulated dependency download failure")
        return True


_PAYLOADS: dict[str, bytes] = {}


def _version(
    project_id: str,
    version_id: str,
    *,
    number: str = "1.0",
    dependencies: list[dict] | None = None,
    game_versions: list[str] | None = None,
    loaders: list[str] | None = None,
) -> dict:
    url = f"https://cdn.example.invalid/{project_id}-{version_id}.jar"
    payload = f"{project_id}:{version_id}".encode()
    _PAYLOADS[url] = payload
    return {
        "id": version_id,
        "project_id": project_id,
        "name": f"{project_id} {number}",
        "version_number": number,
        "game_versions": game_versions if game_versions is not None else [GAME_VERSION],
        "loaders": loaders if loaders is not None else [LOADER],
        "dependencies": dependencies or [],
        "files": [
            {
                "filename": f"{project_id}-{number}.jar",
                "url": url,
                "size": len(payload),
                "primary": True,
                "hashes": {"sha256": hashlib.sha256(payload).hexdigest()},
            }
        ],
    }


def _project(project_id: str, versions: list[dict], *, project_type: str = "mod") -> dict:
    return {
        "details": {"id": project_id, "title": project_id.title(), "project_type": project_type},
        "versions": versions,
    }


def _provider(http: FakeHttp, *, catalog_id: str = "modrinth") -> ModrinthProvider:
    return ModrinthProvider(http, API_URL, catalog_id=catalog_id)


def _write_installed_file(
    directory: Path,
    project_id: str,
    version_id: str,
    *,
    version: str = "1.0",
    trusted: bool = True,
) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / f"{project_id}.jar"
    target.write_bytes(b"installed")
    metadata = {
        "source": "Modrinth",
        "catalog": "modrinth",
        "catalog_id": "modrinth",
        "catalog_url": API_URL,
        "project_id": project_id,
        "project_type": "mod",
        "name": project_id.title(),
        "display_name": project_id.title(),
        "version": version,
        "game_version": GAME_VERSION,
        "loader": LOADER,
        "dependencies": [],
    }
    if trusted:
        metadata["version_id"] = version_id
    sidecar = target.with_name(f".{target.name}{PROJECT_METADATA_SUFFIX}")
    sidecar.write_text(json.dumps(metadata), encoding="utf-8")


class ModCatalogFeatureTests(unittest.TestCase):
    def setUp(self) -> None:
        _PAYLOADS.clear()

    def test_search_validates_ordering_and_encodes_modrinth_index(self) -> None:
        http = FakeHttp()
        provider = _provider(http)

        for ordering in ("relevance", "downloads", "newest", "updated"):
            provider.search("sodium", GAME_VERSION, LOADER, 12, sort_by=ordering)
            params = parse_qs(urlsplit(http.json_urls[-1]).query)
            self.assertEqual(params["index"], [ordering])
            self.assertEqual(json.loads(params["facets"][0]), [
                ["project_type:mod"],
                [f"versions:{GAME_VERSION}"],
                [f"categories:{LOADER}"],
            ])

        request_count = len(http.json_urls)
        with self.assertRaisesRegex(LauncherError, "порядок сортировки"):
            provider.search("sodium", sort_by="popular")
        self.assertEqual(len(http.json_urls), request_count)

    def test_project_details_and_compatible_version_selection_are_type_safe(self) -> None:
        compatible = _version("sample", "fabric-v1")
        incompatible = _version(
            "sample",
            "forge-v2",
            number="2.0",
            game_versions=["1.21.4"],
            loaders=["forge"],
        )
        http = FakeHttp(
            {
                "sample": _project("sample", [incompatible, compatible]),
                "pack": _project("pack", [], project_type="modpack"),
            }
        )
        provider = _provider(http)

        details = provider.project_details("sample")
        versions = provider.compatible_versions("sample", GAME_VERSION, LOADER)
        selected = provider.select_compatible_version(
            "sample", GAME_VERSION, LOADER, version_id="fabric-v1"
        )

        self.assertEqual(details["project_type"], "mod")
        self.assertEqual([item["id"] for item in versions], ["fabric-v1"])
        self.assertEqual(selected["id"], "fabric-v1")
        with self.assertRaisesRegex(LauncherError, "несовместима"):
            provider.select_compatible_version("sample", GAME_VERSION, LOADER, version_id="forge-v2")
        with self.assertRaisesRegex(LauncherError, "имеет тип modpack"):
            provider.project_details("pack", "mod")

    def test_install_resolves_required_but_not_optional_dependencies_and_records_exact_metadata(self) -> None:
        root_version = _version(
            "root",
            "root-v1",
            dependencies=[
                {"project_id": "library", "version_id": "library-v3", "dependency_type": "required"},
                {"project_id": "optional", "dependency_type": "optional"},
            ],
        )
        library_version = _version("library", "library-v3", number="3.0")
        optional_version = _version("optional", "optional-v1")
        http = FakeHttp(
            {
                "root": _project("root", [root_version]),
                "library": _project("library", [library_version]),
                "optional": _project("optional", [optional_version]),
            }
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            result = _provider(http).install("root", instance, GAME_VERSION, LOADER)
            root_path = result.path
            self.assertIsNotNone(root_path)
            assert root_path is not None
            metadata = json.loads(
                root_path.with_name(f".{root_path.name}{PROJECT_METADATA_SUFFIX}").read_text(encoding="utf-8")
            )

            self.assertEqual(result.version_id, "root-v1")
            self.assertEqual(result.dependencies, [{
                "project_id": "library",
                "version_id": "library-v3",
                "dependency_type": "required",
            }])
            self.assertEqual(metadata["version_id"], "root-v1")
            self.assertEqual(metadata["source"], "Modrinth")
            self.assertEqual(metadata["catalog"], "modrinth")
            self.assertEqual(metadata["catalog_url"], API_URL)
            self.assertEqual(metadata["game_version"], GAME_VERSION)
            self.assertEqual(metadata["loader"], LOADER)
            self.assertEqual(metadata["dependencies"], result.dependencies)
            self.assertEqual(len(http.downloads), 2)
            self.assertFalse(any("optional" in url for url, _, _ in http.downloads))
            self.assertEqual(
                {path.name for path in (instance / "mods").iterdir() if path.is_file()},
                {
                    "root-1.0.jar",
                    ".root-1.0.jar" + PROJECT_METADATA_SUFFIX,
                    "library-3.0.jar",
                    ".library-3.0.jar" + PROJECT_METADATA_SUFFIX,
                },
            )

    def test_install_rejects_an_already_installed_root_project_but_update_can_replace_it(self) -> None:
        first = _version("managed", "managed-v1", number="1.0")
        second = _version("managed", "managed-v2", number="2.0")
        http = FakeHttp({"managed": _project("managed", [second, first])})
        provider = _provider(http)

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            installed = provider.install("managed", instance, GAME_VERSION, LOADER)
            original = installed.path.read_bytes()
            download_count = len(http.downloads)

            with self.assertRaisesRegex(LauncherError, "уже установлен"):
                provider.install("managed", instance, GAME_VERSION, LOADER)

            self.assertEqual(len(http.downloads), download_count)
            self.assertEqual(installed.path.read_bytes(), original)

            updated = provider.install("managed", instance, GAME_VERSION, LOADER, allow_replace=True)
            self.assertEqual(updated.version_id, "managed-v2")
            self.assertEqual(updated.path.read_bytes(), b"managed:managed-v2")

    def test_dependency_graph_deduplicates_shared_projects_and_stops_cycles(self) -> None:
        root = _version(
            "root",
            "root-v1",
            dependencies=[
                {"project_id": "first", "dependency_type": "required"},
                {"project_id": "second", "dependency_type": "required"},
            ],
        )
        first = _version(
            "first",
            "first-v1",
            dependencies=[
                {"project_id": "shared", "dependency_type": "required"},
                {"project_id": "root", "dependency_type": "required"},
            ],
        )
        second = _version(
            "second",
            "second-v1",
            dependencies=[
                {"project_id": "shared", "dependency_type": "required"},
                {"project_id": "shared", "dependency_type": "required"},
            ],
        )
        shared = _version("shared", "shared-v1", dependencies=[{"project_id": "first", "dependency_type": "required"}])
        http = FakeHttp(
            {
                "root": _project("root", [root]),
                "first": _project("first", [first]),
                "second": _project("second", [second]),
                "shared": _project("shared", [shared]),
            }
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            _provider(http).install("root", instance, GAME_VERSION, LOADER)

            self.assertEqual(len(http.downloads), 4)
            self.assertEqual(
                {url.rsplit("/", 1)[-1].split("-")[0] for url, _, _ in http.downloads},
                {"root", "first", "second", "shared"},
            )
            root_metadata = json.loads(
                (instance / "mods" / f".root-1.0.jar{PROJECT_METADATA_SUFFIX}").read_text(encoding="utf-8")
            )
            second_metadata = json.loads(
                (instance / "mods" / f".second-1.0.jar{PROJECT_METADATA_SUFFIX}").read_text(encoding="utf-8")
            )
            self.assertEqual([item["project_id"] for item in root_metadata["dependencies"]], ["first", "second"])
            self.assertEqual([item["project_id"] for item in second_metadata["dependencies"]], ["shared"])

    def test_dependency_project_limit_stops_before_any_download(self) -> None:
        root = _version("root", "root-v1", dependencies=[{"project_id": "child", "dependency_type": "required"}])
        child = _version("child", "child-v1", dependencies=[{"project_id": "leaf", "dependency_type": "required"}])
        leaf = _version("leaf", "leaf-v1")
        http = FakeHttp({
            "root": _project("root", [root]),
            "child": _project("child", [child]),
            "leaf": _project("leaf", [leaf]),
        })

        with tempfile.TemporaryDirectory() as temp_dir, patch("nivora_launcher.mods.MAX_MOD_DEPENDENCY_PROJECTS", 2):
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            with self.assertRaisesRegex(LauncherError, "слишком много проектов"):
                _provider(http).install("root", instance, GAME_VERSION, LOADER)
            self.assertEqual(http.downloads, [])
            self.assertEqual(list(instance.iterdir()), [])

    def test_dependency_download_failure_leaves_no_partial_install(self) -> None:
        root = _version("root", "root-v1", dependencies=[{"project_id": "library", "dependency_type": "required"}])
        library = _version("library", "library-v1")
        failed_url = library["files"][0]["url"]
        http = FakeHttp(
            {"root": _project("root", [root]), "library": _project("library", [library])},
            fail_url=failed_url,
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            with self.assertRaisesRegex(LauncherError, "simulated dependency"):
                _provider(http).install("root", instance, GAME_VERSION, LOADER)

            self.assertFalse((instance / "mods").exists())
            self.assertEqual(list(instance.iterdir()), [])
            self.assertEqual(len(http.downloads), 2)

    def test_update_check_returns_only_compatible_updates_and_skips_manual_files(self) -> None:
        current = _version("managed", "managed-v1", number="1.0")
        latest = _version("managed", "managed-v2", number="2.0")
        http = FakeHttp({"managed": _project("managed", [latest, current])})

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            mods = instance / "mods"
            mods.mkdir(parents=True)
            _write_installed_file(mods, "managed", "managed-v1")
            _write_installed_file(mods, "manual", "manual-v1", trusted=False)
            (mods / "manual-file.jar").write_bytes(b"manual")

            updates = _provider(http).check_updates(instance, GAME_VERSION, LOADER)

            self.assertEqual(len(updates), 1)
            self.assertEqual(updates[0]["project_id"], "managed")
            self.assertEqual(updates[0]["current_version_id"], "managed-v1")
            self.assertEqual(updates[0]["version_id"], "managed-v2")

    def test_update_metadata_is_scoped_to_its_catalog(self) -> None:
        current = _version("managed", "managed-v1", number="1.0")
        latest = _version("managed", "managed-v2", number="2.0")
        http = FakeHttp({"managed": _project("managed", [latest, current])})

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            _provider(http, catalog_id="team-mods").install(
                "managed", instance, GAME_VERSION, LOADER, version_id="managed-v1"
            )

            default_updates = _provider(http).check_updates(instance, GAME_VERSION, LOADER)
            team_updates = _provider(http, catalog_id="team-mods").check_updates(instance, GAME_VERSION, LOADER)

        self.assertEqual(default_updates, [])
        self.assertEqual([item["project_id"] for item in team_updates], ["managed"])

    def test_update_check_returns_no_update_when_installed_version_is_current(self) -> None:
        latest = _version("managed", "managed-v2", number="2.0")
        http = FakeHttp({"managed": _project("managed", [latest])})

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            _write_installed_file(instance / "mods", "managed", "managed-v2", version="2.0")
            self.assertEqual(_provider(http).check_updates(instance, GAME_VERSION, LOADER), [])

    def test_update_check_does_not_offer_an_incompatible_candidate(self) -> None:
        incompatible = _version(
            "managed",
            "managed-v2",
            number="2.0",
            game_versions=["1.21.4"],
            loaders=["forge"],
        )
        http = FakeHttp({"managed": _project("managed", [incompatible])})

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            _write_installed_file(instance / "mods", "managed", "managed-v1")
            self.assertEqual(_provider(http).check_updates(instance, GAME_VERSION, LOADER), [])

    def test_install_rejects_a_project_type_mismatch_before_download(self) -> None:
        resource_pack = _version("pack", "pack-v1")
        http = FakeHttp({"pack": _project("pack", [resource_pack], project_type="resourcepack")})

        with tempfile.TemporaryDirectory() as temp_dir:
            instance = Path(temp_dir) / "instance"
            instance.mkdir()
            with self.assertRaisesRegex(LauncherError, "имеет тип resourcepack"):
                _provider(http).install("pack", instance, GAME_VERSION, LOADER, project_type="mod")
            self.assertEqual(http.downloads, [])
            self.assertEqual(list(instance.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
