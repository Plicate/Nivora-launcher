from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.gui_service import _emit_telemetry_report


class FakeHttp:
    def __init__(self, *args, **kwargs) -> None:
        self.calls: list[tuple[str, dict]] = []

    def post_json(self, url: str, value: dict, headers: dict | None = None):
        self.calls.append((url, value))
        return {"ok": True}


class TelemetryTests(unittest.TestCase):
    def test_crash_reports_are_sent_only_when_opted_in(self) -> None:
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
                config.set_values({"telemetry": True, "telemetry_endpoint": "https://errors.example.invalid/collect"})
                instance = SimpleNamespace(name="alpha", kind="client", loader="fabric", minecraft_version="1.20.1", version_id="fabric-1.20.1")
                created_clients: list[FakeHttp] = []

                def build_client(*args, **kwargs):
                    client = FakeHttp(*args, **kwargs)
                    created_clients.append(client)
                    return client

                with patch("nivora_launcher.gui_service.HttpClient", side_effect=build_client):
                    _emit_telemetry_report(
                        config,
                        event="launch_exit_nonzero",
                        instance=instance,
                        version_id="fabric-1.20.1",
                        exit_code=1,
                        message="Minecraft exited with code 1",
                        logs=["line 1", "line 2"],
                    )

        self.assertEqual(len(created_clients), 1)
        self.assertEqual(created_clients[0].calls[0][0], "https://errors.example.invalid/collect")
        payload = created_clients[0].calls[0][1]
        self.assertEqual(payload["instance"]["name"], "alpha")
        self.assertEqual(payload["error"]["exit_code"], 1)
        self.assertEqual(payload["error"]["log_tail"], ["line 1", "line 2"])

    def test_invalid_endpoint_is_normalized_away(self) -> None:
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
                config.set_values({"telemetry": True, "telemetry_endpoint": "file:///tmp/not-allowed"})
                self.assertEqual(config.settings.telemetry_endpoint, "")


if __name__ == "__main__":
    unittest.main()
