from __future__ import annotations

import json
import os
import tempfile
import time
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from nivora_launcher import auto_repair
from nivora_launcher.auto_repair import RepairContext, apply_fixes, diagnose
from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.instances import INSTANCE_METADATA_FILE

GIB = 1024 * 1024


def make_mod(directory: Path, file_name: str, *, fabric: dict | None = None, toml: str | None = None, extra: dict[str, str] | None = None) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / file_name
    with zipfile.ZipFile(path, "w") as archive:
        if fabric is not None:
            archive.writestr("fabric.mod.json", json.dumps(fabric))
        if toml is not None:
            archive.writestr("META-INF/mods.toml", toml)
        for name, content in (extra or {}).items():
            archive.writestr(name, content)
        archive.writestr("dummy.class", b"\xca\xfe\xba\xbe")
    return path


def kinds(report: dict) -> list[str]:
    return [issue["id"] for issue in report["issues"]]


def fix(report: dict, kind: str, action: str) -> dict:
    issue = next(issue for issue in report["issues"] if issue["id"] == kind)
    return next(item for item in issue["fixes"] if item["action"] == action)


class DiagnoseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.instance = Path(self.temporary.name) / "alpha"
        self.mods = self.instance / "mods"
        self.mods.mkdir(parents=True)
        # Java не ищем по системе: тестам важна логика, а не окружение.
        self.java = patch("nivora_launcher.runtime.discover_java_paths", return_value=[])
        self.java.start()

    def tearDown(self) -> None:
        self.java.stop()
        self.temporary.cleanup()

    def run_diagnose(self, lines: list[str], **kwargs) -> dict:
        kwargs.setdefault("loader", "fabric")
        kwargs.setdefault("instance_settings", {"memory": "2GiB", "java_path": "", "jvm_arguments": [], "jvm_preset": "none", "graphics_mode": "auto"})
        return diagnose(lines, kwargs.pop("exit_code", 1), instance_path=self.instance, crash_report=kwargs.pop("crash_report", ""), **kwargs)

    def test_fabric_missing_dependency_offers_install_and_disable(self) -> None:
        make_mod(self.mods, "sodium-extra.jar", fabric={"id": "sodium-extra", "depends": {"sodium": "*"}})
        report = self.run_diagnose([
            "net.fabricmc.loader.impl.FormattedException: Mod resolution encountered an incompatible mod set!",
            " - Mod 'Sodium Extra' (sodium-extra) 0.5.1 requires version 0.5.0 or later of mod 'Sodium' (sodium), which is missing!",
        ])
        self.assertEqual(kinds(report), ["missing_dependencies"])
        self.assertEqual(fix(report, "missing_dependencies", "install_dependencies")["params"]["ids"], ["sodium"])
        self.assertEqual(fix(report, "missing_dependencies", "disable_mods")["params"]["files"], ["sodium-extra.jar"])
        self.assertTrue(report["issues"][0]["fixes"][0]["recommended"])

    def test_forge_missing_and_wrong_minecraft_version(self) -> None:
        make_mod(self.mods, "mowzies.jar", toml='modId="mowziesmobs"\n')
        make_mod(self.mods, "oldmod.jar", toml='modId="oldmod"\n')
        report = self.run_diagnose([
            "Missing or unsupported mandatory dependencies:",
            "\tMod ID: 'geckolib', Requested by: 'mowziesmobs', Expected range: '[4.2,)', Actual version: '[MISSING]'",
            "\tMod ID: 'minecraft', Requested by: 'oldmod', Expected range: '[1.19,1.20)', Actual version: '1.20.1'",
        ], loader="forge")
        self.assertIn("missing_dependencies", kinds(report))
        self.assertEqual(fix(report, "missing_dependencies", "install_dependencies")["params"]["ids"], ["geckolib"])
        self.assertEqual(fix(report, "incompatible_mods", "disable_mods")["params"]["files"], ["oldmod.jar"])

    def test_wrong_loader_and_duplicates_and_corrupted_found_statically(self) -> None:
        make_mod(self.mods, "fabric-ok.jar", fabric={"id": "okmod"})
        make_mod(self.mods, "forge-only.jar", toml='modId="forgeonly"\n')
        old = make_mod(self.mods, "lithium-0.1.jar", fabric={"id": "lithium"})
        os.utime(old, (time.time() - 1000, time.time() - 1000))
        make_mod(self.mods, "lithium-0.2.jar", fabric={"id": "lithium"})
        (self.mods / "broken.jar").write_bytes(b"not a zip")
        report = self.run_diagnose(["Exception in thread main"])
        self.assertEqual(fix(report, "wrong_loader_mods", "disable_mods")["params"]["files"], ["forge-only.jar"])
        self.assertEqual(fix(report, "duplicate_mods", "disable_mods")["params"]["files"], ["lithium-0.1.jar"])
        self.assertEqual(fix(report, "corrupted_mods", "disable_mods")["params"]["files"], ["broken.jar"])

    def test_connector_allows_fabric_mods_on_forge(self) -> None:
        make_mod(self.mods, "connector.jar", toml='modId="connector"\n')
        make_mod(self.mods, "fabric-only.jar", fabric={"id": "fabriconly"})
        report = self.run_diagnose(["crash"], loader="forge")
        self.assertNotIn("wrong_loader_mods", kinds(report))

    def test_mixin_failure_maps_config_to_jar(self) -> None:
        make_mod(self.mods, "badmod.jar", fabric={"id": "badmod"}, extra={"badmod.mixins.json": "{}"})
        report = self.run_diagnose([
            "org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered",
            "Caused by: org.spongepowered.asm.mixin.injection.throwables.InvalidInjectionException: Critical injection failure: @Inject annotation on foo could not find any targets matching 'bar' in net.minecraft.class_1. [PREINJECT Applicator Phase -> badmod.mixins.json:FooMixin]",
        ])
        self.assertEqual(fix(report, "mixin_failure", "disable_mods")["params"]["files"], ["badmod.jar"])

    def test_out_of_memory_raises_memory(self) -> None:
        with patch.object(auto_repair, "system_memory_kib", return_value=16 * GIB):
            report = self.run_diagnose(["java.lang.OutOfMemoryError: Java heap space"], effective_memory="2GiB")
        memory_fix = fix(report, "out_of_memory", "set_memory")
        self.assertEqual(memory_fix["params"]["memory"], "4GiB")

    def test_heap_reservation_failure_lowers_memory(self) -> None:
        with patch.object(auto_repair, "system_memory_kib", return_value=8 * GIB), patch.object(auto_repair, "recommended_memory_kib", return_value=4 * GIB):
            report = self.run_diagnose([
                "Error occurred during initialization of VM",
                "Could not reserve enough space for 12582912KB object heap",
            ], effective_memory="12GiB")
        self.assertEqual(kinds(report), ["memory_too_high"])
        self.assertEqual(fix(report, "memory_too_high", "set_memory")["params"]["memory"], "4GiB")

    def test_java_version_from_class_file_and_fabric_message(self) -> None:
        settings = {"memory": "2GiB", "java_path": "/opt/jdk17/bin/java", "jvm_arguments": [], "jvm_preset": "none"}
        report = self.run_diagnose([
            "java.lang.UnsupportedClassVersionError: foo has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0",
        ], instance_settings=settings)
        issue = report["issues"][0]
        self.assertEqual(issue["id"], "java_version")
        self.assertIn("21", issue["detail"])
        self.assertEqual(fix(report, "java_version", "set_java")["params"]["java_path"], "")
        report = self.run_diagnose([
            " - Mod 'Foo' (foo) 1.0 requires version 21 or later of 'OpenJDK 64-Bit Server VM' (java), but only the wrong version is present: 17!",
        ], instance_settings=settings)
        self.assertIn("21", report["issues"][0]["detail"])

    def test_jvm_arguments_reset_only_when_custom(self) -> None:
        lines = ["Unrecognized VM option 'UseZGC2'", "Error: Could not create the Java Virtual Machine."]
        settings = {"memory": "2GiB", "java_path": "", "jvm_arguments": ["-XX:+UseZGC2"], "jvm_preset": "none"}
        report = self.run_diagnose(lines, instance_settings=settings)
        self.assertEqual(fix(report, "jvm_arguments", "reset_jvm_arguments")["action"], "reset_jvm_arguments")

    def test_broken_config_is_found_inside_instance(self) -> None:
        config = self.instance / "config"
        config.mkdir()
        (config / "create-common.toml").write_text("[[broken", encoding="utf-8")
        report = self.run_diagnose([
            "Failed loading config file create-common.toml of type COMMON for modid create",
            "com.electronwill.nightconfig.core.io.ParsingException: Invalid table declaration",
            "Also config/../../etc/passwd.toml",
        ], loader="forge")
        self.assertEqual(fix(report, "broken_config", "backup_configs")["params"]["files"], ["config/create-common.toml"])

    def test_game_files_and_graphics(self) -> None:
        report = self.run_diagnose(["Error: Could not find or load main class net.fabricmc.loader.impl.launch.knot.KnotClient"])
        # Класс самого загрузчика не найден — это сломанный модлоадер, а не ванильные файлы.
        self.assertEqual(kinds(report), ["loader_broken"])
        self.assertEqual(report["issues"][0]["fixes"][0]["action"], "repair_version")
        self.assertEqual(fix(report, "loader_broken", "update_loader")["params"], {"minimum": ""})
        report = self.run_diagnose(["Error: Could not find or load main class net.minecraft.client.main.Main"], loader="vanilla")
        self.assertEqual(kinds(report), ["game_files"])
        make_mod(self.mods, "iris.jar", fabric={"id": "iris"})
        report = self.run_diagnose(["GLFW error 65542: WGL: The driver does not appear to support OpenGL"])
        issue = report["issues"][0]
        self.assertEqual(issue["id"], "graphics")
        self.assertTrue(issue["manual"])
        self.assertIn("iris.jar", fix(report, "graphics", "disable_mods")["params"]["files"])

    def test_fallbacks_recent_mods_then_unknown(self) -> None:
        make_mod(self.mods, "stable.jar", fabric={"id": "stable"})
        os.utime(self.mods / "stable.jar", (time.time() - 5000, time.time() - 5000))
        (self.instance / auto_repair.STATE_FILE).write_text(json.dumps({"last_success": time.time() - 1000}), encoding="utf-8")
        make_mod(self.mods, "fresh.jar", fabric={"id": "fresh"})
        report = self.run_diagnose(["something odd happened"])
        self.assertEqual(kinds(report), ["recent_mods"])
        self.assertEqual(fix(report, "recent_mods", "disable_mods")["params"]["files"], ["fresh.jar"])
        (self.mods / "fresh.jar").unlink()
        report = self.run_diagnose(["something odd happened"])
        self.assertEqual(kinds(report), ["unknown_crash"])
        self.assertEqual(report["issues"][0]["fixes"][0]["action"], "repair_version")

    def test_crash_report_suspects(self) -> None:
        make_mod(self.mods, "culprit-1.0.jar", fabric={"id": "culprit"})
        crash = "---- Minecraft Crash Report ----\nSuspected Mods:\n\tCulprit (culprit), Version: 1.0\n\t\tIssue tracker URL: x\n\nDetails:"
        report = self.run_diagnose(["exit"], crash_report=crash)
        self.assertEqual(fix(report, "incompatible_mods", "disable_mods")["params"]["files"], ["culprit-1.0.jar"])

    def test_muted_kinds_are_hidden(self) -> None:
        report = self.run_diagnose(["Error: Could not find or load main class foo"], muted=["game_files"])
        self.assertEqual(report["issues"], [])
        self.assertEqual(report["muted_issues"], ["game_files"])


class ApplyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.instance = Path(self.temporary.name) / "alpha"
        (self.instance / "mods").mkdir(parents=True)
        self.saved: dict = {}
        self.toggled: list[str] = []
        self.context = RepairContext(
            instance_name="alpha",
            instance_path=self.instance,
            save_settings=self.saved.update,
            settings=lambda: dict(self.saved),
            toggle_mod=self.toggled.append,
            repair_version=lambda: None,
            install_mod=self.install,
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def install(project_id: str) -> None:
        if project_id == "unknown":
            raise LauncherError("not found")

    def test_disable_rejects_paths_outside_mods(self) -> None:
        results = apply_fixes(self.context, [{"id": "x", "action": "disable_mods", "params": {"files": ["../evil.jar", "good.jar"]}}])
        self.assertTrue(results[0]["ok"])
        self.assertEqual(self.toggled, ["good.jar"])

    def test_settings_fixes(self) -> None:
        results = apply_fixes(self.context, [
            {"action": "reset_jvm_arguments", "params": {}},
            {"action": "set_graphics_mode", "params": {"mode": "mesa"}},
            {"action": "set_java", "params": {"java_path": ""}},
            {"action": "set_graphics_mode", "params": {"mode": "rm -rf"}},
            {"action": "launch_rockets", "params": {}},
        ])
        self.assertEqual([item["ok"] for item in results], [True, True, True, False, False])
        self.assertEqual(self.saved, {"jvm_arguments": [], "jvm_preset": "none", "graphics_mode": "mesa", "java_path": ""})

    def test_install_reports_partial_results(self) -> None:
        results = apply_fixes(self.context, [{"action": "install_dependencies", "params": {"ids": ["geckolib", "unknown"]}}])
        self.assertTrue(results[0]["ok"])
        self.assertIn("unknown", results[0]["message"])

    def test_backup_configs_moves_only_inside_config(self) -> None:
        config = self.instance / "config"
        config.mkdir()
        (config / "a.toml").write_text("x", encoding="utf-8")
        (self.instance / "secret.toml").write_text("keep", encoding="utf-8")
        results = apply_fixes(self.context, [{"action": "backup_configs", "params": {"files": ["config/a.toml", "config/../secret.toml"]}}])
        self.assertTrue(results[0]["ok"])
        self.assertFalse((config / "a.toml").exists())
        self.assertTrue((self.instance / "secret.toml").exists())
        backups = list((self.instance / auto_repair.BACKUP_DIRECTORY).rglob("a.toml"))
        self.assertEqual(len(backups), 1)


class GuiServiceAutoRepairTests(unittest.TestCase):
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
        config = ConfigStore(Paths())
        config.set_value("game_dir", str(self.root / "game"))
        self.instance = config.game_dir / "instances" / "alpha"
        version = "fabric-loader-0.16.10-1.21.4"
        version_dir = self.instance / "versions" / version
        version_dir.mkdir(parents=True)
        (version_dir / f"{version}.json").write_text(json.dumps({"id": version, "inheritsFrom": "1.21.4"}), encoding="utf-8")
        (self.instance / INSTANCE_METADATA_FILE).write_text(json.dumps({"kind": "client", "display_name": "alpha"}), encoding="utf-8")
        make_mod(self.instance / "mods", "forge-only.jar", toml='modId="forgeonly"\n')

    def tearDown(self) -> None:
        self.environment.stop()
        self.temporary.cleanup()

    def call(self, operation: str, **values) -> dict:
        return handle({"operation": operation, **values})

    def test_diagnose_apply_and_mute_round_trip(self) -> None:
        report = self.call("auto_repair_diagnose", instance_name="alpha", manual=True)
        self.assertIn("wrong_loader_mods", kinds(report))
        chosen = fix(report, "wrong_loader_mods", "disable_mods")
        result = self.call("auto_repair_apply", instance_name="alpha", fixes=[chosen])
        self.assertTrue(result["results"][0]["ok"], result)
        self.assertTrue((self.instance / "mods" / "forge-only.jar.disabled").exists())

        settings = self.call("auto_repair_mute", kinds=["graphics", "bogus"], muted=True)["settings"]
        self.assertEqual(settings["auto_repair_muted"], ["graphics"])
        self.assertTrue(settings["auto_repair_enabled"])
        self.assertTrue(any(item["id"] == "graphics" for item in settings["auto_repair_kinds"]))
        settings = self.call("auto_repair_mute", kinds=["graphics"], muted=False)["settings"]
        self.assertEqual(settings["auto_repair_muted"], [])

    def test_disabled_setting_suppresses_automatic_diagnosis(self) -> None:
        self.call("save_settings", values={"auto_repair_enabled": False, "auto_repair_muted": ["graphics"]})
        report = self.call("auto_repair_diagnose", instance_name="alpha", exit_code=1)
        self.assertTrue(report.get("disabled"))
        bootstrap_settings = self.call("save_settings", values={})["settings"]
        self.assertFalse(bootstrap_settings["auto_repair_enabled"])
        self.assertEqual(bootstrap_settings["auto_repair_muted"], ["graphics"])


if __name__ == "__main__":
    unittest.main()
