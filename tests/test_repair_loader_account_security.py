"""Авто-починка: модлоадер, аккаунты, песочница; импорт миров; Windows-запуск."""
from __future__ import annotations

import base64
import io
import json
import os
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher import world_import
from nivora_launcher.auto_repair import RepairContext, apply_fixes, diagnose, pick_loader_version
from nivora_launcher.errors import LauncherError
from nivora_launcher.installer import _ensure_launcher_profiles, loader_installer_command
from nivora_launcher import launcher as launcher_module
from nivora_launcher.launcher import _should_use_argument_file, _windows_ansi_command

STRICT = {
    "memory": "2GiB", "java_path": "", "jvm_arguments": [], "jvm_preset": "none", "graphics_mode": "auto",
    "security_profile": "strict", "security_permissions": ["network:game", "files:instance", "desktop:display", "desktop:audio"],
}


def make_mod(directory: Path, file_name: str, mod_id: str, classes: list[str] = ()) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(directory / file_name, "w") as archive:
        archive.writestr("fabric.mod.json", json.dumps({"id": mod_id, "schemaVersion": 1}))
        for name in classes or ["dummy/Dummy.class"]:
            archive.writestr(name, b"\xca\xfe\xba\xbe")


def kinds(report: dict) -> list[str]:
    return [issue["id"] for issue in report["issues"]]


def issue(report: dict, kind: str) -> dict:
    return next(item for item in report["issues"] if item["id"] == kind)


def actions(report: dict, kind: str) -> list[str]:
    return [item["action"] for item in issue(report, kind)["fixes"]]


