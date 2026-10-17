from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
import zipfile
from contextlib import ExitStack, contextmanager
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from urllib.error import HTTPError

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import IntegrityError, LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.installer import MinecraftInstaller
from nivora_launcher.modpacks import ModpackImporter
from nivora_launcher.net import HttpClient, verify_file


URL = "https://mirror.example.invalid/file.jar"
PAYLOAD = b"0123456789ab"


def _sha1(payload: bytes) -> str:
    return hashlib.sha1(payload).hexdigest()


class FakeResponse:
    def __init__(self, payload: bytes, *, status: int = 200, fail_after: int | None = None, eof_at: int | None = None) -> None:
        self.payload = payload
        self.status = status
        self.headers = {"Content-Length": str(len(payload))}
        self.fail_after = fail_after
        self.eof_at = eof_at
        self.position = 0

    def __enter__(self) -> "FakeResponse":
        return self

    def __exit__(self, *_args) -> None:
        return None

    def read(self, size: int) -> bytes:
        if self.fail_after is not None and self.position >= self.fail_after:
            raise OSError("connection reset by peer")
        if self.eof_at is not None and self.position >= self.eof_at:
            return b""
        chunk = self.payload[self.position : self.position + min(size, 4)]
        self.position += len(chunk)
        return chunk


class FakeOpen:
    def __init__(self, script: list) -> None:
        self.script = list(script)
        self.requests: list[dict[str, str]] = []

    def __call__(self, request, *, redirect_validator=None):
        if not self.script:
            raise AssertionError("unexpected extra request")
        self.requests.append({key.lower(): value for key, value in request.header_items()})
        entry = self.script.pop(0)
        if isinstance(entry, Exception):
            raise entry
        return entry


class DownloadResumeTests(unittest.TestCase):
    def _download(self, script: list, *, retries: int = 3, sha1: str | None = None, size: int | None = len(PAYLOAD)):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        destination = Path(directory.name) / "file.jar"
        client = HttpClient(timeout=1, retries=retries)
        opener = FakeOpen(script)
        client._open = opener  # type: ignore[method-assign]
        client.wait = lambda _seconds: None  # type: ignore[method-assign]
        with patch("nivora_launcher.net._validate_url", return_value=()):
            result = client.download(URL, destination, sha1=sha1 or _sha1(PAYLOAD), size=size)
        return result, destination, opener

    def test_dropped_connection_resumes_with_range(self) -> None:
        result, destination, opener = self._download([
            FakeResponse(PAYLOAD, fail_after=4),
            FakeResponse(PAYLOAD[4:], status=206),
        ])

        self.assertTrue(result)
        self.assertEqual(destination.read_bytes(), PAYLOAD)
        self.assertNotIn("range", opener.requests[0])
        self.assertEqual(opener.requests[1]["range"], "bytes=4-")
        self.assertFalse(destination.with_name(".file.jar.part").exists())

    def test_early_eof_is_resumed_instead_of_restarted(self) -> None:
        result, destination, opener = self._download([
            FakeResponse(PAYLOAD, eof_at=8),
            FakeResponse(PAYLOAD[8:], status=206),
        ])

        self.assertTrue(result)
        self.assertEqual(destination.read_bytes(), PAYLOAD)
        self.assertEqual(opener.requests[1]["range"], "bytes=8-")

    def test_mirror_ignoring_range_downloads_whole_file_again(self) -> None:
        result, destination, _opener = self._download([
            FakeResponse(PAYLOAD, fail_after=4),
            FakeResponse(PAYLOAD, status=200),
        ])

        self.assertTrue(result)
        self.assertEqual(destination.read_bytes(), PAYLOAD)

    def test_416_on_resume_discards_partial_and_restarts(self) -> None:
        result, destination, opener = self._download([
            FakeResponse(PAYLOAD, fail_after=4),
            HTTPError(URL, 416, "Range Not Satisfiable", None, None),
            FakeResponse(PAYLOAD),
        ])

        self.assertTrue(result)
        self.assertEqual(destination.read_bytes(), PAYLOAD)
        self.assertNotIn("range", opener.requests[2])

    def test_corrupted_payload_gets_one_clean_retry(self) -> None:
        result, destination, _opener = self._download([
            FakeResponse(b"XXXXXXXXXXXX"),
            FakeResponse(PAYLOAD),
        ])

        self.assertTrue(result)
        self.assertEqual(destination.read_bytes(), PAYLOAD)

    def test_repeated_corruption_raises_integrity_error(self) -> None:
        with self.assertRaises(IntegrityError):
            self._download([FakeResponse(b"XXXXXXXXXXXX"), FakeResponse(b"YYYYYYYYYYYY"), FakeResponse(b"ZZZZZZZZZZZZ")])

    def test_exhausted_retries_raise_launcher_error(self) -> None:
        with self.assertRaisesRegex(LauncherError, "Не удалось скачать"):
            self._download([FakeResponse(PAYLOAD, fail_after=0) for _ in range(3)])


