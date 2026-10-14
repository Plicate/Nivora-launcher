from __future__ import annotations

import hashlib
import json
import shutil
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.modpacks import MODPACK_MARKER, ModpackImporter, ModpackProvider
from nivora_launcher.mods import ModrinthProvider


class FakeHttp:
    def __init__(self, *, json_handler=None, payloads: dict[str, bytes] | None = None) -> None:
        self.json_handler = json_handler or (lambda _url: {})
        self.payloads = payloads or {}
        self.json_urls: list[str] = []
        self.downloads: list[tuple[str, Path, dict]] = []

    def get_json(self, url: str):
        self.json_urls.append(url)
        return self.json_handler(url)

    def download(self, url: str, destination: Path, **kwargs) -> bool:
        self.downloads.append((url, destination, kwargs))
        payload = self.payloads.get(url, b"downloaded")
        progress = kwargs.get("progress")
        if progress:
            progress(len(payload), len(payload))
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(payload)
        return True


def _write_mrpack(path: Path, index: dict, overrides: dict[str, bytes] | None = None) -> None:
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("modrinth.index.json", json.dumps(index))
        for name, content in (overrides or {}).items():
            archive.writestr(name, content)


class ModpackSafetyTests(unittest.TestCase):
    def test_modrinth_modpack_details_preserve_gallery_images(self) -> None:
        gallery = [{"url": "https://cdn.example.invalid/screenshot.png", "title": "In-game view"}]
        http = FakeHttp(
            json_handler=lambda _url: {
                "id": "pack",
                "project_type": "modpack",
                "title": "Example pack",
                "gallery": gallery,
            }
        )

        details = ModpackProvider(
            http,
            {"kind": "modrinth", "api_url": "https://api.modrinth.com/v2"},
        ).project_details("pack")

        self.assertEqual(details["gallery"], gallery)
        self.assertEqual(http.json_urls, ["https://api.modrinth.com/v2/project/pack"])

    def test_modrinth_resource_size_is_not_a_hash_and_gate_has_no_disk_side_effects(self) -> None:
        payload = b"resource"

        def response(url: str):
            if "/project/resource/version" in url:
                return [{"version_number": "1.0", "files": [{"filename": "resource.jar", "url": "https://cdn.example.invalid/resource.jar", "size": len(payload), "hashes": {}}]}]
            return {"title": "Resource"}

        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            http = FakeHttp(json_handler=response, payloads={"https://cdn.example.invalid/resource.jar": payload})

            with self.assertRaisesRegex(LauncherError, r"\[unverified-download\]"):
                ModrinthProvider(http).install("resource", instance, "1.20.1", loader="fabric")

            self.assertEqual(http.downloads, [])
            self.assertFalse((instance / "mods").exists())

    def test_modrinth_resource_download_proceeds_only_after_explicit_confirmation(self) -> None:
        payload = b"resource"

        def response(url: str):
            if "/project/resource/version" in url:
                return [{"version_number": "1.0", "files": [{"filename": "resource.jar", "url": "https://cdn.example.invalid/resource.jar", "size": len(payload), "hashes": {}}]}]
            return {"title": "Resource"}

        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            http = FakeHttp(json_handler=response, payloads={"https://cdn.example.invalid/resource.jar": payload})

            result = ModrinthProvider(http).install(
                "resource",
                instance,
                "1.20.1",
                loader="fabric",
                confirm_unverified=True,
            )

            self.assertEqual(result.path.read_bytes(), payload)
            self.assertEqual(len(http.downloads), 1)

    def test_catalog_modpack_install_rejects_duplicate_project_before_network_access(self) -> None:
        http = FakeHttp()
        provider = ModpackProvider(
            http,
            {"id": "modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2"},
        )

        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "instance"
            target.mkdir()
            (target / MODPACK_MARKER).write_text(
                json.dumps([{"source": "modrinth", "catalog_id": "modrinth", "project_id": "pack"}]),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(LauncherError, "уже установлен"):
                provider.install("pack", target, "1.20.1")

            self.assertEqual(http.json_urls, [])
            self.assertEqual(http.downloads, [])

    def test_importing_the_same_local_archive_twice_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "pack.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("options.txt", "music:0.2\n")
            target = root / "instance"
            target.mkdir()
            importer = ModpackImporter(FakeHttp())

            first = importer.import_file(
                archive_path,
                target,
                source="local",
                project_id="",
                display_name="Pack",
                version="",
            )
            self.assertIn("options.txt", first["files"])
            with self.assertRaisesRegex(LauncherError, "уже импортирован"):
                importer.import_file(
                    archive_path,
                    target,
                    source="local",
                    project_id="",
                    display_name="Pack",
                    version="",
                )

            marker = json.loads((target / MODPACK_MARKER).read_text(encoding="utf-8"))
            self.assertEqual(len(marker), 1)
            self.assertEqual(marker[0]["archive_sha256"], hashlib.sha256(archive_path.read_bytes()).hexdigest())

    def test_mrpack_preflight_rejects_an_oversized_index_before_parsing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive_path = Path(directory) / "large-index.mrpack"
            _write_mrpack(archive_path, {"files": []})
            with patch("nivora_launcher.modpacks.MAX_MODPACK_INDEX_BYTES", 8):
                with self.assertRaisesRegex(LauncherError, "index.json слишком большой"):
                    ModpackImporter(FakeHttp()).preflight_file(archive_path)

    def test_catalog_modpack_size_without_hash_is_blocked_before_archive_download(self) -> None:
        archive_url = "https://cdn.example.invalid/pack.mrpack"

        def response(url: str):
            if "/project/pack/version" in url:
                return [{"version_number": "1.0", "files": [{"filename": "pack.mrpack", "url": archive_url, "size": 12, "hashes": {}}]}]
            return {"title": "Pack"}

        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "instance"
            target.mkdir()
            http = FakeHttp(json_handler=response)

            with self.assertRaisesRegex(LauncherError, r"\[unverified-download\]"):
                ModpackProvider(http, {"kind": "modrinth", "api_url": "https://api.modrinth.com/v2"}).install(
                    "pack",
                    target,
                    "1.20.1",
                    version="1.0",
                )

            self.assertEqual(http.downloads, [])
            self.assertEqual(list(target.iterdir()), [])

    def test_catalog_mrpack_preflight_finds_unverified_external_files_before_instance_creation(self) -> None:
        external_url = "https://cdn.example.invalid/library.jar"
        content = json.dumps({"files": [{"path": "mods/library.jar", "downloads": [external_url], "hashes": {}}]}).encode()
        archive_path = Path("pack.mrpack")
        with tempfile.TemporaryDirectory() as directory:
            local_archive = Path(directory) / archive_path
            with zipfile.ZipFile(local_archive, "w") as archive:
                archive.writestr("modrinth.index.json", content)
            archive_bytes = local_archive.read_bytes()
        archive_url = "https://cdn.example.invalid/pack.mrpack"
        digest = hashlib.sha256(archive_bytes).hexdigest()

        def response(url: str):
            if "/project/pack/version" in url:
                return [{"version_number": "1.0", "files": [{"filename": "pack.mrpack", "url": archive_url, "size": len(archive_bytes), "hashes": {"sha256": digest}}]}]
            return {"title": "Pack"}

        http = FakeHttp(json_handler=response, payloads={archive_url: archive_bytes})
        provider = ModpackProvider(http, {"kind": "modrinth", "api_url": "https://api.modrinth.com/v2"})

        with self.assertRaisesRegex(LauncherError, r"\[unverified-download\]"):
            provider.preflight("pack", "1.20.1", version="1.0", loader="fabric")

        self.assertEqual([item[0] for item in http.downloads], [archive_url])

    def test_local_mrpack_external_hashes_are_preflighted_but_local_overrides_need_no_hash(self) -> None:
        verified_url = "https://cdn.example.invalid/verified.jar"
        unverified_url = "https://cdn.example.invalid/unverified.jar"
        verified_payload = b"verified"
        unverified_payload = b"unverified"
        index = {
            "files": [
                {"path": "mods/verified.jar", "downloads": [verified_url], "hashes": {"sha1": hashlib.sha1(verified_payload).hexdigest()}},
                {"path": "mods/unverified.jar", "downloads": [unverified_url], "hashes": {}, "fileSize": len(unverified_payload)},
            ]
        }

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "pack.mrpack"
            _write_mrpack(archive_path, index, {"overrides/config/client.cfg": b"client=true"})
            target = root / "instance"
            target.mkdir()
            (target / "keep.txt").write_text("unchanged", encoding="utf-8")
            http = FakeHttp(payloads={verified_url: verified_payload, unverified_url: unverified_payload})
            importer = ModpackImporter(http)

            with self.assertRaisesRegex(LauncherError, r"\[unverified-download\]"):
                importer.import_file(
                    archive_path,
                    target,
                    source="local",
                    project_id="",
                    display_name="Pack",
                    version="",
                )

            self.assertEqual(http.downloads, [])
            self.assertEqual((target / "keep.txt").read_text(encoding="utf-8"), "unchanged")
            self.assertFalse((target / "config" / "client.cfg").exists())

            result = importer.import_file(
                archive_path,
                target,
                source="local",
                project_id="",
                display_name="Pack",
                version="",
                confirm_unverified=True,
            )

            self.assertFalse(result["trusted_hash"])
            self.assertEqual((target / "mods" / "verified.jar").read_bytes(), verified_payload)
            self.assertEqual((target / "mods" / "unverified.jar").read_bytes(), unverified_payload)
            self.assertEqual((target / "config" / "client.cfg").read_bytes(), b"client=true")

    def test_ftb_manifest_size_is_not_a_hash_and_no_file_is_downloaded_before_confirmation(self) -> None:
        url = "https://cdn.example.invalid/ftb.jar"
        entry = {"path": "mods", "name": "ftb.jar", "url": url, "size": 8}

        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "instance"
            target.mkdir()
            http = FakeHttp()

            with self.assertRaisesRegex(LauncherError, r"\[unverified-download\]"):
                ModpackImporter(http).import_manifest(
                    [entry],
                    target,
                    source="ftb",
                    project_id="pack",
                    display_name="Pack",
                    version="1",
                )

            self.assertEqual(http.downloads, [])
            self.assertEqual(list(target.iterdir()), [])

    def test_ftb_manifest_budget_uses_actual_staged_bytes(self) -> None:
        first_url = "https://cdn.example.invalid/first.jar"
        second_url = "https://cdn.example.invalid/second.jar"
        first_payload = b"123456"
        second_payload = b"abcdef"
        entries = [
            {"path": "mods", "name": "first.jar", "url": first_url, "sha1": hashlib.sha1(first_payload).hexdigest(), "size": 2},
            {"path": "mods", "name": "second.jar", "url": second_url, "sha1": hashlib.sha1(second_payload).hexdigest()},
        ]

        with tempfile.TemporaryDirectory() as directory, patch("nivora_launcher.modpacks.MAX_MODPACK_UNCOMPRESSED_BYTES", 10):
            target = Path(directory) / "instance"
            target.mkdir()
            http = FakeHttp(payloads={first_url: first_payload, second_url: second_payload})

            with self.assertRaisesRegex(LauncherError, "слишком большой"):
                ModpackImporter(http).import_manifest(
                    entries,
                    target,
                    source="ftb",
                    project_id="pack",
                    display_name="Pack",
                    version="1",
                )

            self.assertEqual([item[0] for item in http.downloads], [first_url, second_url])
            self.assertEqual(list(target.iterdir()), [])

    def test_modrinth_external_file_budget_uses_actual_staged_bytes(self) -> None:
        first_url = "https://cdn.example.invalid/first.jar"
        second_url = "https://cdn.example.invalid/second.jar"
        first_payload = b"123456"
        second_payload = b"abcdef"
        index = {
            "files": [
                {"path": "mods/first.jar", "downloads": [first_url], "hashes": {"sha1": hashlib.sha1(first_payload).hexdigest()}, "fileSize": 2},
                {"path": "mods/second.jar", "downloads": [second_url], "hashes": {"sha1": hashlib.sha1(second_payload).hexdigest()}},
            ]
        }

        with tempfile.TemporaryDirectory() as directory, patch("nivora_launcher.modpacks.MAX_MODPACK_UNCOMPRESSED_BYTES", 10):
            root = Path(directory)
            archive_path = root / "pack.mrpack"
            _write_mrpack(archive_path, index)
            target = root / "instance"
            target.mkdir()
            http = FakeHttp(payloads={first_url: first_payload, second_url: second_payload})

            with self.assertRaisesRegex(LauncherError, "слишком большой"):
                ModpackImporter(http).import_file(
                    archive_path,
                    target,
                    source="local",
                    project_id="",
                    display_name="Pack",
                    version="",
                )

            self.assertEqual([item[0] for item in http.downloads], [first_url, second_url])
            self.assertEqual(list(target.iterdir()), [])

    def test_server_overrides_are_not_imported_into_client(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "pack.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("overrides/config/client.cfg", "client=true")
                archive.writestr("server-overrides/config/server.cfg", "server=true")
                archive.writestr("overrides/server-overrides/config/nested.cfg", "nested=true")
                archive.writestr("pack/server-overrides/root.cfg", "root=true")
                archive.writestr("client-overrides/options.txt", "client-option=true")
            target = root / "instance"
            target.mkdir()

            result = ModpackImporter(FakeHttp()).import_file(
                archive_path,
                target,
                source="local",
                project_id="",
                display_name="Pack",
                version="",
            )

            self.assertEqual(result["files"], ["config/client.cfg", "options.txt"])
            self.assertTrue((target / "config" / "client.cfg").exists())
            self.assertTrue((target / "options.txt").exists())
            self.assertFalse((target / "config" / "server.cfg").exists())
            self.assertFalse((target / "config" / "nested.cfg").exists())
            self.assertFalse((target / "pack" / "server-overrides" / "root.cfg").exists())

    def test_merge_write_failure_restores_overwritten_file_and_removes_new_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "pack.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("a.txt", "new-a")
                archive.writestr("b.txt", "new-b")
            target = root / "instance"
            target.mkdir()
            (target / "a.txt").write_text("old-a", encoding="utf-8")
            marker_data = json.dumps([{"source": "old", "project_id": "existing", "files": []}])
            (target / MODPACK_MARKER).write_text(marker_data, encoding="utf-8")
            copyfile = shutil.copyfile

            def fail_b(source, destination, *args, **kwargs):
                if Path(destination) == target / "b.txt":
                    Path(destination).write_bytes(b"partial")
                    raise OSError("simulated disk write failure")
                return copyfile(source, destination, *args, **kwargs)

            with patch("nivora_launcher.modpacks.shutil.copyfile", side_effect=fail_b):
                with self.assertRaisesRegex(OSError, "simulated disk write failure"):
                    ModpackImporter(FakeHttp()).import_file(
                        archive_path,
                        target,
                        source="local",
                        project_id="new",
                        display_name="Pack",
                        version="",
                    )

            self.assertEqual((target / "a.txt").read_text(encoding="utf-8"), "old-a")
            self.assertFalse((target / "b.txt").exists())
            self.assertEqual((target / MODPACK_MARKER).read_text(encoding="utf-8"), marker_data)
            self.assertFalse(any(path.name.startswith(".trial-launcher-modpack-backup-") for path in target.iterdir()))

    def test_marker_write_failure_rolls_back_merged_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "pack.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("a.txt", "new-a")
            target = root / "instance"
            target.mkdir()
            (target / "a.txt").write_text("old-a", encoding="utf-8")

            with patch("nivora_launcher.modpacks._record_modpack", side_effect=OSError("simulated marker write failure")):
                with self.assertRaisesRegex(OSError, "simulated marker write failure"):
                    ModpackImporter(FakeHttp()).import_file(
                        archive_path,
                        target,
                        source="local",
                        project_id="new",
                        display_name="Pack",
                        version="",
                    )

            self.assertEqual((target / "a.txt").read_text(encoding="utf-8"), "old-a")
            self.assertFalse((target / MODPACK_MARKER).exists())

    def test_mrpack_preflight_reports_known_minecraft_and_loader_mismatches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive_path = Path(directory) / "pack.mrpack"
            _write_mrpack(
                archive_path,
                {"files": [], "dependencies": {"minecraft": "1.20.1", "fabric-loader": "0.15.0"}},
            )
            importer = ModpackImporter(FakeHttp())

            game_mismatch = importer.preflight_file(
                archive_path,
                expected_game_version="1.21.1",
                expected_loader="fabric",
                strict_compatibility=False,
            )
            loader_mismatch = importer.preflight_file(
                archive_path,
                expected_game_version="1.20.1",
                expected_loader="vanilla",
                strict_compatibility=False,
            )

        self.assertFalse(game_mismatch["compatible"])
        self.assertTrue(any("требует Minecraft 1.20.1" in issue for issue in game_mismatch["issues"]))
        self.assertFalse(loader_mismatch["compatible"])
        self.assertTrue(any("требует загрузчик fabric" in issue for issue in loader_mismatch["issues"]))


if __name__ == "__main__":
    unittest.main()
