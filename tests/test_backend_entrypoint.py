import os
import subprocess
import unittest
from unittest import mock

from nivora_launcher import backend_entrypoint


class HideChildConsolesTest(unittest.TestCase):
    def test_windows_children_get_create_no_window(self) -> None:
        calls: list[dict] = []

        def recorder(self, *args, **kwargs):  # type: ignore[no-untyped-def]
            calls.append(kwargs)

        original = subprocess.Popen.__init__
        try:
            subprocess.Popen.__init__ = recorder  # type: ignore[method-assign]
            with mock.patch.object(os, "name", "nt"):
                backend_entrypoint._hide_child_consoles()
                backend_entrypoint._hide_child_consoles()  # идемпотентно
            subprocess.Popen(["java", "-version"])
            subprocess.Popen(["cmd"], creationflags=0x10)
            subprocess.Popen(["x"], creationflags=0x200)
        finally:
            subprocess.Popen.__init__ = original  # type: ignore[method-assign]
        self.assertEqual(calls[0]["creationflags"], 0x08000000)
        self.assertEqual(calls[1]["creationflags"], 0x10)
        self.assertEqual(calls[2]["creationflags"], 0x200 | 0x08000000)

    def test_non_windows_is_untouched(self) -> None:
        original = subprocess.Popen.__init__
        with mock.patch.object(os, "name", "posix"):
            backend_entrypoint._hide_child_consoles()
        self.assertIs(subprocess.Popen.__init__, original)


if __name__ == "__main__":
    unittest.main()
