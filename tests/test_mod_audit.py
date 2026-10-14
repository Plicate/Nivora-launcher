from __future__ import annotations

import json
import os
import time
import unittest
import zipfile
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.mod_audit import (
    AUDIT_CATEGORIES,
    AuditError,
    ModAuditStore,
    format_estimate,
    parse_class_file,
)


class ClassFileBuilder:
    """Builds minimal but valid Java class files for audit tests."""

    def __init__(self) -> None:
        self.pool: dict[int, bytes] = {}
        self.utf8_indices: dict[str, int] = {}
        self.class_indices: dict[str, int] = {}
        self.nat_indices: dict[tuple[str, str], int] = {}
        self.next_index = 1

    def _add(self, raw: bytes) -> int:
        index = self.next_index
        self.pool[index] = raw
        self.next_index += 1
        return index

    def utf8(self, value: str) -> int:
        if value in self.utf8_indices:
            return self.utf8_indices[value]
        encoded = value.encode("utf-8")
        index = self._add(b"\x01" + len(encoded).to_bytes(2, "big") + encoded)
        self.utf8_indices[value] = index
        return index

    def klass(self, name: str) -> int:
        if name in self.class_indices:
            return self.class_indices[name]
        name_index = self.utf8(name)
        index = self._add(b"\x07" + name_index.to_bytes(2, "big"))
        self.class_indices[name] = index
        return index

    def name_and_type(self, name: str, descriptor: str) -> int:
        key = (name, descriptor)
        if key in self.nat_indices:
            return self.nat_indices[key]
        name_index = self.utf8(name)
        descriptor_index = self.utf8(descriptor)
        index = self._add(b"\x0c" + name_index.to_bytes(2, "big") + descriptor_index.to_bytes(2, "big"))
        self.nat_indices[key] = index
        return index

    def method_ref(self, owner: str, name: str, descriptor: str = "()V") -> None:
        class_index = self.klass(owner)
        nat_index = self.name_and_type(name, descriptor)
        self._add(b"\x0a" + class_index.to_bytes(2, "big") + nat_index.to_bytes(2, "big"))

    def field_ref(self, owner: str, name: str, descriptor: str = "I") -> None:
        class_index = self.klass(owner)
        nat_index = self.name_and_type(name, descriptor)
        self._add(b"\x09" + class_index.to_bytes(2, "big") + nat_index.to_bytes(2, "big"))

    def string(self, value: str) -> None:
        index = self.utf8(value)
        self._add(b"\x08" + index.to_bytes(2, "big"))

    def build(
        self,
        this_class: str,
        super_class: str = "java/lang/Object",
        interfaces: tuple[str, ...] = (),
        major: int = 52,
    ) -> bytes:
        this_index = self.klass(this_class)
        super_index = self.klass(super_class)
        interface_indices = [self.klass(name) for name in interfaces]
        out = bytearray(b"\xca\xfe\xba\xbe")
        out += (0).to_bytes(2, "big")
        out += major.to_bytes(2, "big")
        out += self.next_index.to_bytes(2, "big")
        for index in range(1, self.next_index):
            out += self.pool[index]
        out += (0x0021).to_bytes(2, "big")
        out += this_index.to_bytes(2, "big")
        out += super_index.to_bytes(2, "big")
        out += len(interface_indices).to_bytes(2, "big")
        for index in interface_indices:
            out += index.to_bytes(2, "big")
        out += (0).to_bytes(2, "big") * 3
        return bytes(out)


def _write_jar(path: Path, entries: dict[str, bytes]) -> Path:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in entries.items():
            archive.writestr(name, data)
    return path


def _clean_mod_jar(path: Path) -> Path:
    builder = ClassFileBuilder()
    builder.method_ref("org/bukkit/plugin/java/JavaPlugin", "getConfig", "()Lorg/bukkit/configuration/file/FileConfiguration;")
    builder.method_ref("org/bukkit/configuration/file/YamlConfiguration", "save", "(Ljava/io/File;)V")
    builder.method_ref("java/util/logging/Logger", "info", "(Ljava/lang/String;)V")
    builder.string("https://api.modrinth.com/v2/project/example")
    builder.string("mod.version=1.0.0")
    return _write_jar(
        path,
        {
            "fabric.mod.json": json.dumps(
                {
                    "schemaVersion": 1,
                    "id": "example-mod",
                    "version": "1.0.0",
                    "name": "Example Mod",
                    "entrypoints": {"main": ["com.example.ExampleMod"]},
                }
            ).encode("utf-8"),
            "com/example/ExampleMod.class": builder.build("com/example/ExampleMod"),
        },
    )


