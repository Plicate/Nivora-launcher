from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.instances import InstanceStore
from nivora_launcher.server import ServerInstanceStore, server_memory_limit_kib


GIB_IN_KIB = 1024 * 1024


class ServerMemoryLimitTests(unittest.TestCase):
    def test_limit_reserves_one_quarter_of_physical_memory(self) -> None:
        self.assertEqual(server_memory_limit_kib(8 * GIB_IN_KIB), 6 * GIB_IN_KIB)
        self.assertEqual(server_memory_limit_kib(1), 1)

    def test_unknown_physical_memory_uses_two_gib_limit(self) -> None:
        self.assertEqual(server_memory_limit_kib(0), 2 * GIB_IN_KIB)

    def test_created_and_saved_server_configs_enforce_the_heap_limit(self) -> None:
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
                with patch("nivora_launcher.server.system_memory_kib", return_value=8 * GIB_IN_KIB):
                    info = servers.create(
                        "memory-limit-test",
                        display_name="Memory limit test",
                        core="custom",
                        version="1.21.1",
                        port=25565,
                        memory="7GiB",
                        eula_accepted=False,
                    )
                    self.assertEqual(servers.config(info.name)["memory"], "6GiB")
                    with self.assertRaisesRegex(LauncherError, "75% физической памяти"):
                        servers.save_config(info.name, {"memory": "7GiB"})
                    self.assertEqual(servers.config(info.name)["memory"], "6GiB")


if __name__ == "__main__":
    unittest.main()
