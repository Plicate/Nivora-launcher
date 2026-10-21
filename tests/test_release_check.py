from __future__ import annotations

import os
import tempfile
import unittest
from unittest.mock import Mock, patch

from nivora_launcher.constants import APP_VERSION, GITHUB_RELEASES_API_URL
from nivora_launcher.errors import LauncherError
from nivora_launcher.release_check import app_version_key, check_remote_release, is_newer_version


class VersionCompareTests(unittest.TestCase):
    def test_short_and_full_versions_are_equal(self) -> None:
        self.assertEqual(app_version_key("1.1"), app_version_key("1.1.0"))
        self.assertEqual(app_version_key("v1.1.0"), (1, 1, 0))
        self.assertEqual(app_version_key("1.0.3-1"), (1, 0, 3))

    def test_garbage_is_lowest(self) -> None:
        self.assertEqual(app_version_key("release"), (0, 0, 0))
        self.assertFalse(is_newer_version("release", "1.0.0"))

    def test_newer(self) -> None:
        self.assertTrue(is_newer_version("1.1", "1.0.3"))
        self.assertTrue(is_newer_version("1.10.0", "1.9.9"))
        self.assertFalse(is_newer_version("1.0.3", "1.1.0"))
        self.assertFalse(is_newer_version("1.1.0", "1.1"))


class RemoteReleaseTests(unittest.TestCase):
    def test_reports_update_from_github_payload(self) -> None:
        http = Mock()
        http.get_json.return_value = {
            "tag_name": "v9.0.0",
            "html_url": "https://github.com/vedrooou0/Nivora-launcher/releases/tag/v9.0.0",
            "published_at": "2026-10-17T10:00:00Z",
            "body": "notes",
            "assets": [
                {"name": "Nivora.Launcher_9.0.0_x64-setup.exe", "browser_download_url": "https://example.com/setup.exe"},
                "junk",
                {"name": "", "browser_download_url": "https://example.com/x"},
            ],
        }
        result = check_remote_release(http)
        self.assertEqual(http.get_json.call_args.args[0], GITHUB_RELEASES_API_URL)
        self.assertTrue(result["update_available"])
        self.assertEqual(result["latest_version"], "9.0.0")
        self.assertEqual(result["current_version"], APP_VERSION)
        self.assertEqual(len(result["assets"]), 1)

    def test_same_version_is_not_an_update(self) -> None:
        http = Mock()
        http.get_json.return_value = {"tag_name": APP_VERSION, "assets": []}
        result = check_remote_release(http)
        self.assertFalse(result["update_available"])
        self.assertTrue(result["release_url"].startswith("https://github.com/"))

    def test_invalid_payload_raises(self) -> None:
        http = Mock()
        http.get_json.return_value = {"message": "Not Found"}
        with self.assertRaises(LauncherError):
            check_remote_release(http)

    def test_network_error_is_wrapped(self) -> None:
        http = Mock()
        http.get_json.side_effect = LauncherError("offline")
        with self.assertRaisesRegex(LauncherError, "GitHub"):
            check_remote_release(http)


class InstanceLayoutSettingTests(unittest.TestCase):
    def test_layout_round_trip_and_normalization(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            env = {
                "XDG_DATA_HOME": os.path.join(root, "data"),
                "XDG_CONFIG_HOME": os.path.join(root, "config"),
                "XDG_CACHE_HOME": os.path.join(root, "cache"),
            }
            with patch.dict(os.environ, env):
                from nivora_launcher.config import ConfigStore

                store = ConfigStore()
                self.assertEqual(store.settings.instance_layout, "default")
                store.set_values({"instance_layout": "compact"})
                self.assertEqual(ConfigStore().settings.instance_layout, "compact")
                store.set_values({"instance_layout": "nonsense"})
                self.assertEqual(store.settings.instance_layout, "default")
                self.assertEqual(ConfigStore().settings.launcher_version, APP_VERSION)


if __name__ == "__main__":
    unittest.main()
