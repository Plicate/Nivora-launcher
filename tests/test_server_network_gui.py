from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.instances import InstanceStore
from nivora_launcher.server import ServerInstanceStore


class ServerNetworkGuiTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        self.environment = patch.dict(os.environ, {"XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"), "XDG_CACHE_HOME": str(root / "cache")})
        self.environment.start()
        self.config = ConfigStore(Paths())
        self.config.set_value("game_dir", str(root / "game"))
        self.instances = InstanceStore(self.config)
        self.servers = ServerInstanceStore(self.config, self.instances)
        self.server = self.servers.create("net-server", display_name="Network test", core="custom", version="1.21.1", port=25565, memory="4GiB", eula_accepted=True)
        self.context = patch("nivora_launcher.gui_service._context", return_value=(self.config, None, None, None, None, None, self.instances))
        self.context.start()

    def tearDown(self):
        self.context.stop()
        self.environment.stop()
        self.temp.cleanup()

    def request(self, operation, **values):
        return handle({"operation": operation, "name": self.server.name, **values})

    def test_status_is_local_only_and_does_not_discover_network(self):
        with patch("nivora_launcher.gui_service.diagnose_server_network") as diagnose:
            self.assertFalse(self.request("server_network_status")["mapping"]["active"])
        diagnose.assert_not_called()

    def test_prepare_requires_confirmation_and_stopped_server(self):
        with self.assertRaisesRegex(LauncherError, "Подтвердите"):
            self.request("server_network_prepare")
        with patch("nivora_launcher.gui_service._server_is_running", return_value=True):
            with self.assertRaisesRegex(LauncherError, "Остановите"):
                self.request("server_network_prepare", confirmed=True)

    def test_prepare_changes_bind_without_disabling_authentication(self):
        path = self.server.path / "server.properties"
        path.write_text("server-ip=127.0.0.1\nonline-mode=true\nwhite-list=true\n", encoding="utf-8")
        result = self.request("server_network_prepare", confirmed=True)
        self.assertTrue(result["prepared"])
        self.assertIn("server-ip=\n", path.read_text())
        self.assertIn("online-mode=true", path.read_text())
        self.assertIn("white-list=true", path.read_text())

    def test_proxy_cores_do_not_modify_server_properties(self):
        self.servers.save_config(self.server.name, {"core": "velocity"})
        with self.assertRaisesRegex(LauncherError, "listener"):
            self.request("server_network_prepare", confirmed=True)

    def test_open_does_not_allow_unconfirmed_or_malformed_payload(self):
        with patch("nivora_launcher.gui_service.server_publisher.open") as publish:
            with self.assertRaisesRegex(LauncherError, "Подтвердите"):
                self.request("server_network_open")
            with self.assertRaisesRegex(LauncherError, "IPv4"):
                self.request("server_network_open", confirmed=True, observed_ip="127.0.0.1")
            with self.assertRaisesRegex(LauncherError, "Порт"):
                self.request("server_network_open", confirmed=True, external_port="25565; echo bad")
        publish.assert_not_called()

    def test_diagnose_passes_the_saved_internal_port_not_an_arbitrary_target(self):
        self.servers.save_config(self.server.name, {"port": 25566})
        with patch("nivora_launcher.gui_service.diagnose_server_network", return_value={"report": True}) as diagnose:
            self.assertTrue(self.request("server_network_diagnose", external_port=25570)["report"])
        self.assertEqual(diagnose.call_args.args[1], 25566)
        self.assertEqual(diagnose.call_args.kwargs["external_port"], 25570)

    def test_stop_attempts_cleanup_but_does_not_fail_when_router_is_offline(self):
        events = []
        with patch("nivora_launcher.gui_service.server_publisher.close", side_effect=LauncherError("Router offline")) as close, \
             patch("nivora_launcher.gui_service.ServerProcessManager.stop", return_value={"running": False}) as stop:
            result = handle({"operation": "server_stop", "name": self.server.name}, emit=events.append)
        self.assertFalse(result["running"])
        close.assert_called_once_with(self.server.path)
        stop.assert_called_once_with(self.server.name)
        self.assertTrue(any(item.get("event") == "server-network-close-failed" for item in events))


if __name__ == "__main__":
    unittest.main()
