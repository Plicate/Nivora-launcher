from __future__ import annotations

import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from nivora_launcher.runtime import JavaRuntimeManager


class JavaRuntimeSelectionTests(unittest.TestCase):
    def test_newer_system_java_prefers_mojang_runtime_for_game_major(self) -> None:
        config = SimpleNamespace(settings=SimpleNamespace(java_path=""))
        manager = JavaRuntimeManager(config, http=Mock())
        managed = Path("/managed/java/bin/java")
        version = {"javaVersion": {"majorVersion": 21, "component": "java-runtime-delta"}}

        with (
            patch("nivora_launcher.runtime.shutil.which", return_value="/usr/bin/java"),
            patch("nivora_launcher.runtime.validate_java_path", side_effect=lambda path, **_kwargs: Path(path)),
            patch("nivora_launcher.runtime.java_version", return_value=27),
            patch.object(manager, "_ensure_mojang_runtime", return_value=managed) as ensure_runtime,
        ):
            selected = manager.java_for(version)

        self.assertEqual(selected, managed)
        ensure_runtime.assert_called_once_with("java-runtime-delta")

    def test_matching_system_java_skips_mojang_runtime_download(self) -> None:
        config = SimpleNamespace(settings=SimpleNamespace(java_path=""))
        manager = JavaRuntimeManager(config, http=Mock())
        system_java = Path("/usr/bin/java")
        version = {"javaVersion": {"majorVersion": 21, "component": "java-runtime-delta"}}

        with (
            patch("nivora_launcher.runtime.shutil.which", return_value=str(system_java)),
            patch("nivora_launcher.runtime.validate_java_path", side_effect=lambda path, **_kwargs: Path(path)),
            patch("nivora_launcher.runtime.java_version", return_value=21),
            patch.object(manager, "_ensure_mojang_runtime") as ensure_runtime,
        ):
            selected = manager.java_for(version)

        self.assertEqual(selected, system_java)
        ensure_runtime.assert_not_called()