class DiagnoseBase(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.instance = Path(self.temporary.name) / "alpha"
        (self.instance / "mods").mkdir(parents=True)
        self.java = patch("nivora_launcher.runtime.discover_java_paths", return_value=[])
        self.java.start()

    def tearDown(self) -> None:
        self.java.stop()
        self.temporary.cleanup()

    def run_diagnose(self, lines: list[str], **kwargs) -> dict:
        kwargs.setdefault("loader", "fabric")
        kwargs.setdefault("instance_settings", dict(STRICT))
        return diagnose(lines, kwargs.pop("exit_code", 1), instance_path=self.instance, crash_report=kwargs.pop("crash_report", ""), **kwargs)


class LoaderTests(DiagnoseBase):
    def test_fabric_loader_too_old_offers_update_not_mod_install(self) -> None:
        make_mod(self.instance / "mods", "sodium.jar", "sodium")
        report = self.run_diagnose([
            "- Mod 'Sodium' (sodium) 0.5.8 requires version 0.15.0 or later of 'Fabric Loader' (fabricloader), but only the wrong version is present: 0.14.21!",
        ], loader_version="0.14.21", minecraft_version="1.20.1")
        self.assertEqual(kinds(report), ["loader_outdated"])
        update = issue(report, "loader_outdated")["fixes"][0]
        self.assertEqual(update["action"], "update_loader")
        self.assertEqual(update["params"], {"minimum": "0.15.0"})
        self.assertIn("sodium.jar", issue(report, "loader_outdated")["fixes"][1]["params"]["files"])

    def test_forge_and_neoforge_loader_requirements(self) -> None:
        report = self.run_diagnose([
            "Missing or unsupported mandatory dependencies:",
            "\tMod ID: 'forge', Requested by: 'create', Expected range: '[47.1.3,)', Actual version: '47.1.0'",
        ], loader="forge")
        self.assertEqual(issue(report, "loader_outdated")["fixes"][0]["params"]["minimum"], "47.1.3")
        report = self.run_diagnose([
            "Mod create requires neoforge 21.1.77 or above",
            "Currently, neoforge is 21.1.50",
        ], loader="neoforge")
        self.assertEqual(issue(report, "loader_outdated")["fixes"][0]["params"]["minimum"], "21.1.77")
        report = self.run_diagnose(["Mod jei requires geckolib 4.0 or above", "Currently, geckolib is not installed"], loader="neoforge")
        self.assertIn("geckolib", issue(report, "missing_dependencies")["fixes"][0]["params"]["ids"])

    def test_broken_forge_installation(self) -> None:
        report = self.run_diagnose(["forge installer завершился с кодом 1:", "There is no minecraft launcher profile"], loader="forge")
        self.assertEqual(actions(report, "loader_broken"), ["repair_version", "update_loader"])
        self.assertNotIn("game_files", kinds(report))

    def test_pick_loader_version(self) -> None:
        fabric = ["0.16.10", "0.16.9", "0.16.0-beta.1", "0.15.11", "0.14.21"]
        self.assertEqual(pick_loader_version(fabric), "0.16.10")
        self.assertEqual(pick_loader_version(fabric, "0.15.0"), "0.16.10")
        self.assertEqual(pick_loader_version(["47.1.0", "47.2.20", "47.3.0"], "47.1.3"), "47.3.0")
        self.assertEqual(pick_loader_version(["0.1.0-beta.2"], ""), "0.1.0-beta.2")
        self.assertEqual(pick_loader_version(["1.0"], "2.0"), "")


class AccountTests(DiagnoseBase):
    accounts = [
        {"account_id": "main", "kind": "microsoft", "username": "Steve", "is_default": True},
        {"account_id": "alt", "kind": "microsoft", "username": "Alex"},
        {"account_id": "offline-debug", "kind": "offline", "username": "Dev"},
    ]

    def test_expired_token_offers_relogin_and_switch(self) -> None:
        report = self.run_diagnose(["Microsoft-токен истёк. Повторите login microsoft"], exit_code=None, accounts=self.accounts, current_account_id="main")
        found = issue(report, "account")
        self.assertEqual(found["fixes"][0]["action"], "login_microsoft")
        switch = [item for item in found["fixes"] if item["action"] == "set_account"]
        self.assertEqual({item["params"]["account_id"] for item in switch}, {"alt", "offline-debug"})
        self.assertEqual(len({item["id"] for item in found["fixes"]}), len(found["fixes"]))

    def test_missing_account_and_xsts(self) -> None:
        report = self.run_diagnose(["Аккаунт не найден: gone"], exit_code=None, accounts=self.accounts, current_account_id="gone")
        self.assertEqual(issue(report, "account")["fixes"][0]["params"]["account_id"], "main")
        report = self.run_diagnose(["Xbox XSTS отклонил аккаунт, код 2148916238"], exit_code=None, accounts=self.accounts, current_account_id="main")
        self.assertIn("детский", issue(report, "account")["manual"])

    def test_offline_account_on_online_server(self) -> None:
        report = self.run_diagnose(["Disconnected: Failed to verify username!"], exit_code=None, accounts=self.accounts, current_account_id="offline-debug")
        found = issue(report, "account")
        self.assertEqual(found["fixes"][0]["action"], "set_account")
        self.assertEqual(found["fixes"][0]["params"]["account_id"], "main")


class SandboxTests(DiagnoseBase):
    def test_launcher_side_sandbox_error_offers_profile_changes(self) -> None:
        settings = {**STRICT, "security_permissions": [*STRICT["security_permissions"], "process:spawn"]}
        report = self.run_diagnose(["Security profile strict не может безопасно выдать разрешения: process:spawn. Используйте off"], exit_code=None, instance_settings=settings)
        fixes = issue(report, "security_sandbox")["fixes"]
        self.assertEqual(fixes[0]["params"]["remove_permissions"], ["process:spawn"])
        self.assertEqual([item["params"]["profile"] for item in fixes[1:]], ["compat", "off"])

    def test_network_blocked_when_permission_missing(self) -> None:
        settings = {**STRICT, "security_permissions": ["files:instance", "desktop:display"]}
        report = self.run_diagnose(["java.net.UnknownHostException: sessionserver.mojang.com"], instance_settings=settings)
        self.assertEqual(issue(report, "security_sandbox")["fixes"][0]["params"]["add_permissions"], ["network:game"])
        # С выданной сетью — это обычная сетевая ошибка, песочница ни при чём.
        report = self.run_diagnose(["java.net.UnknownHostException: sessionserver.mojang.com"])
        self.assertNotIn("security_sandbox", kinds(report))

    def test_host_file_access_points_to_mod_from_stack(self) -> None:
        make_mod(self.instance / "mods", "spy-1.0.jar", "spy", ["com/evil/spy/Grabber.class"])
        make_mod(self.instance / "mods", "fine.jar", "fine", ["org/fine/mod/Main.class"])
        report = self.run_diagnose([
            "java.nio.file.AccessDeniedException: /home/user/.ssh/id_rsa",
            "\tat java.base/sun.nio.fs.UnixException.translateToIOException(UnixException.java:90)",
            "\tat com.evil.spy.Grabber.steal(Grabber.java:12)",
            "\tat net.minecraft.client.Minecraft.run(Minecraft.java:1)",
        ])
        fixes = issue(report, "security_sandbox")["fixes"]
        self.assertEqual(fixes[0]["action"], "disable_mods")
        self.assertEqual(fixes[0]["params"]["files"], ["spy-1.0.jar"])
        self.assertIn("off", [item["params"].get("profile") for item in fixes if item["action"] == "set_security"])

    def test_unsandboxed_profiles_ignore_game_side_patterns(self) -> None:
        report = self.run_diagnose(["java.nio.file.AccessDeniedException: /etc/shadow"], instance_settings={**STRICT, "security_profile": "off"})
        self.assertNotIn("security_sandbox", kinds(report))


class ApplyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.saved: dict = {"security_profile": "strict", "security_permissions": ["files:instance", "process:spawn"]}
        self.calls: list = []
        self.context = RepairContext(
            instance_name="alpha",
            instance_path=Path(self.temporary.name),
            save_settings=self.saved.update,
            settings=lambda: dict(self.saved),
            toggle_mod=lambda _name: None,
            repair_version=lambda: None,
            install_mod=lambda _id: None,
            update_loader=lambda minimum: self.calls.append(("loader", minimum)) or "0.16.10",
            set_account=lambda account_id: self.calls.append(("account", account_id)) or "Alex",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_new_actions(self) -> None:
        results = apply_fixes(self.context, [
            {"id": "a", "action": "set_security", "params": {"add_permissions": ["network:game", "bogus"], "remove_permissions": []}},
            {"id": "b", "action": "set_account", "params": {"account_id": "alt"}},
            {"id": "c", "action": "login_microsoft", "params": {}},
            {"id": "d", "action": "update_loader", "params": {"minimum": "0.15.0"}},
            {"id": "e", "action": "set_account", "params": {"account_id": "../x"}},
        ])
        self.assertEqual([item["ok"] for item in results], [True, True, True, False, True])
        # Неразрешимое в strict разрешение снимается, иначе запуск снова упадёт.
        self.assertEqual(self.saved["security_permissions"], ["files:instance", "network:game"])
        self.assertIn(("loader", "0.15.0"), self.calls)
        self.assertIn(("account", "alt"), self.calls)
        apply_fixes(self.context, [{"action": "set_security", "params": {"profile": "off"}}])
        self.assertEqual(self.saved["security_profile"], "off")
        bad = apply_fixes(self.context, [{"action": "set_security", "params": {"profile": "yolo"}}])
        self.assertFalse(bad[0]["ok"])


class WorldImportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.instance = Path(self.temporary.name) / "inst"
        self.instance.mkdir()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def upload(self, kind: str, label: str, files: dict[str, bytes]) -> list[str]:
        upload_id = world_import.begin(self.instance, kind, label)
        for path, content in files.items():
            for offset in range(0, max(1, len(content)), 7):
                world_import.chunk(self.instance, upload_id, path, content[offset : offset + 7], offset)
        return world_import.finish(self.instance, upload_id)

    def test_zip_with_nested_world_and_flat_world(self) -> None:
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("My World/level.dat", b"nbt")
            archive.writestr("My World/region/r.0.0.mca", b"data")
        self.assertEqual(self.upload("archive", "pack.zip", {"pack.zip": buffer.getvalue()}), ["My World"])
        self.assertTrue((self.instance / "saves" / "My World" / "region" / "r.0.0.mca").is_file())
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("level.dat", b"nbt")
        self.assertEqual(self.upload("archive", "Flat.zip", {"Flat.zip": buffer.getvalue()}), ["Flat"])
        # Повтор не затирает существующий мир.
        self.assertEqual(self.upload("archive", "Flat.zip", {"Flat.zip": buffer.getvalue()}), ["Flat (2)"])
        self.assertFalse(any((self.instance / world_import.STAGING_DIRECTORY).iterdir()))

    def test_tar_gz_and_folder(self) -> None:
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            info = tarfile.TarInfo("saves/A/level.dat")
            info.size = 3
            archive.addfile(info, io.BytesIO(b"nbt"))
            info = tarfile.TarInfo("saves/B/level.dat")
            info.size = 3
            archive.addfile(info, io.BytesIO(b"nbt"))
        self.assertEqual(sorted(self.upload("archive", "w.tar.gz", {"w.tar.gz": buffer.getvalue()})), ["A", "B"])
        self.assertEqual(self.upload("folder", "Castle", {"Castle/level.dat": b"nbt", "Castle/data/x.dat": b"1"}), ["Castle"])

    def test_rejections(self) -> None:
        with self.assertRaises(LauncherError):
            world_import.begin(self.instance, "archive", "world.rar")
        upload_id = world_import.begin(self.instance, "folder", "x")
        for bad in ("../evil", "/abs", "C:/win", "a/../../b"):
            with self.assertRaises(LauncherError):
                world_import.chunk(self.instance, upload_id, bad, b"x", 0)
        with self.assertRaises(LauncherError):
            world_import.chunk(self.instance, upload_id, "w/level.dat", b"x", 5)
        world_import.chunk(self.instance, upload_id, "w/readme.txt", b"x", 0)
        with self.assertRaises(LauncherError):
            world_import.finish(self.instance, upload_id)
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("../../escape/level.dat", b"nbt")
        with self.assertRaises(LauncherError):
            self.upload("archive", "bad.zip", {"bad.zip": buffer.getvalue()})
        self.assertFalse((Path(self.temporary.name) / "escape").exists())


class WindowsLaunchTests(unittest.TestCase):
    def test_argument_file_policy(self) -> None:
        self.assertFalse(_should_use_argument_file(True, 8))
        self.assertTrue(_should_use_argument_file(True, 17))
        with patch("nivora_launcher.launcher.os.name", "nt"):
            self.assertTrue(_should_use_argument_file(False, 21))
            self.assertFalse(_should_use_argument_file(False, 8))
        with patch("nivora_launcher.launcher.os.name", "posix"):
            self.assertFalse(_should_use_argument_file(False, 21))

    def test_cyrillic_user_profile_survives_windows_ansi_encoding(self) -> None:
        # Скриншот пользователя: C:\Users\Месси\… — KnotClient не находился, потому что
        # @argfile писался в UTF-8, а java.exe читает его в cp1251.
        command = [
            r"C:\Users\Месси\AppData\Local\nivora-launcher\runtime\bin\javaw.exe",
            r"-Djava.library.path=C:\Users\Месси\game\natives",
            "-cp",
            r"C:\Users\Месси\game\libraries\fabric-loader.jar;C:\Users\Месси\game\client.jar",
            "net.fabricmc.loader.impl.launch.knot.KnotClient",
        ]
        self.assertEqual(_windows_ansi_command(command, "cp1251", lambda _path: ""), command)
        content = "\n".join(command).encode("cp1251")
        self.assertIn("Месси".encode("cp1251"), content)

    def test_unencodable_paths_become_short_names_or_clear_error(self) -> None:
        shorts = {r"C:\Users\张伟\lib.jar": r"C:\Users\D5A1~1\lib.jar", r"C:\Users\张伟\natives": r"C:\Users\D5A1~1\natives"}
        result = _windows_ansi_command(
            [r"-Djava.library.path=C:\Users\张伟\natives", r"C:\ok.jar;C:\Users\张伟\lib.jar"], "cp1251", lambda path: shorts.get(path, "")
        )
        self.assertEqual(result, [r"-Djava.library.path=C:\Users\D5A1~1\natives", r"C:\ok.jar;C:\Users\D5A1~1\lib.jar"])
        with self.assertRaises(LauncherError):
            _windows_ansi_command([r"C:\Users\张伟\x.jar"], "cp1251", lambda _path: "")

    def test_argument_file_encoding_follows_platform(self) -> None:
        with patch("nivora_launcher.launcher.os.name", "nt"):
            self.assertEqual(launcher_module._argument_file_encoding(), launcher_module._windows_ansi_encoding())
        self.assertEqual(launcher_module._argument_file_encoding(), "utf-8")

    def test_loader_installer_command_and_profiles(self) -> None:
        command = loader_installer_command("java", Path("/i/installer.jar"), Path("/i"))
        self.assertEqual(command[-2:], ["--installClient", str(Path("/i"))])
        self.assertNotIn("--target", command)
        with tempfile.TemporaryDirectory() as directory:
            _ensure_launcher_profiles(Path(directory))
            data = json.loads((Path(directory) / "launcher_profiles.json").read_text(encoding="utf-8"))
            self.assertEqual(data["profiles"], {})


class GuiOperationsTests(unittest.TestCase):
    def setUp(self) -> None:
        from nivora_launcher.config import ConfigStore, Paths
        from nivora_launcher.instances import INSTANCE_METADATA_FILE

        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.environment = patch.dict(os.environ, {
            "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"), "XDG_CACHE_HOME": str(root / "cache"),
        })
        self.environment.start()
        config = ConfigStore(Paths())
        config.set_value("game_dir", str(root / "game"))
        self.instance = config.game_dir / "instances" / "alpha"
        version = "fabric-loader-0.16.10-1.21.4"
        (self.instance / "versions" / version).mkdir(parents=True)
        (self.instance / "versions" / version / f"{version}.json").write_text(json.dumps({"id": version, "inheritsFrom": "1.21.4"}), encoding="utf-8")
        (self.instance / INSTANCE_METADATA_FILE).write_text(json.dumps({"kind": "client", "display_name": "alpha"}), encoding="utf-8")
        for name in ("a.jar", "b.jar", "c.jar.disabled"):
            make_mod(self.instance / "mods", name, name.split(".")[0])

    def tearDown(self) -> None:
        self.environment.stop()
        self.temporary.cleanup()

    def call(self, operation: str, **values) -> dict:
        from nivora_launcher.gui_service import handle

        return handle({"operation": operation, **values})

    def test_bulk_remove_mods(self) -> None:
        result = self.call("mods_remove_files", name="alpha", paths=["a.jar", "c.jar.disabled", "../evil.jar"])
        self.assertEqual(result["removed"], ["a.jar", "c.jar.disabled"])
        self.assertEqual([item["path"] for item in result["failed"]], ["../evil.jar"])
        self.assertEqual([item.name for item in (self.instance / "mods").glob("*.jar*")], ["b.jar"])

    def test_world_import_round_trip(self) -> None:
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("Skyblock/level.dat", b"nbt")
        begin = self.call("world_import_begin", name="alpha", kind="archive", label="sky.zip")
        self.call("world_import_chunk", name="alpha", upload_id=begin["upload_id"], path="sky.zip", offset=0, data=base64.b64encode(buffer.getvalue()).decode())
        result = self.call("world_import_finish", name="alpha", upload_id=begin["upload_id"])
        self.assertEqual(result["imported"], ["Skyblock"])
        self.assertEqual([world["id"] for world in result["details"]["worlds"]], ["Skyblock"])

    def test_catalog_pages(self) -> None:
        seen = {}

        class FakeModrinth:
            def __init__(self, *_args, **_kwargs):
                self.last_total = 45

            def search(self, query, game_version, loader, limit, **kwargs):
                seen.update(kwargs, limit=limit)
                return [{"project_id": f"p{index}", "title": "x"} for index in range(limit)]

        with patch("nivora_launcher.gui_service.ModrinthProvider", FakeModrinth):
            result = self.call("mods_search", instance_name="alpha", project_type="mod", catalog_id="modrinth", page=2, limit=20)
        self.assertEqual(seen["offset"], 40)
        self.assertEqual((result["page"], result["total"], result["has_more"]), (2, 45, False))
        with patch("nivora_launcher.gui_service.ModrinthProvider", FakeModrinth):
            result = self.call("mods_search", instance_name="alpha", project_type="mod", catalog_id="modrinth", page=0, limit=20)
        self.assertTrue(result["has_more"])


if __name__ == "__main__":
    unittest.main()
