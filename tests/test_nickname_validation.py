from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.accounts import validate_nickname
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle


class NicknameValidationTests(unittest.TestCase):
    def test_valid_nicknames(self) -> None:
        for name in ("Steve", "Player_1", "abc", "A" * 16):
            self.assertEqual(validate_nickname(name), name)

    def test_invalid_characters_are_rejected_not_stripped(self) -> None:
        cases = {
            "Игрок": "«И»",
            "Steve Jobs": "«пробел»",
            "Steve-1!": "«-», «!»",
            "Stéve": "«é»",
            " Steve": "«пробел»",
        }
        for name, fragment in cases.items():
            with self.assertRaises(LauncherError) as caught:
                validate_nickname(name)
            message = str(caught.exception)
            self.assertTrue(message.startswith("Ник содержит недопустимые символы:"), message)
            self.assertIn(fragment, message)

    def test_length(self) -> None:
        with self.assertRaisesRegex(LauncherError, "не короче 3"):
            validate_nickname("ab")
        with self.assertRaisesRegex(LauncherError, "не длиннее 16"):
            validate_nickname("a" * 17)
        with self.assertRaisesRegex(LauncherError, "Введите ник"):
            validate_nickname("")

    def test_gui_operation_reports_error(self) -> None:
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ,
            {"XDG_CONFIG_HOME": str(Path(root) / "c"), "XDG_DATA_HOME": str(Path(root) / "d"), "XDG_CACHE_HOME": str(Path(root) / "x")},
        ):
            with self.assertRaisesRegex(LauncherError, "недопустимые символы"):
                handle({"operation": "create_offline_account", "username": "Ник Игрока"})
            account = handle({"operation": "create_offline_account", "username": "Steve_77"})["account"]
            self.assertEqual(account["username"], "Steve_77")


if __name__ == "__main__":
    unittest.main()
