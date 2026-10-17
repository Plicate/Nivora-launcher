from __future__ import annotations

import json
import tempfile
import unittest
import zipfile
from pathlib import Path

from nivora_launcher.errors import LauncherError
from nivora_launcher.modpacks import ModpackImporter


class FakeHttp:
    """Resolver + download stub for the CurseForge manifest flow."""

    def __init__(self, resolutions: dict[str, dict] | None = None) -> None:
        self.resolutions = resolutions or {}
        self.json_urls: list[str] = []

    def get_json(self, url: str):
        self.json_urls.append(url)
        return self.resolutions.get(url, {})

    def download(self, url: str, destination: Path, **_kwargs) -> bool:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(b"fake-jar-bytes")
        return True


def _write_curseforge_pack(path: Path, files: list[dict], minecraft: dict | None = None, overrides: dict[str, bytes] | None = None) -> None:
    manifest = {
        "minecraft": minecraft or {"version": "1.20.1", "modLoaders": [{"id": "forge-47.2.0", "primary": True}]},
        "manifestType": "minecraftModpack",
        "manifestVersion": 1,
        "name": "Example CF pack",
        "files": files,
        "overrides": "overrides",
    }
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("manifest.json", json.dumps(manifest))
        archive.writestr("modlist.html", "<html></html>")
        for name, content in (overrides or {}).items():
            archive.writestr(f"overrides/{name}", content)


class CurseForgeImportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_manifest_mods_are_downloaded_instead_of_silently_skipped(self) -> None:
        # Раньше manifest.json отбрасывался, overrides распаковывались, и
        # импорт «притворялся успешным» без единого мода.
        http = FakeHttp({
            "https://api.cfwidget.com/238222/files/4583475": {
                "download_url": "https://edge.forgecdn.net/files/4583/475/create-0.5.1.jar",
                "display": "Create 0.5.1",
                "size": 12345,
            },
        })
        importer = ModpackImporter(http)
        archive = self.root / "pack.zip"
        _write_curseforge_pack(archive, [{"projectID": 238222, "fileID": 4583475, "required": True}], overrides={"config/setting.txt": b"data"})
        target = self.root / "instance"
        target.mkdir()

        result = importer.import_file(
            archive,
            target,
            source="local",
            project_id="",
            display_name="Example CF pack",
            version="1.0",
            confirm_unverified=True,
            expected_game_version="1.20.1",
            expected_loader="forge",
            expected_loader_version="47.2.0",
        )

        written = set(result["files"])
        self.assertIn("mods/create-0.5.1.jar", written)
        self.assertIn("config/setting.txt", written)
        self.assertFalse(result["trusted_hash"])

    def test_manifest_download_requires_user_confirmation(self) -> None:
        importer = ModpackImporter(FakeHttp())
        archive = self.root / "pack.zip"
        _write_curseforge_pack(archive, [{"projectID": 1, "fileID": 2}])
        target = self.root / "instance"
        target.mkdir()
        with self.assertRaises(LauncherError) as raised:
            importer.import_file(
                archive,
                target,
                source="local",
                project_id="",
                display_name="pack",
                version="",
                confirm_unverified=False,
            )
        self.assertIn("[unverified-download]", str(raised.exception))

    def test_resolver_only_accepts_forgecdn_downloads(self) -> None:
        importer = ModpackImporter(
            FakeHttp({
                "https://api.cfwidget.com/1/files/2": {
                    "download_url": "https://evil.example.invalid/files/create.jar",
                    "display": "Create",
                },
            })
        )
        archive = self.root / "pack.zip"
        _write_curseforge_pack(archive, [{"projectID": 1, "fileID": 2}])
        target = self.root / "instance"
        target.mkdir()
        with self.assertRaises(LauncherError) as raised:
            importer.import_file(
                archive,
                target,
                source="local",
                project_id="",
                display_name="pack",
                version="",
                confirm_unverified=True,
            )
        self.assertIn("forgecdn.net", str(raised.exception))

    def test_unresolvable_required_file_fails_loudly(self) -> None:
        importer = ModpackImporter(FakeHttp())
        archive = self.root / "pack.zip"
        _write_curseforge_pack(archive, [{"projectID": 999, "fileID": 1, "required": True}])
        target = self.root / "instance"
        target.mkdir()
        with self.assertRaises(LauncherError) as raised:
            importer.import_file(
                archive,
                target,
                source="local",
                project_id="",
                display_name="pack",
                version="",
                confirm_unverified=True,
            )
        self.assertIn("999/1", str(raised.exception))

    def test_optional_unresolvable_file_is_skipped(self) -> None:
        http = FakeHttp({
            "https://api.cfwidget.com/5/files/6": {
                "download_url": "https://mediafilez.forgecdn.net/files/5/6/extra.jar",
                "display": "Extra",
            },
        })
        importer = ModpackImporter(http)
        archive = self.root / "pack.zip"
        _write_curseforge_pack(
            archive,
            [
                {"projectID": 4, "fileID": 5, "required": False},
                {"projectID": 5, "fileID": 6, "required": True},
            ],
        )
        target = self.root / "instance"
        target.mkdir()
        result = importer.import_file(
            archive,
            target,
            source="local",
            project_id="",
            display_name="pack",
            version="",
            confirm_unverified=True,
            expected_game_version="1.20.1",
            expected_loader="forge",
            expected_loader_version="47.2.0",
        )
        self.assertEqual([item for item in result["files"] if item.startswith("mods/")], ["mods/extra.jar"])

    def test_curseforge_compatibility_requirements_are_reported(self) -> None:
        importer = ModpackImporter(FakeHttp())
        archive = self.root / "pack.zip"
        _write_curseforge_pack(archive, [])
        dependencies = importer.preflight_file(
            archive,
            expected_game_version="1.19.2",
            expected_loader="fabric",
            expected_loader_version="0.15.7",
            strict_compatibility=False,
        )
        self.assertEqual(dependencies["minecraft"], "1.20.1")
        self.assertEqual(dependencies["loader"], "forge")
        self.assertEqual(dependencies["loader_version"], "47.2.0")
        self.assertFalse(dependencies["compatible"])
        self.assertTrue(any("forge" in issue.lower() for issue in dependencies["issues"]))

    def test_mod_preview_lists_mods_from_every_source(self) -> None:
        http = FakeHttp({
            "https://api.cfwidget.com/7/files/8": {
                "download_url": "https://edge.forgecdn.net/files/7/8/sodium.jar",
                "display": "Sodium",
            },
        })
        importer = ModpackImporter(http)
        archive = self.root / "pack.zip"
        _write_curseforge_pack(
            archive,
            [{"projectID": 7, "fileID": 8, "required": True}],
            overrides={"mods/bundled-mod.jar": b"jar"},
        )
        preview = importer.mod_preview_file(archive)
        names = {item["name"]: item["source"] for item in preview["mods"]}
        self.assertEqual(names.get("Sodium"), "curseforge")
        self.assertEqual(names.get("bundled-mod.jar"), "archive")
        self.assertEqual(preview["total"], 2)

    def test_preview_of_plain_overrides_archive(self) -> None:
        importer = ModpackImporter(FakeHttp())
        archive = self.root / "pack.zip"
        with zipfile.ZipFile(archive, "w") as pack:
            pack.writestr("overrides/mods/simple-mod.jar", b"jar")
            pack.writestr("overrides/config.toml", b"config")
        preview = importer.mod_preview_file(archive)
        self.assertEqual(preview["total"], 1)
        self.assertEqual(preview["mods"][0]["name"], "simple-mod.jar")


if __name__ == "__main__":
    unittest.main()