def _keylogger_jar(path: Path) -> Path:
    builder = ClassFileBuilder()
    builder.method_ref("org/jnativehook/GlobalScreen", "registerNativeHook", "()V")
    builder.method_ref("java/awt/Robot", "keyPress", "(I)V")
    builder.method_ref("java/awt/Robot", "createScreenCapture", "(Ljava/awt/Rectangle;)Ljava/awt/image/BufferedImage;")
    builder.string("https://discord.com/api/webhooks/123456789/secret-token")
    return _write_jar(
        path,
        {"com/evil/Spy.class": builder.build("com/evil/Spy")},
    )


def _exec_network_jar(path: Path) -> Path:
    builder = ClassFileBuilder()
    builder.method_ref("java/lang/Runtime", "exec", "(Ljava/lang/String;)Ljava/lang/Process;")
    builder.method_ref("java/net/HttpURLConnection", "connect", "()V")
    builder.string("powershell -enc aHR0cHM6Ly9ldmlsLmNvbQ==")
    return _write_jar(
        path,
        {"com/evil/Dropper.class": builder.build("com/evil/Dropper")},
    )


def _token_thief_jar(path: Path) -> Path:
    builder = ClassFileBuilder()
    builder.string("launcher_accounts.json")
    builder.string("access_token")
    builder.klass("java/net/URL")
    builder.method_ref("java/net/URL", "openConnection", "()Ljava/net/URLConnection;")
    return _write_jar(path, {"com/evil/Tokens.class": builder.build("com/evil/Tokens")})


class ParseClassFileTests(unittest.TestCase):
    def test_extracts_strings_refs_and_hierarchy(self) -> None:
        builder = ClassFileBuilder()
        builder.method_ref("java/net/URL", "openStream", "()Ljava/io/InputStream;")
        builder.field_ref("java/lang/System", "out", "Ljava/io/PrintStream;")
        builder.string("hello world")
        data = builder.build("com/example/Foo", "java/lang/Object", ("org/bukkit/event/Listener",))
        info = parse_class_file(data, entry="com/example/Foo.class")
        self.assertEqual(info.name, "com/example/Foo")
        self.assertEqual(info.super_name, "java/lang/Object")
        self.assertEqual(info.interfaces, ["org/bukkit/event/Listener"])
        self.assertIn("java/net/URL.openStream", info.method_refs)
        self.assertIn("java/lang/System.out", info.field_refs)
        self.assertIn("hello world", info.strings)
        self.assertIn("java/net/URL", info.class_refs)

    def test_rejects_non_class_data(self) -> None:
        with self.assertRaises(LauncherError):
            parse_class_file(b"\x00\x01\x02\x03not-a-class-at-all")
        with self.assertRaises(LauncherError):
            parse_class_file(b"\xca\xfe\xba\xbe\x00\x00")


