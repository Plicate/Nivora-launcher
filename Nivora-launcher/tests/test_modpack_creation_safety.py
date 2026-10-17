from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from nivora_launcher.accounts import AccountStore
from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.instances import InstanceStore


class ModpackCreationSafetyTests(unittest.TestCase):
    def test_new_modpack_install_refuses_existing_name_before_mutating_settings(self) -> None:
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
                instances = InstanceStore(config)
                existing = instances.root / "existing-pack"
                existing.mkdir()
                (existing / "keep.txt").write_text("working copy", encoding="utf-8")
                installer = Mock()
                context = (
                    config,
                    object(),
                    AccountStore(config),
                    object(),
                    installer,
                    object(),
                    instances,
                )

                with patch("nivora_launcher.gui_service._context", return_value=context):
                    with self.assertRaisesRegex(LauncherError, "Для модпака выберите другое имя"):
                        handle(
                            {
                                "operation": "install",
                                "version": "1.20.1",
                                "instance_name": "existing-pack",
                                "fail_if_exists": True,
                                "java_path": "/not-applied/java",
                            }
                        )

                self.assertEqual((existing / "keep.txt").read_text(encoding="utf-8"), "working copy")
                self.assertNotEqual(config.settings.java_path, "/not-applied/java")
                installer.install.assert_not_called()


if __name__ == "__main__":
    unittest.main()
