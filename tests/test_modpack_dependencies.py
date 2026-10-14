from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace

from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import _instance_loader_version
from nivora_launcher.modpacks import ModpackImporter, ModpackProvider


class FakeHttp:
    def __init__(self, *, json_handler=None, payloads: dict[str, bytes] | None = None) -> None:
        self.json_handler = json_handler or (lambda _url: {})
        self.payloads = payloads or {}
        self.downloads: list[str] = []

    def get_json(self, url: str):
        return self.json_handler(url)

    def download(self, url: str, destination: Path, **kwargs) -> bool:
        self.downloads.append(url)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(self.payloads.get(url, b"downloaded"))
        return True


def _write_mrpack(path: Path, dependencies: dict[str, str], files: list[dict] | None = None) -> None:
    index = {
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": "test-pack",
        "name": "Test pack",
        "files": files or [],
        "dependencies": dependencies,
    }
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("modrinth.index.json", json.dumps(index))
        archive.writestr("overrides/config/test.cfg", "test=true")


class ModpackDependencyTests(unittest.TestCase):
    dependencies = {"minecraft": "1.20.1", "fabric-loader": "0.15.4"}

    def test_preflight_reports_required_game_loader_and_exact_loader_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "pack.mrpack"
            _write_mrpack(archive, self.dependencies)

            requirements = ModpackImporter(FakeHttp()).preflight_file(
                archive,
                expected_game_version="1.20.1",
                expected_loader="fabric",
                expected_loader_version="0.15.4",
            )

        self.assertEqual(
            requirements,
            {"minecraft": "1.20.1", "loader": "fabric", "loader_version": "0.15.4", "compatible": True, "issues": []},
        )

    def test_catalog_preflight_uses_the_selected_exact_loader_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "pack.mrpack"
            _write_mrpack(archive, self.dependencies)
            content = archive.read_bytes()
        url = "https://cdn.example.invalid/pack.mrpack"
        digest = hashlib.sha256(content).hexdigest()

        def json_handler(request_url: str):
            if "/project/pack/version" in request_url:
                return [{
                    "version_number": "1.0",
                    "game_versions": ["1.20.1"],
                    "files": [{
                        "filename": "pack.mrpack",
                        "url": url,
                        "size": len(content),
                        "hashes": {"sha256": digest},
                    }],
                }]
            return {"title": "Pack"}

        http = FakeHttp(json_handler=json_handler, payloads={url: content})
        provider = ModpackProvider(http, {"kind": "modrinth", "api_url": "https://api.modrinth.com/v2"})

        mismatch = provider.preflight(
            "pack",
            "1.20.1",
            version="1.0",
            loader="fabric",
            loader_version="0.15.3",
        )
        self.assertFalse(mismatch["compatible"])
        self.assertIn("требует Fabric версии 0.15.4", mismatch["issues"][0])

        requirements = provider.preflight(
            "pack",
            "1.20.1",
            version="1.0",
            loader="fabric",
            loader_version="0.15.4",
        )
        self.assertEqual(requirements["loader_version"], "0.15.4")
        self.assertEqual(http.downloads, [url, url])

    def test_preflight_rejects_game_family_and_exact_loader_mismatches(self) -> None:
        cases = (
            (
                {"expected_game_version": "1.20.4", "expected_loader": "fabric", "expected_loader_version": "0.15.4"},
                "требует Minecraft 1.20.1",
            ),
            (
                {"expected_game_version": "1.20.1", "expected_loader": "quilt", "expected_loader_version": "0.15.4"},
                "требует загрузчик fabric",
            ),
            (
                {"expected_game_version": "1.20.1", "expected_loader": "fabric", "expected_loader_version": "0.15.5"},
                "требует Fabric версии 0.15.4",
            ),
        )
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "pack.mrpack"
            _write_mrpack(archive, self.dependencies)
            importer = ModpackImporter(FakeHttp())

            for options, message in cases:
                with self.subTest(message=message):
                    result = importer.preflight_file(archive, strict_compatibility=False, **options)
                    self.assertFalse(result["compatible"])
                    self.assertTrue(any(message in issue for issue in result["issues"]))

    def test_preflight_rejects_a_missing_choice_for_a_pinned_loader_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "pack.mrpack"
            _write_mrpack(archive, self.dependencies)

            requirements = ModpackImporter(FakeHttp()).preflight_file(
                archive,
                expected_game_version="1.20.1",
                expected_loader="fabric",
                expected_loader_version="",
                strict_compatibility=False,
            )

        self.assertFalse(requirements["compatible"])
        self.assertTrue(any("версия загрузчика не выбрана" in issue for issue in requirements["issues"]))

    def test_loader_mismatch_does_not_create_staging_or_change_target_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "pack.mrpack"
            _write_mrpack(archive, self.dependencies)
            target = root / "instance"
            target.mkdir()
            (target / "keep.txt").write_text("unchanged", encoding="utf-8")
            http = FakeHttp()

            with self.assertRaisesRegex(LauncherError, "требует Fabric версии 0.15.4"):
                ModpackImporter(http).import_file(
                    archive,
                    target,
                    source="local",
                    project_id="",
                    display_name="Test pack",
                    version="",
                    expected_game_version="1.20.1",
                    expected_loader="fabric",
                    expected_loader_version="0.15.3",
                )

            self.assertEqual((target / "keep.txt").read_text(encoding="utf-8"), "unchanged")
            self.assertEqual([path.name for path in target.iterdir()], ["keep.txt"])
            self.assertEqual(http.downloads, [])

    def test_unverified_external_file_gate_remains_in_preflight(self) -> None:
        url = "https://cdn.example.invalid/unverified.jar"
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "pack.mrpack"
            _write_mrpack(
                archive,
                self.dependencies,
                [{"path": "mods/unverified.jar", "downloads": [url], "hashes": {}}],
            )
            http = FakeHttp()
            importer = ModpackImporter(http)
            options = {
                "expected_game_version": "1.20.1",
                "expected_loader": "fabric",
                "expected_loader_version": "0.15.4",
            }

            with self.assertRaisesRegex(LauncherError, r"\[unverified-download\]"):
                importer.preflight_file(archive, **options)

            requirements = importer.preflight_file(archive, confirm_unverified=True, **options)

        self.assertEqual(http.downloads, [])
        self.assertEqual(requirements["loader_version"], "0.15.4")

    def test_actual_installed_loader_version_is_read_for_each_loader_family(self) -> None:
        cases = (
            ("fabric", "fabric-loader-0.15.4-1.20.1", "net.fabricmc:fabric-loader:0.15.4", "0.15.4"),
            ("quilt", "quilt-loader-0.25.0-1.20.1", "org.quiltmc:quilt-loader:0.25.0", "0.25.0"),
            ("forge", "1.20.1-forge-47.2.0", "net.minecraftforge:forge:1.20.1-47.2.0", "47.2.0"),
            ("neoforge", "neoforge-20.4.237", "net.neoforged:neoforge:20.4.237", "20.4.237"),
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for loader, version_id, coordinate, expected in cases:
                with self.subTest(loader=loader):
                    version_dir = root / loader / "versions" / version_id
                    version_dir.mkdir(parents=True)
                    (version_dir / f"{version_id}.json").write_text(
                        json.dumps({"id": version_id, "libraries": [{"name": coordinate}]}),
                        encoding="utf-8",
                    )
                    instance = SimpleNamespace(
                        loader=loader,
                        version_id=version_id,
                        minecraft_version="1.20.1",
                        path=root / loader,
                    )

                    self.assertEqual(_instance_loader_version(instance), expected)


if __name__ == "__main__":
    unittest.main()