class ModAuditStoreTests(unittest.TestCase):
    def setUp(self) -> None:
        import tempfile

        self.tmp = Path(tempfile.mkdtemp(prefix="mod-audit-test-"))
        self.addCleanup(_rmtree, self.tmp)
        self.store = ModAuditStore(self.tmp / "audit")

    def _session_dir(self, session_id: str) -> Path:
        return self.tmp / "audit" / session_id

    def test_prepare_reports_structure_estimate_and_check_list(self) -> None:
        jar = _clean_mod_jar(self.tmp / "clean.jar")
        payload = self.store.prepare(jar)
        self.assertEqual(payload["state"], "awaiting")
        self.assertTrue(payload["session_id"])
        self.assertEqual(payload["source"]["kind"], "mod")
        self.assertEqual(payload["structure"]["platform"], "fabric")
        self.assertEqual(payload["structure"]["classes"], 1)
        self.assertGreater(payload["structure"]["source_lines"], 0)
        self.assertGreaterEqual(payload["estimate"]["seconds"], 1)
        self.assertTrue(payload["estimate"]["label"])
        self.assertGreater(payload["checks_count"], 20)
        self.assertEqual(len(payload["checks"]), payload["checks_count"])
        categories = {item["id"] for item in payload["checks"]}
        for required in ("keylogger", "windows", "close", "logging", "files", "network", "process", "combo"):
            self.assertIn(required, categories)
        self.assertTrue((self._session_dir(payload["session_id"]) / "sources").is_dir())

    def test_accept_scans_then_destroys_sources(self) -> None:
        jar = _clean_mod_jar(self.tmp / "clean.jar")
        payload = self.store.prepare(jar)
        progress: list[int] = []
        report = self.store.accept(payload["session_id"], progress=lambda percent, _name: progress.append(percent))
        self.assertTrue(report["sources_wiped"])
        self.assertFalse((self._session_dir(payload["session_id"]) / "sources").exists())
        self.assertTrue(progress)
        self.assertEqual(progress[-1], 99)
        self.assertEqual(report["verdict"], "clean")
        self.assertEqual(report["counts"]["critical"], 0)
        self.assertEqual(report["counts"]["warning"], 0)
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("network-known-host", finding_ids)
        self.assertIn("logging-frameworks", finding_ids)
        # Second accept of the same session is rejected
        with self.assertRaises(LauncherError):
            self.store.accept(payload["session_id"])

    def test_cancel_destroys_sources_without_checking(self) -> None:
        jar = _keylogger_jar(self.tmp / "spy.jar")
        payload = self.store.prepare(jar)
        result = self.store.cancel(payload["session_id"])
        self.assertTrue(result["cancelled"])
        self.assertFalse((self._session_dir(payload["session_id"]) / "sources").exists())
        status = self.store.status(payload["session_id"])
        self.assertEqual(status["state"], "cancelled")
        self.assertTrue(status["sources_wiped"])
        self.assertNotIn("report", status)
        with self.assertRaises(LauncherError):
            self.store.accept(payload["session_id"])

    def test_keylogger_detection_is_critical(self) -> None:
        jar = _keylogger_jar(self.tmp / "spy.jar")
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        self.assertEqual(report["verdict"], "dangerous")
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("keylogger-native-hook", finding_ids)
        self.assertIn("keylogger-robot", finding_ids)
        self.assertIn("screenshot-capture", finding_ids)
        self.assertIn("exfil-webhook", finding_ids)
        self.assertIn("combo-keylogger-network", finding_ids)
        self.assertGreaterEqual(report["counts"]["critical"], 4)

    def test_exec_and_network_detection(self) -> None:
        jar = _exec_network_jar(self.tmp / "dropper.jar")
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        self.assertEqual(report["verdict"], "dangerous")
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("exec-runtime", finding_ids)
        self.assertIn("exec-shell-string", finding_ids)
        self.assertIn("network-http", finding_ids)
        self.assertIn("combo-exec-network", finding_ids)

    def test_token_theft_detection(self) -> None:
        jar = _token_thief_jar(self.tmp / "tokens.jar")
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("files-sensitive-account", finding_ids)
        self.assertIn("mc-session-files", finding_ids)
        self.assertIn("combo-sensitive-network", finding_ids)
        self.assertEqual(report["verdict"], "dangerous")

    def test_cryptominer_detection(self) -> None:
        builder = ClassFileBuilder()
        builder.string("stratum+tcp://pool.minexmr.org:4444")
        builder.string("xmrig worker setup, cryptonight variant")
        builder.method_ref("java/net/HttpURLConnection", "getOutputStream", "()Ljava/io/OutputStream;")
        jar = _write_jar(
            self.tmp / "miner.jar",
            {
                "fabric.mod.json": b'{"schemaVersion":1,"id":"optim","version":"1.0","name":"Optimizer"}',
                "com/optim/Main.class": builder.build("com/optim/Main"),
            },
        )
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("miner-pool-endpoints", finding_ids)
        self.assertIn("combo-miner-network", finding_ids)
        self.assertEqual(report["verdict"], "dangerous")

    def test_wallet_and_browser_credential_theft_detection(self) -> None:
        builder = ClassFileBuilder()
        builder.string("AppData/Local/Google/Chrome/User Data/Default/Login Data")
        builder.string(r"C:\Users\steve\AppData\Roaming\discord\Local Storage\leveldb\000003.log")
        builder.string("UTC--2024-01-01T04-05-06.000Z--abcdef")
        builder.method_ref("java/net/Socket", "connect", "(Ljava/net/SocketAddress;I)V")
        jar = _write_jar(
            self.tmp / "stealer.jar",
            {"com/steal/S.class": builder.build("com/steal/S")},
        )
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("browser-credential-files", finding_ids)
        self.assertIn("discord-session-theft", finding_ids)
        self.assertIn("wallet-files", finding_ids)
        self.assertIn("combo-wallet-network", finding_ids)
        self.assertEqual(report["verdict"], "dangerous")

    def test_disk_wiper_detection_is_critical(self) -> None:
        builder = ClassFileBuilder()
        builder.string("cmd /c del /f /s /q C:\\Users\\%USERNAME%\\Documents")
        builder.method_ref("java/lang/Runtime", "exec", "(Ljava/lang/String;)Ljava/lang/Process;")
        jar = _write_jar(
            self.tmp / "wiper.jar",
            {"com/wipe/W.class": builder.build("com/wipe/W")},
        )
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("disk-wiper", finding_ids)
        categories = {finding["category"] for finding in report["findings"]}
        self.assertIn("destruction", categories)

    def test_env_harvest_with_network_combines_into_warning(self) -> None:
        builder = ClassFileBuilder()
        builder.method_ref("java/lang/System", "getenv", "(Ljava/lang/String;)Ljava/lang/String;")
        builder.method_ref("java/net/HttpURLConnection", "connect", "()V")
        jar = _write_jar(
            self.tmp / "env.jar",
            {"com/env/E.class": builder.build("com/env/E")},
        )
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("env-harvest", finding_ids)
        self.assertIn("combo-env-exfil", finding_ids)

    def test_hardware_probe_is_notice_not_alarm(self) -> None:
        builder = ClassFileBuilder()
        builder.klass("oshi/SystemInfo")
        jar = _write_jar(
            self.tmp / "hw.jar",
            {"com/hw/H.class": builder.build("com/hw/H")},
        )
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("hardware-probe", finding_ids)
        self.assertNotEqual(report["verdict"], "dangerous")

    def test_no_false_positive_for_allowlisted_hosts_and_game_input(self) -> None:
        builder = ClassFileBuilder()
        builder.klass("org/lwjgl/input/Keyboard")
        builder.method_ref("org/bukkit/plugin/java/JavaPlugin", "getConfig", "()Lorg/bukkit/configuration/file/FileConfiguration;")
        builder.string("https://api.modrinth.com/v2/search?query=sodium")
        builder.string("Requires Minecraft 1.20.1 or later")
        builder.method_ref("java/lang/reflect/Field", "setAccessible", "(Z)V")
        jar = _write_jar(
            self.tmp / "benign.jar",
            {
                "fabric.mod.json": b'{"schemaVersion":1,"id":"benign","version":"1.0","name":"Benign"}',
                "com/example/Benign.class": builder.build("com/example/Benign"),
            },
        )
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        self.assertEqual(report["verdict"], "clean")
        self.assertEqual(report["counts"]["critical"], 0)
        self.assertEqual(report["counts"]["warning"], 0)
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("network-known-host", finding_ids)
        self.assertIn("keyboard-game-input", finding_ids)
        self.assertNotIn("network-raw-ip", finding_ids)
        self.assertNotIn("self-update-download", finding_ids)

    def test_minecraft_version_string_is_not_flagged_as_ip(self) -> None:
        builder = ClassFileBuilder()
        builder.string("Supported versions: 1.20.1.5 and newer")
        builder.string("https://modrinth.com/mod/example")
        jar = _write_jar(self.tmp / "versioned.jar", {"com/example/V.class": builder.build("com/example/V")})
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertNotIn("network-raw-ip", finding_ids)
        self.assertIn("network-known-host", finding_ids)

    def test_malformed_and_unsafe_entries_are_skipped_not_fatal(self) -> None:
        builder = ClassFileBuilder()
        builder.string("ok")
        entries = {
            "com/ok/Ok.class": builder.build("com/ok/Ok"),
            "com/bad/Broken.class": b"\xca\xfe\xba\xbe\xff\xff\xff\xff",
            "native/evil.dll": b"MZ fake dll",
            "../escape.class": builder.build("escape/Esc"),
        }
        jar = _write_jar(self.tmp / "mixed.jar", entries)
        payload = self.store.prepare(jar)
        self.assertEqual(payload["structure"]["classes"], 1)
        self.assertIn("native/evil.dll", payload["structure"]["natives"])
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("native-bundled", finding_ids)
        self.assertIn("skipped-entries", finding_ids)
        self.assertIn("no-manifest", finding_ids)
        self.assertEqual(report["verdict"], "dangerous")

    def test_obfuscated_names_are_detected(self) -> None:
        entries = {
            f"com/obf/{name}.class": ClassFileBuilder().build(f"com/obf/{name}")
            for name in ("a", "b", "c", "ab", "ac", "ad", "ba", "bb", "bc", "bd", "ca", "cb", "cc", "cd", "da", "db", "dc", "dd", "e", "f", "g", "h", "i", "j", "k", "l", "m")
        }
        jar = _write_jar(self.tmp / "obf.jar", entries)
        payload = self.store.prepare(jar)
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("obfuscation-names", finding_ids)
        self.assertEqual(report["verdict"], "suspicious")

    def test_server_plugin_manifest_and_kind(self) -> None:
        builder = ClassFileBuilder()
        builder.method_ref("org/bukkit/Bukkit", "dispatchCommand", "(Lorg/bukkit/command/CommandSender;Ljava/lang/String;)Z")
        builder.string("ops.json")
        jar = _write_jar(
            self.tmp / "plugin.jar",
            {
                "plugin.yml": b"name: Demo\nversion: 1.0\nmain: com.example.Demo\napi-version: 1.20\n",
                "com/example/Demo.class": builder.build("com/example/Demo"),
            },
        )
        payload = self.store.prepare(jar)
        self.assertEqual(payload["source"]["kind"], "plugin")
        self.assertEqual(payload["structure"]["platform"], "bukkit")
        self.assertEqual(payload["structure"]["declared"].get("name"), "Demo")
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("server-dispatch-command", finding_ids)
        self.assertIn("server-files", finding_ids)

    def test_nested_jar_classes_are_scanned(self) -> None:
        inner_builder = ClassFileBuilder()
        inner_builder.method_ref("java/lang/Runtime", "exec", "(Ljava/lang/String;)Ljava/lang/Process;")
        inner = _write_jar(
            self.tmp / "inner.jar",
            {"hidden/payload.class": inner_builder.build("hidden/payload")},
        )
        outer_builder = ClassFileBuilder()
        outer_builder.string("outer string")
        outer = _write_jar(
            self.tmp / "outer.jar",
            {
                "com/ok/Outer.class": outer_builder.build("com/ok/Outer"),
                "META-INF/libraries/inner.jar": inner.read_bytes(),
            },
        )
        payload = self.store.prepare(outer)
        self.assertEqual(payload["structure"]["classes"], 2)
        self.assertEqual(payload["structure"]["nested_jars"], ["META-INF/libraries/inner.jar"])
        report = self.store.accept(payload["session_id"])
        finding_ids = {finding["id"] for finding in report["findings"]}
        self.assertIn("exec-runtime", finding_ids)
        evidence_classes = {
            evidence["class"]
            for finding in report["findings"]
            for evidence in finding["evidence"]
        }
        self.assertTrue(any("inner" in name for name in evidence_classes))

    def test_missing_and_invalid_inputs(self) -> None:
        with self.assertRaises(LauncherError):
            self.store.prepare(self.tmp / "missing.jar")
        not_a_zip = self.tmp / "plain.jar"
        not_a_zip.write_bytes(b"definitely not a zip file")
        with self.assertRaises(LauncherError):
            self.store.prepare(not_a_zip)
        no_classes = _write_jar(self.tmp / "empty.jar", {"readme.txt": b"nothing here"})
        with self.assertRaises(LauncherError):
            self.store.prepare(no_classes)
        with self.assertRaises(LauncherError):
            self.store.accept("not-a-session-id")
        with self.assertRaises(LauncherError):
            self.store.status("f" * 32)

    def test_stale_sessions_are_cleaned_up(self) -> None:
        jar = _clean_mod_jar(self.tmp / "clean.jar")
        payload = self.store.prepare(jar)
        record_path = self._session_dir(payload["session_id"]) / "session.json"
        record = json.loads(record_path.read_text(encoding="utf-8"))
        record["created_at"] = time.time() - 3600
        record_path.write_text(json.dumps(record), encoding="utf-8")
        self.assertEqual(self.store.cleanup(), 1)
        self.assertFalse(self._session_dir(payload["session_id"]).exists())

    def test_cancelled_accept_wipes_sources(self) -> None:
        jar = _keylogger_jar(self.tmp / "spy.jar")
        payload = self.store.prepare(jar)
        with self.assertRaises(LauncherError):
            self.store.accept(payload["session_id"], cancelled=lambda: True)
        self.assertFalse((self._session_dir(payload["session_id"]) / "sources").exists())
        self.assertEqual(self.store.status(payload["session_id"])["state"], "cancelled")

    def test_format_estimate_labels(self) -> None:
        self.assertEqual(format_estimate(45), "около 45 с")
        self.assertEqual(format_estimate(90), "около 1 мин 30 с")
        self.assertEqual(format_estimate(3600), "около 1 ч 0 мин")


class GuiModAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        import tempfile

        self.tmp = Path(tempfile.mkdtemp(prefix="mod-audit-gui-"))
        self.addCleanup(_rmtree, self.tmp)
        instance_dir = self.tmp / "instance" / "Test"
        (instance_dir / "mods").mkdir(parents=True)
        self.jar = _keylogger_jar(instance_dir / "mods" / "SpyMod.jar")
        self.config = SimpleNamespace(paths=SimpleNamespace(data_home=self.tmp / "data"), settings=SimpleNamespace())
        self.instances = SimpleNamespace(
            get=lambda name: SimpleNamespace(name=name, kind="client", path=instance_dir),
        )

    def _handle(self, request: dict, events: list[dict] | None = None):
        from nivora_launcher.gui_service import handle

        patches = [
            patch(
                "nivora_launcher.gui_service._context",
                return_value=(self.config, object(), None, None, None, None, self.instances),
            ),
            patch("nivora_launcher.gui_service.InstanceWorkspace", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.PluginStore", return_value=SimpleNamespace(list=lambda: [])),
            patch("nivora_launcher.gui_service.ServerInstanceStore", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.ServerFiles", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.ServerProcessManager", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.BedrockProfileStore", return_value=SimpleNamespace()),
        ]
        emit = events.append if events is not None else None
        with ExitStack() as stack:
            for patcher in patches:
                stack.enter_context(patcher)
            return handle(request, emit=emit)

    def _handle_with_events(self, request: dict, events: list[dict]):
        return self._handle(request, events)

    def test_full_gui_flow_prepare_accept_and_progress_events(self) -> None:
        events: list[dict] = []
        prepared = self._handle({"operation": "mod_audit_prepare", "instance_name": "Test", "path": "SpyMod.jar"})
        self.assertEqual(prepared["state"], "awaiting")
        self.assertGreater(prepared["checks_count"], 20)

        result = self._handle_with_events(
            {"operation": "mod_audit_accept", "session_id": prepared["session_id"]},
            events,
        )
        report = result["report"]
        self.assertEqual(report["verdict"], "dangerous")
        self.assertTrue(report["sources_wiped"])
        progress_events = [event for event in events if event.get("event") == "mod-audit-progress"]
        self.assertTrue(progress_events)
        finish_events = [event for event in events if event.get("event") == "mod-audit-finished"]
        self.assertTrue(finish_events)

    def test_gui_cancel_flow(self) -> None:
        prepared = self._handle({"operation": "mod_audit_prepare", "instance_name": "Test", "path": "SpyMod.jar"})
        result = self._handle({"operation": "mod_audit_cancel", "session_id": prepared["session_id"]})
        self.assertTrue(result["cancelled"])
        status = self._handle({"operation": "mod_audit_status", "session_id": prepared["session_id"]})
        self.assertEqual(status["state"], "cancelled")
        self.assertTrue(status["sources_wiped"])

    def test_gui_rejects_traversal_paths(self) -> None:
        with self.assertRaises(LauncherError):
            self._handle({"operation": "mod_audit_prepare", "instance_name": "Test", "path": "../../etc/passwd"})
        with self.assertRaises(LauncherError):
            self._handle({"operation": "mod_audit_prepare", "instance_name": "Test", "path": "Missing.jar"})


def _rmtree(path: Path) -> None:
    import shutil

    shutil.rmtree(path, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
