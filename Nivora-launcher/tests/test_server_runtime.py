from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.instances import InstanceStore
from nivora_launcher.server import ServerInstanceStore, ServerProcessManager, _PROCESSES, _LAUNCH_ERRORS, _initial_server_heap, _server_java_requirement


class ServerRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.environment = patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root / "config"), "XDG_DATA_HOME": str(self.root / "data"), "XDG_CACHE_HOME": str(self.root / "cache")})
        self.environment.start()
        self.config = ConfigStore(Paths())
        self.config.set_value("game_dir", str(self.root / "game"))
        self.store = ServerInstanceStore(self.config, InstanceStore(self.config))
        self.info = self.store.create("runtime-test", display_name="Test", core="custom", version="1.21.1", port=25565, memory="4GiB", eula_accepted=True)
        (self.info.path / "server.jar").write_bytes(b"test jar")
        self.manager = ServerProcessManager(self.config, self.store)
        self.patches = [patch("nivora_launcher.server.validate_java_path", return_value=Path("/usr/bin/java")), patch("nivora_launcher.server.java_version", return_value=21), patch("nivora_launcher.server.safe_process_environment", return_value=dict(os.environ))]
        for item in self.patches: item.start()

    def tearDown(self):
        try:
            self.manager.stop(self.info.name)
        finally:
            state = _PROCESSES.pop(self.info.name, None)
            if state:
                if state.process.stdout: state.process.stdout.close()
                if state.process.stdin: state.process.stdin.close()
                state.process_lock.release()
            _LAUNCH_ERRORS.pop(self.info.name, None)
            for item in self.patches: item.stop()
            self.environment.stop()
            self.temp.cleanup()

    def prepare(self, code):
        return patch("nivora_launcher.server.prepare_process", return_value=([sys.executable, "-u", "-c", code], self.info.path, dict(os.environ), None))

    def test_early_crash_output_survives_and_is_restored_after_backend_restart(self):
        with self.prepare("print('UnsupportedClassVersionError: Java too old', flush=True); raise SystemExit(7)"):
            self.manager.start(self.info.name)
        state = _PROCESSES[self.info.name]
        self.assertTrue(state.finished.wait(5))
        self.assertFalse(self.manager.status(self.info.name)["running"])
        self.assertEqual(self.manager.status(self.info.name)["exit_code"], 7)
        self.assertTrue(any("UnsupportedClassVersionError" in line for line in self.manager.log(self.info.name)["lines"]))
        _PROCESSES.pop(self.info.name)
        self.assertTrue(any("UnsupportedClassVersionError" in line for line in self.manager.log(self.info.name)["lines"]))
        if state.process.stdout: state.process.stdout.close()
        if state.process.stdin: state.process.stdin.close()

    def test_restart_waits_for_stop_and_releases_the_old_process_lock(self):
        code = "import sys; print('Ready',flush=True)\nfor line in sys.stdin:\n if line.strip()=='stop': print('World saved',flush=True); break"
        with self.prepare(code):
            first = self.manager.start(self.info.name)
            previous = _PROCESSES[self.info.name]
            second = self.manager.restart(self.info.name)
        self.assertTrue(previous.finished.is_set())
        self.assertEqual(previous.process.poll(), 0)
        self.assertTrue(second["running"])
        self.assertNotEqual(first["pid"], second["pid"])
        self.assertLessEqual(second["uptime_seconds"], 1)
        if previous.process.stdout: previous.process.stdout.close()
        if previous.process.stdin: previous.process.stdin.close()

    def test_old_explicit_java_is_rejected_before_java_is_launched(self):
        self.store.save_config(self.info.name, {"java_path": "/old/jdk/bin/java"})
        with patch("nivora_launcher.server.java_version", return_value=8), patch("nivora_launcher.server.subprocess.Popen") as spawn:
            with self.assertRaisesRegex(LauncherError, "Java 8.*Java 21"):
                self.manager.start(self.info.name)
        spawn.assert_not_called()
        self.assertIn("Java 21", self.manager.status(self.info.name)["start_error"])
        self.assertTrue(any("Ошибка запуска" in line for line in self.manager.log(self.info.name)["lines"]))

    def test_snapshot_failure_never_leaks_the_instance_process_lock(self):
        with self.prepare("print('finished',flush=True)"), patch("nivora_launcher.server.ProcessMetricsTracker.finish", side_effect=LauncherError("metrics unavailable")):
            self.manager.start(self.info.name)
            state = _PROCESSES[self.info.name]
            self.assertTrue(state.finished.wait(5))
        self.assertIsNone(state.process_lock._handle)

    def test_proxy_is_not_given_minecraft_gui_flags(self):
        self.store.save_config(self.info.name, {"core": "velocity"})
        with self.prepare("print('proxy',flush=True)") as prepare:
            self.manager.start(self.info.name)
        self.assertNotIn("--nogui", prepare.call_args.args[0])
        self.assertTrue(_PROCESSES[self.info.name].finished.wait(5))

    def test_java_requirements_and_initial_heap(self):
        self.assertEqual(_server_java_requirement("1.20.4")[0], 17)
        self.assertEqual(_server_java_requirement("1.20.5")[0], 21)
        self.assertEqual(_server_java_requirement("1.17.1")[0], 16)
        self.assertEqual(_server_java_requirement("1.16.5")[0], 8)
        self.assertEqual(_server_java_requirement("custom-build")[0], 0)
        self.assertEqual(_initial_server_heap("4G"), "512M")
        self.assertEqual(_initial_server_heap("256M"), "256M")


if __name__ == "__main__":
    unittest.main()
