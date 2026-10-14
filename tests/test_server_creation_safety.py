from __future__ import annotations

import json
import os
import threading
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.instances import InstanceStore
from nivora_launcher.server import SERVER_JAR, SERVER_PROPERTIES_FILE, ServerInstanceStore, ServerProcessManager, _apply_server_port


class ServerCreationSafetyTests(unittest.TestCase):
    def test_server_config_refuses_to_write_eula_through_a_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
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
                servers = ServerInstanceStore(config, InstanceStore(config))
                info = servers.create(
                    "eula-link-test",
                    display_name="EULA link test",
                    core="custom",
                    version="1.21.1",
                    port=25565,
                    memory="4GiB",
                    eula_accepted=True,
                )
                outside = root / "outside.txt"
                outside.write_text("leave this alone", encoding="utf-8")
                eula = info.path / "eula.txt"
                eula.unlink()
                eula.symlink_to(outside)

                with self.assertRaisesRegex(LauncherError, "EULA.*символьной ссылкой"):
                    servers.save_config(info.name, {"eula_accepted": False})

                self.assertEqual(outside.read_text(encoding="utf-8"), "leave this alone")
                self.assertTrue(eula.is_symlink())

    def test_configured_port_updates_all_property_entries_and_preserves_other_settings(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            properties = root / SERVER_PROPERTIES_FILE
            properties.write_bytes(
                b"# server defaults\r\nserver-port=25565\r\nmotd=Keep this\r\nserver-port : 25566\r\n"
            )

            _apply_server_port(root, 25577)

            updated = properties.read_bytes()
            self.assertEqual(
                updated,
                b"# server defaults\r\nserver-port=25577\r\nmotd=Keep this\r\nserver-port : 25577\r\n",
            )
            _apply_server_port(root, 25577)
            self.assertEqual(properties.read_bytes(), updated)

    def test_server_properties_symlinks_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "outside.properties"
            outside.write_text("motd=unchanged\n", encoding="utf-8")
            (root / SERVER_PROPERTIES_FILE).symlink_to(outside)

            with self.assertRaises(LauncherError):
                _apply_server_port(root, 25577)

            self.assertEqual(outside.read_text(encoding="utf-8"), "motd=unchanged\n")

    def test_server_start_applies_saved_port_before_java_starts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
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
                servers = ServerInstanceStore(config, InstanceStore(config))
                info = servers.create(
                    "port-test",
                    display_name="Port test",
                    core="custom",
                    version="1.21.1",
                    port=25577,
                    memory="4GiB",
                    eula_accepted=True,
                )
                (info.path / SERVER_JAR).write_bytes(b"fake jar")
                (info.path / SERVER_PROPERTIES_FILE).write_text("motd=Keep this\n", encoding="utf-8")
                process = SimpleNamespace(pid=99999999, poll=lambda: None, stdin=None, stdout=[], wait=lambda: 0)
                manager = ServerProcessManager(config, servers)
                threads_before = set(threading.enumerate())

                with patch("nivora_launcher.server.validate_java_path", return_value=Path("/usr/bin/java")), \
                    patch("nivora_launcher.server.java_version", return_value=21), \
                    patch("nivora_launcher.server.safe_process_environment", return_value={}), \
                    patch("nivora_launcher.server.prepare_process", return_value=([], info.path, {}, None)), \
                    patch("nivora_launcher.server.subprocess.Popen", return_value=process):
                    manager.start(info.name)

                live_state = info.path / ".trial-launcher-live.json"
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    if live_state.exists() and not json.loads(live_state.read_text())["running"]:
                        break
                    time.sleep(0.01)

                for thread in set(threading.enumerate()) - threads_before:
                    if "_read_output" in thread.name:
                        thread.join(timeout=2)

                self.assertEqual(
                    (info.path / SERVER_PROPERTIES_FILE).read_text(encoding="utf-8"),
                    "motd=Keep this\nserver-port=25577\n",
                )
                self.assertFalse(json.loads(live_state.read_text())["running"])

    def test_server_creation_cannot_replace_an_existing_world(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
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
                servers = ServerInstanceStore(config, InstanceStore(config))
                original = servers.create(
                    "existing-server",
                    display_name="Existing server",
                    core="custom",
                    version="1.21.1",
                    port=25565,
                    memory="4GiB",
                    eula_accepted=True,
                )
                world_file = original.path / "world" / "level.dat"
                world_file.write_bytes(b"existing-world")

                with self.assertRaisesRegex(LauncherError, "удалите сервер отдельно"):
                    servers.create(
                        "existing-server",
                        display_name="Replacement",
                        core="paper",
                        version="1.21.1",
                        port=25566,
                        memory="6GiB",
                        eula_accepted=True,
                        replace_existing=True,
                    )

                self.assertEqual(world_file.read_bytes(), b"existing-world")
                self.assertEqual(servers.get("existing-server").display_name, "Existing server")


if __name__ == "__main__":
    unittest.main()