class RecordingHttp:
    def __init__(self) -> None:
        self.urls: list[str] = []

    def download(self, url: str, destination: Path, **_kwargs) -> bool:
        self.urls.append(url)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(b"downloaded")
        return True


def _write_zip(path: Path, entries: dict[str, bytes | str]) -> None:
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in entries.items():
            archive.writestr(name, content)


class ModpackLoaderProtectionTests(unittest.TestCase):
    def _import(self, root: Path, entries: dict[str, bytes | str], http=None):
        archive = root / "pack.mrpack"
        _write_zip(archive, entries)
        target = root / "instance"
        target.mkdir(exist_ok=True)
        result = ModpackImporter(http or RecordingHttp()).import_file(
            archive,
            target,
            source="local",
            project_id="pack",
            display_name="Pack",
            version="1.0",
            confirm_unverified=True,
        )
        return target, result

    def test_overrides_cannot_replace_loader_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "instance"
            version_json = target / "versions" / "1.20.1-fabric" / "1.20.1-fabric.json"
            version_json.parent.mkdir(parents=True)
            version_json.write_text('{"id": "1.20.1-fabric"}', encoding="utf-8")

            target, result = self._import(root, {
                "modrinth.index.json": json.dumps({"formatVersion": 1, "files": []}),
                "overrides/versions/1.20.1-fabric/1.20.1-fabric.json": "{}",
                "overrides/libraries/evil/evil.jar": b"evil",
                "overrides/natives/evil.so": b"evil",
                "overrides/bin/modpack.jar": b"evil",
                "overrides/mods/good.jar": b"good",
                "overrides/config/good.cfg": "ok=true",
            })

            self.assertEqual(version_json.read_text(encoding="utf-8"), '{"id": "1.20.1-fabric"}')
            self.assertFalse((target / "libraries").exists())
            self.assertFalse((target / "natives").exists())
            self.assertFalse((target / "bin").exists())
            self.assertEqual((target / "mods" / "good.jar").read_bytes(), b"good")
            self.assertEqual(
                sorted(result["skipped_loader_files"]),
                ["bin/modpack.jar", "libraries/evil/evil.jar", "natives/evil.so", "versions/1.20.1-fabric/1.20.1-fabric.json"],
            )

    def test_multimc_export_is_mapped_into_the_instance(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target, result = self._import(Path(directory), {
                "modrinth.index.json": json.dumps({"formatVersion": 1, "files": []}),
                "mmc-pack.json": "{}",
                "instance.cfg": "name=Pack",
                ".minecraft/mods/foo.jar": b"foo",
                ".minecraft/versions/x/x.json": "{}",
            })

            self.assertEqual((target / "mods" / "foo.jar").read_bytes(), b"foo")
            self.assertFalse((target / ".minecraft").exists())
            self.assertFalse((target / "mmc-pack.json").exists())
            self.assertFalse((target / "instance.cfg").exists())
            self.assertEqual(result["skipped_loader_files"], ["versions/x/x.json"])

    def test_index_files_targeting_loader_dirs_are_not_downloaded(self) -> None:
        http = RecordingHttp()
        index = {
            "formatVersion": 1,
            "files": [
                {"path": "versions/evil/evil.json", "downloads": ["https://cdn.modrinth.com/evil.json"], "hashes": {}},
                {"path": "mods/ok.jar", "downloads": ["https://cdn.modrinth.com/ok.jar"], "hashes": {}},
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            target, _result = self._import(Path(directory), {"modrinth.index.json": json.dumps(index)}, http=http)

            self.assertEqual(http.urls, ["https://cdn.modrinth.com/ok.jar"])
            self.assertFalse((target / "versions").exists())
            self.assertTrue((target / "mods" / "ok.jar").is_file())


CLIENT_URL = "https://launcher.example.invalid/client.jar"
LIBRARY_URL = "https://launcher.example.invalid/library.jar"


def _base_version() -> tuple[dict, dict[str, bytes]]:
    client = b"client-good"
    library = b"library-good"
    version = {
        "id": "1.20.1",
        "mainClass": "example.Main",
        "downloads": {"client": {"url": CLIENT_URL, "sha1": _sha1(client), "size": len(client)}},
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
    }
    return version, {CLIENT_URL: client, LIBRARY_URL: library}


class FakeRepository:
    def __init__(self, version_json: dict) -> None:
        self.version_json = version_json

    def resolve(self, _version_id: str) -> dict:
        return json.loads(json.dumps(self.version_json))

    def library_url(self, library: dict, classifier: str | None = None) -> str:
        return str(library["downloads"]["artifact"]["url"])


class FakeLoaders:
    def profile(self, loader: str, minecraft_version: str, loader_version: str | None = None) -> dict:
        return {"id": f"{minecraft_version}-{loader}-{loader_version or 'test'}", "inheritsFrom": minecraft_version}


class FakeInstallHttp:
    def __init__(self, payloads: dict[str, bytes]) -> None:
        self.payloads = payloads
        self.fail = False

    def download(self, url: str, destination: Path, *, sha1=None, sha256=None, sha512=None, size=None, **_kwargs) -> bool:
        if destination.exists() and verify_file(destination, sha1=sha1, size=size):
            return False
        if self.fail:
            raise LauncherError("simulated download failure")
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(self.payloads[url])
        return True

    def check_cancelled(self) -> None:
        return None


@contextmanager
def _config(root: Path):
    with patch.dict(
        os.environ,
        {"XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"), "XDG_CACHE_HOME": str(root / "cache")},
        clear=False,
    ):
        config = ConfigStore(Paths())
        config.set_value("game_dir", str(root / "game"))
        yield config


class LoaderRepairTests(unittest.TestCase):
    def _broken_fabric_instance(self, config, http: FakeInstallHttp) -> tuple[MinecraftInstaller, Path, Path]:
        version, _payloads = _base_version()
        installer = MinecraftInstaller(config, repository=FakeRepository(version), loaders=FakeLoaders(), http=http)
        installed = installer.install("1.20.1", loader="fabric", instance_name="pack", assets=False)
        instance = installed.instance_dir
        (instance / "mods").mkdir()
        (instance / "mods" / "keep.jar").write_bytes(b"mod")
        metadata = instance / "versions" / installed.version_id / f"{installed.version_id}.json"
        metadata.write_text("{broken", encoding="utf-8")
        return installer, instance, metadata

    def test_repair_recreates_loader_version_and_keeps_mods(self) -> None:
        _version, payloads = _base_version()
        http = FakeInstallHttp(payloads)
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            installer, instance, metadata = self._broken_fabric_instance(config, http)

            result = installer.repair_version(instance, "1.20.1", loader="fabric", loader_version="test", version_id=metadata.stem)

            self.assertEqual(result.version_id, "1.20.1-fabric-test")
            self.assertEqual(json.loads(metadata.read_text(encoding="utf-8"))["mainClass"], "example.Main")
            self.assertEqual((instance / "mods" / "keep.jar").read_bytes(), b"mod")
            self.assertEqual(list((instance / "versions").glob(".trial-repair-*")), [])

    def test_failed_repair_restores_previous_version_directory(self) -> None:
        _version, payloads = _base_version()
        http = FakeInstallHttp(payloads)
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            installer, instance, metadata = self._broken_fabric_instance(config, http)
            (metadata.parent / f"{metadata.stem}.jar").unlink()
            http.fail = True

            with self.assertRaisesRegex(LauncherError, "simulated"):
                installer.repair_version(instance, "1.20.1", loader="fabric", loader_version="test", version_id=metadata.stem)

            self.assertEqual(metadata.read_text(encoding="utf-8"), "{broken")
            self.assertEqual(list((instance / "versions").glob(".trial-repair-*")), [])

    def test_repair_rejects_paths_outside_instances_root(self) -> None:
        version, payloads = _base_version()
        with tempfile.TemporaryDirectory() as directory, _config(Path(directory)) as config:
            installer = MinecraftInstaller(config, repository=FakeRepository(version), loaders=FakeLoaders(), http=FakeInstallHttp(payloads))
            outside = Path(directory) / "elsewhere"
            outside.mkdir()
            with self.assertRaisesRegex(LauncherError, "Недопустимый путь"):
                installer.repair_version(outside, "1.20.1")


class RepairOperationTests(unittest.TestCase):
    def _handle(self, request: dict, *, info, installer, data_home: Path):
        config = SimpleNamespace(settings=SimpleNamespace(), paths=SimpleNamespace(data_home=data_home))
        instances = SimpleNamespace(get=lambda _name: info, list=lambda: [])
        patches = [
            patch("nivora_launcher.gui_service._context", return_value=(config, object(), None, None, installer, None, instances)),
            patch("nivora_launcher.gui_service.InstanceWorkspace", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.PluginStore", return_value=SimpleNamespace(list=lambda: [])),
            patch("nivora_launcher.gui_service.ServerInstanceStore", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.ServerFiles", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.ServerProcessManager", return_value=SimpleNamespace()),
        ]
        with ExitStack() as stack:
            for patcher in patches:
                stack.enter_context(patcher)
            return handle(request)

    def _info(self, path: Path, **overrides):
        values = {
            "name": "pack",
            "path": path,
            "kind": "client",
            "game_edition": "java",
            "running": False,
            "loader": "fabric",
            "minecraft_version": "1.20.1",
            "version_id": "1.20.1-fabric-0.15.11",
        }
        values.update(overrides)
        return SimpleNamespace(**values)

    def test_operation_reinstalls_with_the_instance_loader(self) -> None:
        calls: list[tuple] = []

        def repair_version(path, minecraft_version, **kwargs):
            calls.append((path, minecraft_version, kwargs))
            return SimpleNamespace(version_id="1.20.1-fabric-0.15.11", instance_dir=path)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "pack"
            path.mkdir()
            result = self._handle(
                {"operation": "instance_repair_version", "instance_name": "pack"},
                info=self._info(path),
                installer=SimpleNamespace(repair_version=repair_version),
                data_home=Path(directory) / "data",
            )

        self.assertEqual(result["version_id"], "1.20.1-fabric-0.15.11")
        self.assertEqual(calls[0][1], "1.20.1")
        self.assertEqual(calls[0][2], {"loader": "fabric", "loader_version": "0.15.11", "version_id": "1.20.1-fabric-0.15.11"})

    def test_operation_refuses_running_and_server_instances(self) -> None:
        installer = SimpleNamespace(repair_version=lambda *_args, **_kwargs: self.fail("must not repair"))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "pack"
            path.mkdir()
            for overrides, message in (({"running": True}, "Остановите"), ({"kind": "server"}, "клиентских")):
                with self.assertRaisesRegex(LauncherError, message):
                    self._handle(
                        {"operation": "instance_repair_version", "instance_name": "pack"},
                        info=self._info(path, **overrides),
                        installer=installer,
                        data_home=Path(directory) / "data",
                    )


if __name__ == "__main__":
    unittest.main()
