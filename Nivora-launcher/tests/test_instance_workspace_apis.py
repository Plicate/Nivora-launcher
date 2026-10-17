from __future__ import annotations

import base64
import json
import os
import tempfile
import time
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.instance_files import (
    MAX_INSTANCE_LOG_READ_BYTES,
    MAX_INSTANCE_PREVIEW_BYTES,
    MAX_INSTANCE_TEXT_BYTES,
    MAX_INSTANCE_UPLOAD_BYTES,
    MIN_INSTANCE_LOG_READ_BYTES,
    InstanceFiles,
    instance_log_hash,
)
from nivora_launcher.instance_workspace import DEFAULT_CONSOLE_COLORS, PROJECT_METADATA_SUFFIX, InstanceWorkspace
from nivora_launcher.instances import INSTANCE_METADATA_FILE, InstanceStore
from nivora_launcher.launcher import _write_live_state
from nivora_launcher.process_guard import InstanceProcessLock


class InstanceWorkspaceApiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.environment = patch.dict(
            os.environ,
            {
                "XDG_CONFIG_HOME": str(self.root / "config"),
                "XDG_DATA_HOME": str(self.root / "data"),
                "XDG_CACHE_HOME": str(self.root / "cache"),
            },
            clear=False,
        )
        self.environment.start()
        self.config = ConfigStore(Paths())
        self.config.set_value("game_dir", str(self.root / "game"))
        self.instances = InstanceStore(self.config)
        self.instance_root = self.make_instance("alpha")
        self.files = InstanceFiles(self.config, self.instances)

    def tearDown(self) -> None:
        self.environment.stop()
        self.temporary.cleanup()

    def make_instance(self, name: str, *, kind: str = "client", version: str = "1.21.4") -> Path:
        path = self.instances.root / name
        path.mkdir(parents=True, exist_ok=True)
        (path / INSTANCE_METADATA_FILE).write_text(
            json.dumps({"kind": kind, "display_name": name}),
            encoding="utf-8",
        )
        if version:
            version_dir = path / "versions" / version
            version_dir.mkdir(parents=True, exist_ok=True)
            (version_dir / f"{version}.json").write_text(json.dumps({"id": version}), encoding="utf-8")
        return path

    def new_log(self, name: str, text: str) -> Path:
        component = name
        path = self.config.paths.logs_dir / (
            f"{component}-{instance_log_hash(name)}-1.21.4-{time.time_ns()}-{uuid.uuid4().hex[:10]}.log"
        )
        path.write_text(text, encoding="utf-8")
        return path

    def test_listing_is_client_rooted_and_does_not_create_missing_directories(self) -> None:
        (self.instance_root / "config").mkdir()
        (self.instance_root / "readme.txt").write_text("hello", encoding="utf-8")
        files = self.files.list_files("alpha")
        self.assertEqual({item["name"] for item in files}, {"config", "readme.txt", "versions"})
        readme = next(item for item in files if item["name"] == "readme.txt")
        self.assertEqual(readme, {
            "name": "readme.txt",
            "path": "readme.txt",
            "kind": "file",
            "size": 5,
            "modified_at": readme["modified_at"],
            "editable": True,
        })
        with self.assertRaises(LauncherError):
            self.files.list_files("alpha", "missing/nested")
        self.assertFalse((self.instance_root / "missing").exists())

    def test_traversal_absolute_and_reserved_paths_are_rejected_or_hidden(self) -> None:
        outside = self.root / "outside.txt"
        outside.write_text("outside", encoding="utf-8")
        for path in ("../outside.txt", "/etc/passwd", "C:/outside.txt", "./readme.txt", "a/../outside.txt"):
            with self.subTest(path=path), self.assertRaises(LauncherError):
                self.files.read_file("alpha", path)
        for path in (".trial-cell/secrets.json", ".trial-launcher.json", "nested/.trial-launcher-live.json"):
            with self.subTest(path=path), self.assertRaises(LauncherError):
                self.files.write_file("alpha", path, "blocked")
        (self.instance_root / ".trial-cell").mkdir()
        (self.instance_root / ".trial-cell" / "secret").write_text("secret", encoding="utf-8")
        (self.instance_root / ".trial-launcher-private").write_text("private", encoding="utf-8")
        names = {item["name"] for item in self.files.list_files("alpha")}
        self.assertNotIn(".trial-cell", names)
        self.assertNotIn(".trial-launcher-private", names)
        self.assertEqual(outside.read_text(encoding="utf-8"), "outside")

    def test_symlinks_and_special_files_are_never_followed(self) -> None:
        outside = self.root / "outside.txt"
        outside.write_text("safe", encoding="utf-8")
        (self.instance_root / "escape.txt").symlink_to(outside)
        (self.instance_root / "escape-dir").symlink_to(self.root, target_is_directory=True)
        os.mkfifo(self.instance_root / "pipe")
        names = {item["name"] for item in self.files.list_files("alpha")}
        self.assertNotIn("escape.txt", names)
        self.assertNotIn("escape-dir", names)
        self.assertNotIn("pipe", names)
        for path in ("escape.txt", "escape-dir/outside.txt", "pipe"):
            with self.subTest(path=path), self.assertRaises(LauncherError):
                self.files.read_file("alpha", path)
        with self.assertRaises(LauncherError):
            self.files.delete("alpha", "escape.txt")
        with self.assertRaises(LauncherError):
            self.files.write_file("alpha", "escape-dir/new.txt", "blocked")
        self.assertEqual(outside.read_text(encoding="utf-8"), "safe")

    def test_mutations_require_the_instance_lock_and_reject_running_games(self) -> None:
        lock = InstanceProcessLock(self.config.paths.data_home, "alpha")
        lock.acquire()
        try:
            with self.assertRaises(LauncherError):
                self.files.write_file("alpha", "locked.txt", "blocked")
        finally:
            lock.release()

        (self.instance_root / ".trial-launcher-live.json").write_text(
            json.dumps({"running": True, "pid": os.getpid()}),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(LauncherError, "запущенного"):
            self.files.mkdir("alpha", "while-running")
        self.assertFalse((self.instance_root / "while-running").exists())

    def test_text_image_and_upload_limits_are_enforced(self) -> None:
        with self.assertRaises(LauncherError):
            self.files.write_file("alpha", "too-large.txt", "x" * (MAX_INSTANCE_TEXT_BYTES + 1))
        (self.instance_root / "large.txt").write_bytes(b"x" * (MAX_INSTANCE_TEXT_BYTES + 1))
        with self.assertRaises(LauncherError):
            self.files.read_file("alpha", "large.txt")

        image_bytes = b"\x89PNG\r\n\x1a\n" + b"image-data"
        (self.instance_root / "preview.png").write_bytes(image_bytes)
        result = self.files.read_file("alpha", "preview.png", "base64")
        self.assertEqual(result["encoding"], "base64")
        self.assertEqual(result["mime_type"], "image/png")
        self.assertEqual(base64.b64decode(result["content"], validate=True), image_bytes)
        with self.assertRaises(LauncherError):
            self.files.read_file("alpha", "large.txt", "base64")

        too_large_image = self.instance_root / "huge.png"
        with too_large_image.open("wb") as handle:
            handle.write(b"\x89PNG\r\n\x1a\n")
            handle.truncate(MAX_INSTANCE_PREVIEW_BYTES + 1)
        with self.assertRaises(LauncherError):
            self.files.read_file("alpha", "huge.png", "base64")
        with self.assertRaises(LauncherError):
            self.files.upload("alpha", "oversized.bin", b"x" * (MAX_INSTANCE_UPLOAD_BYTES + 1))
        self.assertFalse((self.instance_root / "oversized.bin").exists())

    def test_write_upload_mkdir_and_delete_round_trip(self) -> None:
        self.files.mkdir("alpha", "workspace")
        self.files.write_file("alpha", "workspace/note.txt", "hello")
        self.files.upload("alpha", "workspace/blob.bin", b"\x00\x01")
        self.assertEqual(self.files.read_file("alpha", "workspace/note.txt")["content"], "hello")
        self.assertEqual((self.instance_root / "workspace/blob.bin").read_bytes(), b"\x00\x01")
        self.files.delete("alpha", "workspace")
        self.assertFalse((self.instance_root / "workspace").exists())

    def test_server_instances_are_not_exposed_by_client_file_api(self) -> None:
        self.make_instance("server-one", kind="server")
        with self.assertRaises(LauncherError):
            self.files.list_files("server-one")

    def test_catalog_install_status_and_removal_are_scoped_to_the_selected_catalog(self) -> None:
        mods = self.instance_root / "mods"
        mods.mkdir()
        for file_name, catalog_id in (("modrinth.jar", "modrinth"), ("custom.jar", "custom-catalog")):
            target = mods / file_name
            target.write_bytes(file_name.encode())
            target.with_name(f".{file_name}{PROJECT_METADATA_SUFFIX}").write_text(
                json.dumps({
                    "source": "Modrinth",
                    "catalog": "modrinth",
                    "catalog_id": catalog_id,
                    "project_id": "same-project",
                    "project_type": "mod",
                }),
                encoding="utf-8",
            )
        workspace = InstanceWorkspace(self.config, self.instances)

        self.assertEqual(workspace.installed_projects("alpha", "mod", catalog_id="modrinth"), {"same-project"})
        self.assertEqual(workspace.installed_projects("alpha", "mod", catalog_id="custom-catalog"), {"same-project"})
        _details, removed = workspace.remove_installed_project(
            "alpha",
            "mod",
            "same-project",
            catalog_id="modrinth",
        )

        self.assertEqual(removed, 1)
        self.assertFalse((mods / "modrinth.jar").exists())
        self.assertTrue((mods / "custom.jar").exists())
        self.assertEqual(workspace.installed_projects("alpha", "mod", catalog_id="custom-catalog"), {"same-project"})

    def test_console_colors_default_validate_and_round_trip(self) -> None:
        workspace = InstanceWorkspace(self.config, self.instances)
        self.assertEqual(workspace.settings("alpha")["console_colors"], DEFAULT_CONSOLE_COLORS)
        saved = workspace.save_settings(
            "alpha",
            {
                "console_colors": {
                    "error": "#Ab12Cd",
                    "info": "red",
                    "warning": "#0011FF",
                    "unused": "#ffffff",
                }
            },
        )
        expected = {"error": "#ab12cd", "info": "#e6edf7", "warning": "#0011ff"}
        self.assertEqual(saved["console_colors"], expected)
        self.assertEqual(InstanceWorkspace(self.config, self.instances).settings("alpha")["console_colors"], expected)
        partial = workspace.save_settings("alpha", {"console_colors": {"error": "#ABCDEF"}})
        self.assertEqual(partial["console_colors"], {"error": "#abcdef", "info": "#e6edf7", "warning": "#0011ff"})

    def test_gui_handler_dispatches_instance_workspace_operations(self) -> None:
        context = (self.config, object(), None, None, None, None, self.instances)
        log = self.new_log("alpha", "archived log")
        encoded_upload = base64.b64encode(b"uploaded").decode("ascii")
        with patch("nivora_launcher.gui_service._context", return_value=context), \
            patch("nivora_launcher.gui_service.PluginStore"), \
            patch("nivora_launcher.gui_service.ServerInstanceStore"), \
            patch("nivora_launcher.gui_service.ServerFiles"), \
            patch("nivora_launcher.gui_service.ServerProcessManager"):
            self.assertEqual(handle({"operation": "instance_file_mkdir", "name": "alpha", "path": "workspace"}), {"created": True})
            self.assertEqual(
                handle({"operation": "instance_file_write", "name": "alpha", "path": "workspace/note.txt", "content": "hello"}),
                {"saved": True},
            )
            read = handle({"operation": "instance_file_read", "name": "alpha", "path": "workspace/note.txt"})
            self.assertEqual(read["content"], "hello")
            self.assertEqual(handle({"operation": "instance_file_upload", "name": "alpha", "path": "workspace/blob.bin", "data": encoded_upload}), {"uploaded": True})
            self.assertEqual((self.instance_root / "workspace/blob.bin").read_bytes(), b"uploaded")
            archives = handle({"operation": "instance_log_archives", "name": "alpha"})
            self.assertEqual([item["id"] for item in archives["logs"]], [log.name])
            archived = handle({"operation": "instance_log_archive_read", "name": "alpha", "log_id": log.name})
            self.assertEqual(archived["content"], "archived log")
            settings = handle({"operation": "save_instance_settings", "name": "alpha", "values": {"console_colors": {"error": "#ABCDEF"}}})
            self.assertEqual(settings["settings"]["console_colors"]["error"], "#abcdef")

    def test_archived_logs_are_isolated_and_legacy_names_are_exactly_associated(self) -> None:
        self.make_instance("alpha-long")
        first_log = self.new_log("alpha", "first log")
        other_log = self.new_log("alpha-long", "other log")
        legacy_log = self.config.paths.logs_dir / f"alpha-1.21.4-123456789-abcdef1234.log"
        legacy_log.write_text("legacy log", encoding="utf-8")

        first = self.files.list_log_archives("alpha")
        other = self.files.list_log_archives("alpha-long")
        first_ids = {item["id"] for item in first["logs"]}
        other_ids = {item["id"] for item in other["logs"]}
        self.assertEqual(first_ids, {first_log.name, legacy_log.name})
        self.assertEqual(other_ids, {other_log.name})
        self.assertIsNone(first["next_offset"])
        self.assertEqual(self.files.read_log_archive("alpha", legacy_log.name)["content"], "legacy log")
        with self.assertRaises(LauncherError):
            self.files.read_log_archive("alpha", other_log.name)

    def test_archived_log_reads_are_bounded_and_log_ids_are_basenames(self) -> None:
        log = self.new_log("alpha", "x" * (MAX_INSTANCE_LOG_READ_BYTES + 4096))
        first = self.files.read_log_archive("alpha", log.name, limit=65_536)
        self.assertEqual(len(first["content"].encode("utf-8")), 65_536)
        self.assertEqual(first["next_offset"], 65_536)
        self.assertFalse(first["eof"])
        second = self.files.read_log_archive("alpha", log.name, first["next_offset"], MAX_INSTANCE_LOG_READ_BYTES)
        self.assertEqual(len(second["content"].encode("utf-8")), 4096 + MAX_INSTANCE_LOG_READ_BYTES - 65_536)
        self.assertTrue(second["eof"])
        with self.assertRaises(LauncherError):
            self.files.read_log_archive("alpha", f"../{log.name}")
        with self.assertRaises(LauncherError):
            self.files.read_log_archive("alpha", log.name, limit=MAX_INSTANCE_LOG_READ_BYTES + 1)
        with self.assertRaises(LauncherError):
            self.files.read_log_archive("alpha", log.name, limit=MIN_INSTANCE_LOG_READ_BYTES - 1)

    def test_live_log_tail_reads_persisted_archive_and_bounds_lines(self) -> None:
        log = self.new_log("alpha", "".join(f"line {index}\n" for index in range(605)))
        self.assertEqual(
            self.files.read_log_tail("alpha", log.name),
            [f"line {index}" for index in range(105, 605)],
        )

    def test_instance_log_reads_the_live_archive_after_a_worker_restart(self) -> None:
        log = self.new_log("alpha", "first line\nsecond line\n")
        _write_live_state(
            self.instance_root / ".trial-launcher-live.json",
            running=False,
            exit_code=1,
            pid=0,
            log_id=log.name,
        )
        context = (self.config, object(), None, None, None, None, self.instances)
        with patch("nivora_launcher.gui_service._context", return_value=context), \
            patch("nivora_launcher.gui_service.PluginStore"), \
            patch("nivora_launcher.gui_service.ServerInstanceStore"), \
            patch("nivora_launcher.gui_service.ServerFiles"), \
            patch("nivora_launcher.gui_service.ServerProcessManager"), \
            patch("nivora_launcher.gui_service._session_snapshot", return_value=([], False)):
            result = handle({"operation": "instance_log", "name": "alpha"})

        self.assertEqual(result["lines"], ["first line", "second line"])
        self.assertFalse(result["running"])
        self.assertEqual(json.loads((self.instance_root / ".trial-launcher-live.json").read_text())["log_id"], log.name)

    def test_archive_listing_paginates_and_skips_log_symlinks(self) -> None:
        first = self.new_log("alpha", "one")
        second = self.new_log("alpha", "two")
        linked = self.config.paths.logs_dir / (
            f"alpha-{instance_log_hash('alpha')}-1.21.4-123456789-abcdef1234.log"
        )
        linked.symlink_to(first)
        page = self.files.list_log_archives("alpha", limit=1)
        self.assertEqual(len(page["logs"]), 1)
        self.assertEqual(page["next_offset"], 1)
        rest = self.files.list_log_archives("alpha", offset=page["next_offset"], limit=1)
        self.assertEqual(len(rest["logs"]), 1)
        ids = {item["id"] for item in page["logs"] + rest["logs"]}
        self.assertEqual(ids, {first.name, second.name})
        with self.assertRaises(LauncherError):
            self.files.read_log_archive("alpha", linked.name)


if __name__ == "__main__":
    unittest.main()
