from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.installer import MinecraftInstaller, _remove_instance_path, _rename_exchange, _swap_staged_directory
from nivora_launcher.net import verify_file


CLIENT_URL = "https://launcher.example.invalid/client.jar"
LIBRARY_URL = "https://launcher.example.invalid/library.jar"
FORGE_URL = "https://launcher.example.invalid/forge-47.2.0.jar"
INDEX_URL = "https://launcher.example.invalid/assets.json"


def _sha1(payload: bytes) -> str:
    return hashlib.sha1(payload).hexdigest()


def _rewrite_corrupt(path: Path, content: bytes) -> None:
    path.write_bytes(content)
    stat = path.stat()
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000))


def _version_json() -> tuple[dict, dict[str, bytes]]:
    client = b"client-good"
    library = b"library-good"
    asset = b"asset-good"
    digest = _sha1(asset)
    index = json.dumps(
        {"objects": {"textures/example.png": {"hash": digest, "size": len(asset)}}},
        separators=(",", ":"),
    ).encode("utf-8")
    version = {
        "id": "1.20.1",
        "mainClass": "example.Main",
        "downloads": {
            "client": {"url": CLIENT_URL, "sha1": _sha1(client), "size": len(client)},
        },
        "libraries": [
            {
                "name": "example:library:1.0",
                "downloads": {
                    "artifact": {
                        "path": "example/library/1.0/library-1.0.jar",
                        "url": LIBRARY_URL,
                        "sha1": _sha1(library),
                        "size": len(library),
                    }
                },
            }
        ],
        "assetIndex": {
            "id": "1.20.1",
            "url": INDEX_URL,
            "sha1": _sha1(index),
            "size": len(index),
        },
        "assets": "1.20.1",
    }
    return version, {
        CLIENT_URL: client,
        LIBRARY_URL: library,
        INDEX_URL: index,
        f"https://resources.download.minecraft.net/{digest[:2]}/{digest}": asset,
    }


class FakeRepository:
    def __init__(self, version_json: dict) -> None:
        self.version_json = version_json
        self.resolutions: list[str] = []

    def resolve(self, version_id: str) -> dict:
        self.resolutions.append(version_id)
        return json.loads(json.dumps(self.version_json))

    def library_url(self, library: dict, classifier: str | None = None) -> str:
        downloads = library.get("downloads", {})
        artifact = downloads.get("artifact", {}) if not classifier else downloads.get("classifiers", {}).get(classifier, {})
        if artifact.get("url"):
            return str(artifact["url"])
        return f"https://launcher.example.invalid/{library['name'].replace(':', '-')}.jar"


class FakeLoaders:
    def profile(self, loader: str, minecraft_version: str, loader_version: str | None = None) -> dict:
        return {
            "id": f"{minecraft_version}-{loader}-{loader_version or 'test'}",
            "inheritsFrom": minecraft_version,
        }


class FakeHttp:
    def __init__(self, payloads: dict[str, bytes], *, fail_url: str = "") -> None:
        self.payloads = payloads
        self.fail_url = fail_url
        self.download_calls: list[str] = []
        self.network_calls: list[str] = []

    def download(
        self,
        url: str,
        destination: Path,
        *,
        sha1: str | None = None,
        sha256: str | None = None,
        sha512: str | None = None,
        size: int | None = None,
        **_kwargs,
    ) -> bool:
        self.download_calls.append(url)
        if destination.exists() and verify_file(destination, sha1=sha1, sha256=sha256, sha512=sha512, size=size):
            return False
        self.network_calls.append(url)
        if url == self.fail_url:
            raise LauncherError("simulated download failure")
        payload = self.payloads[url]
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(payload)
        if not verify_file(destination, sha1=sha1, sha256=sha256, sha512=sha512, size=size):
            raise AssertionError(f"fixture payload does not match trusted metadata: {url}")
        return True

    def check_cancelled(self) -> None:
        return None


@contextmanager
def _config(root: Path):
    with patch.dict(
        os.environ,
        {
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_CACHE_HOME": str(root / "cache"),
        },
        clear=False,
    ):
        config = ConfigStore(Paths())
        config.set_value("game_dir", str(root / "game"))
        yield config


