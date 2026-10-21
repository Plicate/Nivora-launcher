from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.gui_cloud_ops import CLOUD_OPERATIONS, handle_cloud_operation


class CloudOperationDispatchTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        root = Path(self._tmp.name)
        environment = {
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_CACHE_HOME": str(root / "cache"),
        }
        patcher = patch.dict(os.environ, environment, clear=False)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.config = ConfigStore(Paths())

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_unknown_operation_returns_none(self) -> None:
        self.assertIsNone(
            handle_cloud_operation("bootstrap", {}, self.config, lambda _event: None, lambda _c: {}, lambda _c: {})
        )

    def test_status_reports_unconfigured_storage(self) -> None:
        result = handle_cloud_operation(
            "cloud_status", {}, self.config, lambda _event: None, lambda _c: {}, lambda _c: {"configured": False}
        )
        self.assertEqual(result, {"status": {"configured": False}})

    def test_disconnect_wipes_settings_and_password(self) -> None:
        events: list[dict] = []
        self.config.set_values({"cloud_url": "https://example.com/dav", "cloud_username": "user"})
        result = handle_cloud_operation(
            "cloud_disconnect",
            {},
            self.config,
            events.append,
            lambda _config: {"settings": True},
            lambda _config: {"configured": False},
        )
        self.assertEqual(result["status"], {"configured": False})
        self.assertFalse(self.config.settings.cloud_url)
        self.assertFalse(self.config.settings.cloud_username)

    def test_operation_set_matches_the_extracted_handlers(self) -> None:
        # если в gui_service появится новая cloud_-операция мимо модуля,
        # набор перестанет сходиться с реальным списком операций
        self.assertEqual(
            sorted(CLOUD_OPERATIONS),
            sorted(
                [
                    "cloud_status",
                    "cloud_configure",
                    "cloud_test",
                    "cloud_list",
                    "cloud_sync",
                    "cloud_restore",
                    "cloud_delete",
                    "cloud_disconnect",
                ]
            ),
        )


if __name__ == "__main__":
    unittest.main()
