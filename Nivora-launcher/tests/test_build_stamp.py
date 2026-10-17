from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from nivora_launcher.constants import BUILD_STAMP
from nivora_launcher.gui_service import handle


class BuildStampTests(unittest.TestCase):
    """Метка сборки бэкенда: фронтенд сверяет её через bootstrap и показывает
    баннер, если в собранное приложение подменили только Python-часть."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.environment = os.environ.copy()
        os.environ.update(
            {
                "XDG_CONFIG_HOME": str(root / "config"),
                "XDG_DATA_HOME": str(root / "data"),
                "XDG_CACHE_HOME": str(root / "cache"),
            }
        )

    def tearDown(self) -> None:
        os.environ.clear()
        os.environ.update(self.environment)
        self.temporary.cleanup()

    def test_build_stamp_is_declared(self) -> None:
        self.assertTrue(BUILD_STAMP.strip(), "BUILD_STAMP не должен быть пустым")

    def test_bootstrap_reports_build_stamp(self) -> None:
        result = handle({"operation": "bootstrap"})
        self.assertEqual(result.get("build_stamp"), BUILD_STAMP)

    def test_bootstrap_reports_gdk_backend_for_wayland_detection(self) -> None:
        # Оверлей решает, работать ли с глобальными клавишами, по РЕАЛЬНОМУ
        # бэкенду приложения: пакетные врапперы форсируют GDK_BACKEND=x11
        # (XWayland), и тогда клавиши работают даже в Wayland-сессии.
        os.environ["GDK_BACKEND"] = "x11"
        try:
            result = handle({"operation": "bootstrap"})
        finally:
            os.environ.pop("GDK_BACKEND", None)
        self.assertEqual(result.get("system", {}).get("gdk_backend"), "x11")


if __name__ == "__main__":
    unittest.main()