class InstallerRecoveryTests(unittest.TestCase):
    def test_load_existing_merges_inherited_profile_without_main_class(self) -> None:
        version_json, payloads = _version_json()
        inherited = {
            "id": "1.20.1-forge-47.2.0",
            "inheritsFrom": "1.20.1",
            "releaseTime": "2024-01-01T00:00:00Z",
            "libraries": [
                {
                    "name": "net.minecraftforge:forge:47.2.0",
                    "downloads": {
                        "artifact": {
                            "path": "net/minecraftforge/forge/47.2.0/forge-47.2.0.jar",
                            "url": FORGE_URL,
                            "sha1": _sha1(b"forge-good"),
                            "size": len(b"forge-good"),
                        }
                    },
                }
            ],
            "arguments": {"game": ["--fml.forgeVersion", "47.2.0"]},
        }
        payloads[FORGE_URL] = b"forge-good"
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            installer = MinecraftInstaller(config, repository=FakeRepository(version_json), http=FakeHttp(payloads))
            installed = installer.install("1.20.1", instance_name="forge-pack", assets=True)
            forge_dir = installed.instance_dir / "versions" / "1.20.1-forge-47.2.0"
            forge_dir.mkdir(parents=True)
            (forge_dir / "1.20.1-forge-47.2.0.json").write_text(json.dumps(inherited), encoding="utf-8")

            resolved = installer.load_existing(installed.instance_dir, "1.20.1-forge-47.2.0")

            self.assertEqual(resolved.version_json.get("mainClass"), "example.Main")
            forge_jar = next((str(item) for item in resolved.libraries if "forge-47.2.0" in str(item)), "")
            self.assertTrue(forge_jar.endswith("net/minecraftforge/forge/47.2.0/forge-47.2.0.jar"))

    def test_existing_directory_swap_keeps_old_tree_until_new_tree_is_ready(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "instance"
            staging = root / "staging"
            target.mkdir()
            staging.mkdir()
            (target / "old.txt").write_text("old", encoding="utf-8")
            (staging / "new.txt").write_text("new", encoding="utf-8")

            with patch("nivora_launcher.installer._rename_exchange", wraps=_rename_exchange) as exchange:
                previous = _swap_staged_directory(staging, target)

            self.assertEqual(exchange.call_count, 1)
            self.assertEqual((target / "new.txt").read_text(encoding="utf-8"), "new")
            self.assertIsNotNone(previous)
            self.assertEqual((previous / "old.txt").read_text(encoding="utf-8"), "old")
            _remove_instance_path(previous)

    def test_failed_replacement_preserves_existing_instance_for_all_loader_families(self) -> None:
        version_json, payloads = _version_json()
        cases = (
            ("vanilla", None),
            ("fabric", "0.15.0"),
            ("quilt", "0.24.0"),
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for loader, loader_version in cases:
                with self.subTest(loader=loader), _config(root / loader) as config:
                    http = FakeHttp(payloads, fail_url=CLIENT_URL)
                    installer = MinecraftInstaller(
                        config,
                        repository=FakeRepository(version_json),
                        loaders=FakeLoaders(),
                        http=http,
                    )
                    target = config.game_dir / "instances" / "existing"
                    target.mkdir(parents=True)
                    (target / "working.txt").write_text("old instance", encoding="utf-8")

                    with self.assertRaisesRegex(LauncherError, "simulated download failure"):
                        installer.install(
                            "1.20.1",
                            loader=loader,
                            loader_version=loader_version,
                            instance_name="existing",
                            replace_existing=True,
                        )

                    self.assertEqual((target / "working.txt").read_text(encoding="utf-8"), "old instance")
                    self.assertEqual(list((config.game_dir / "instances").glob(".trial-stage-*")), [])

    def test_forge_and_neoforge_replacement_builds_in_staging_before_failure(self) -> None:
        version_json, payloads = _version_json()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for loader in ("forge", "neoforge"):
                with self.subTest(loader=loader), _config(root / loader) as config:
                    installer = MinecraftInstaller(
                        config,
                        repository=FakeRepository(version_json),
                        loaders=FakeLoaders(),
                        http=FakeHttp(payloads),
                    )
                    target = config.game_dir / "instances" / "existing"
                    target.mkdir(parents=True)
                    (target / "working.txt").write_text("old instance", encoding="utf-8")

                    def fail_in_staging(_version, _loader, _loader_version, _assets, staging, _base):
                        (staging / "partial-loader-output").write_text("partial", encoding="utf-8")
                        raise LauncherError("simulated loader failure")

                    with patch.object(installer, "_install_external_loader", side_effect=fail_in_staging):
                        with self.assertRaisesRegex(LauncherError, "simulated loader failure"):
                            installer.install(
                                "1.20.1",
                                loader=loader,
                                loader_version="test-loader",
                                instance_name="existing",
                                replace_existing=True,
                            )

                    self.assertEqual((target / "working.txt").read_text(encoding="utf-8"), "old instance")
                    self.assertEqual(list((config.game_dir / "instances").glob(".trial-stage-*")), [])

    def test_successful_replacement_rebases_result_and_removes_old_instance(self) -> None:
        version_json, payloads = _version_json()
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            http = FakeHttp(payloads)
            installer = MinecraftInstaller(config, repository=FakeRepository(version_json), http=http)
            target = config.game_dir / "instances" / "existing"
            target.mkdir(parents=True)
            (target / "old.txt").write_text("old", encoding="utf-8")

            with patch("nivora_launcher.installer._rename_exchange", return_value=False):
                result = installer.install("1.20.1", instance_name="existing", replace_existing=True)

            self.assertEqual(result.instance_dir, target)
            self.assertEqual(result.client_jar, target / "versions" / "1.20.1" / "1.20.1.jar")
            self.assertEqual(result.libraries, [target / "libraries" / "example/library/1.0/library-1.0.jar"])
            self.assertEqual(result.assets_root, target / "assets")
            self.assertTrue(result.client_jar.is_file())
            self.assertFalse((target / "old.txt").exists())
            self.assertEqual(list((config.game_dir / "instances").glob(".trial-stage-*")), [])
            self.assertEqual(list((config.game_dir / "instances").glob(".trial-backup-*")), [])

    def test_existing_instance_repairs_corrupt_and_missing_core_files_then_skips_healthy_files(self) -> None:
        version_json, payloads = _version_json()
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            http = FakeHttp(payloads)
            installer = MinecraftInstaller(config, repository=FakeRepository(version_json), http=http)
            installed = installer.install("1.20.1", instance_name="existing", assets=True)
            target = installed.instance_dir
            library_path = target / "libraries" / "example/library/1.0/library-1.0.jar"
            asset_index = json.loads(payloads[INDEX_URL].decode("utf-8"))
            asset_digest = next(iter(asset_index["objects"].values()))["hash"]
            asset_path = target / "assets" / "objects" / asset_digest[:2] / asset_digest
            index_path = target / "assets" / "indexes" / "1.20.1.json"

            http.network_calls.clear()
            _rewrite_corrupt(installed.client_jar, b"client-bad!")
            _rewrite_corrupt(library_path, b"library-bad!")
            _rewrite_corrupt(asset_path, b"asset-bad!")
            index_bytes = index_path.read_bytes()
            _rewrite_corrupt(index_path, b"X" + index_bytes[1:])

            repaired = installer.load_existing(target, "1.20.1")

            self.assertEqual(repaired.client_jar.read_bytes(), payloads[CLIENT_URL])
            self.assertEqual(library_path.read_bytes(), payloads[LIBRARY_URL])
            self.assertEqual(asset_path.read_bytes(), payloads[f"https://resources.download.minecraft.net/{asset_digest[:2]}/{asset_digest}"])
            self.assertEqual(index_path.read_bytes(), payloads[INDEX_URL])
            self.assertCountEqual(http.network_calls, [CLIENT_URL, LIBRARY_URL, INDEX_URL, f"https://resources.download.minecraft.net/{asset_digest[:2]}/{asset_digest}"])

            http.download_calls.clear()
            http.network_calls.clear()
            with patch("nivora_launcher.installer.verify_file", side_effect=AssertionError("healthy cache should not be hashed")):
                installer.load_existing(target, "1.20.1")
            self.assertEqual(http.download_calls, [])
            self.assertEqual(http.network_calls, [])

            http.network_calls.clear()
            repaired.client_jar.unlink()
            library_path.unlink()
            asset_path.unlink()
            installer.load_existing(target, "1.20.1")
            self.assertCountEqual(http.network_calls, [CLIENT_URL, LIBRARY_URL, f"https://resources.download.minecraft.net/{asset_digest[:2]}/{asset_digest}"])
            self.assertTrue(repaired.client_jar.is_file())
            self.assertTrue(library_path.is_file())
            self.assertTrue(asset_path.is_file())

    def test_managed_file_without_integrity_metadata_downloads_without_hash_kwargs(self) -> None:
        # Регрессия: у файла нет ни sha1/sha256/sha512, ни size →
        # _integrity_metadata возвращает None, и вызов
        # download(url, destination, **None) падал с TypeError
        # «argument after ** must be a mapping, not NoneType»
        # («Не удалось запустить игру»). Теперь None-safe: файл просто
        # скачивается без контрольных сумм.
        url = "https://launcher.example.invalid/extra.bin"
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            http = FakeHttp({url: b"payload"})
            installer = MinecraftInstaller(config, repository=FakeRepository({}), http=http)
            instance = config.paths.instances_dir / "bare"
            instance.mkdir(parents=True, exist_ok=True)
            destination = instance / "extra.bin"
            downloaded, verified = installer._download_managed_file(
                instance,
                {},
                url,
                destination,
                {"url": url, "path": "extra.bin"},
                repair_only=False,
            )
            self.assertTrue(downloaded)
            self.assertFalse(verified)
            self.assertEqual(destination.read_bytes(), b"payload")


if __name__ == "__main__":
    unittest.main()
