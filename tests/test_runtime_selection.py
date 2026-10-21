from __future__ import annotations

import subprocess
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from nivora_launcher.runtime import (
    JavaRuntimeManager,
    _legacy_version_id,
    _required_java,
    java_bitness,
)


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

    def test_legacy_version_without_java_version_uses_jre_legacy(self) -> None:
        config = SimpleNamespace(settings=SimpleNamespace(java_path=""))
        manager = JavaRuntimeManager(config, http=Mock())
        managed = Path("/managed/jre-legacy/bin/java")
        # до 1.13 блока javaVersion в JSON нет вовсе
        version = {"id": "1.12.2-fabric-0.14.25"}

        with (
            patch("nivora_launcher.runtime.shutil.which", return_value="/usr/bin/java"),
            patch("nivora_launcher.runtime.validate_java_path", side_effect=lambda path, **_kwargs: Path(path)),
            patch("nivora_launcher.runtime.java_version", return_value=17),
            patch.object(manager, "_ensure_mojang_runtime", return_value=managed) as ensure_runtime,
        ):
            selected = manager.java_for(version)

        self.assertEqual(selected, managed)
        ensure_runtime.assert_called_once_with("jre-legacy")

    def test_system_java_8_satisfies_legacy_version_without_download(self) -> None:
        config = SimpleNamespace(settings=SimpleNamespace(java_path=""))
        manager = JavaRuntimeManager(config, http=Mock())
        system_java = Path("/usr/bin/java")

        with (
            patch("nivora_launcher.runtime.shutil.which", return_value=str(system_java)),
            patch("nivora_launcher.runtime.validate_java_path", side_effect=lambda path, **_kwargs: Path(path)),
            patch("nivora_launcher.runtime.java_version", return_value=8),
            patch.object(manager, "_ensure_mojang_runtime") as ensure_runtime,
        ):
            selected = manager.java_for({"id": "1.8.9"})

        self.assertEqual(selected, system_java)
        ensure_runtime.assert_not_called()

    def test_required_java_rules(self) -> None:
        self.assertEqual(_required_java({"id": "1.12.2"}), (8, "jre-legacy"))
        self.assertEqual(_required_java({"id": "1.8.9-fabric"}), (8, "jre-legacy"))
        self.assertEqual(_required_java({"id": "1.16.5"}), (None, None))
        self.assertEqual(
            _required_java({"id": "1.21.1", "javaVersion": {"majorVersion": 21, "component": "java-runtime-gamma"}}),
            (21, "java-runtime-gamma"),
        )
        # кастомный json без javaVersion и без 1.x-идентификатора — не legacy
        self.assertEqual(_required_java({"id": "my-custom-pack"}), (None, None))

    def test_legacy_version_id_boundaries(self) -> None:
        self.assertTrue(_legacy_version_id("1.12.2"))
        self.assertTrue(_legacy_version_id("1.8.9-fabric-0.14"))
        self.assertFalse(_legacy_version_id("1.13"))
        self.assertFalse(_legacy_version_id("1.16.5"))
        self.assertFalse(_legacy_version_id("custom-pack"))


class JavaBitnessTests(unittest.TestCase):
    def _probe(self, output: str) -> int | None:
        completed = subprocess.CompletedProcess([], 0, stdout="", stderr=output)
        with patch("nivora_launcher.runtime.subprocess.run", return_value=completed):
            return java_bitness(Path("/opt/java/bin/java"))

    def test_hotspot_styles_are_classified(self) -> None:
        self.assertEqual(self._probe('openjdk version "1.8.0_312"\nOpenJDK 64-Bit Server VM'), 64)
        self.assertEqual(self._probe('java version "1.8.0_301"\nJava HotSpot(TM) Client VM'), 32)
        self.assertEqual(self._probe('openjdk version "17.0.9"\nOpenJDK Server VM'), 32)
        self.assertEqual(self._probe('openjdk version "21"'), 32)
        self.assertIsNone(self._probe("ls: cannot access '/opt/java': No such file"))

    def test_failed_probe_returns_none(self) -> None:
        with patch("nivora_launcher.runtime.subprocess.run", side_effect=OSError("nope")):
            self.assertIsNone(java_bitness(Path("/opt/java/bin/java")))
