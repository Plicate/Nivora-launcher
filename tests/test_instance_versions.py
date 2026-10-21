from __future__ import annotations

import base64
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock

from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import _import_custom_version, _instance_versions_payload
from nivora_launcher.instance_workspace import _normalize_library_overrides
from nivora_launcher.installer import _apply_library_overrides, _instance_library_overrides


class LibraryOverrideTests(unittest.TestCase):
    def test_normalize_accepts_only_allowlisted_lwjgl_coordinates(self) -> None:
        cleaned = _normalize_library_overrides({
            "org.lwjgl:lwjgl": "3.3.6",
            "org.lwjgl:lwjgl-glfw": "3.3.4",
            "com.example:evil": "1.0.0",
            "org.lwjgl:lwjgl-opengl": "not a version!",
        })
        self.assertEqual(cleaned, {"org.lwjgl:lwjgl": "3.3.6", "org.lwjgl:lwjgl-glfw": "3.3.4"})

    def test_overrides_rewrite_libraries_to_maven_central(self) -> None:
        version_json = {
            "libraries": [
                {"name": "org.lwjgl:lwjgl:3.3.3", "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar", "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar", "sha1": "abc"}}},
                {"name": "com.mojang:brigadier:1.0.18", "downloads": {"artifact": {"path": "com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar", "url": "https://libraries.minecraft.net/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"}}},
            ]
        }
        rewritten = _apply_library_overrides(version_json, {"org.lwjgl:lwjgl": "3.3.6"})
        lwjgl = rewritten["libraries"][0]
        self.assertEqual(lwjgl["name"], "org.lwjgl:lwjgl:3.3.6")
        self.assertEqual(lwjgl["downloads"]["artifact"]["url"], "https://repo1.maven.org/maven2/org/lwjgl/lwjgl/3.3.6/lwjgl-3.3.6.jar")
        self.assertNotIn("sha1", lwjgl["downloads"]["artifact"])
        brigadier = rewritten["libraries"][1]
        self.assertEqual(brigadier["name"], "com.mojang:brigadier:1.0.18")
        # Оригинал не изменён.
        self.assertEqual(version_json["libraries"][0]["name"], "org.lwjgl:lwjgl:3.3.3")

    def test_instance_overrides_read_from_settings_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory)
            (instance / ".trial-launcher.json").write_text(json.dumps({"library_overrides": {"org.lwjgl:lwjgl": "3.3.6"}}), encoding="utf-8")
            self.assertEqual(_instance_library_overrides(instance), {"org.lwjgl:lwjgl": "3.3.6"})
            self.assertEqual(_instance_library_overrides(Path("/nonexistent")), {})


def _make_instance(root: Path, name: str = "test-instance") -> Path:
    instance = root / "instances" / name
    version_dir = instance / "versions" / "1.21.1"
    version_dir.mkdir(parents=True)
    (version_dir / "1.21.1.json").write_text(json.dumps({
        "id": "1.21.1",
        "mainClass": "net.minecraft.client.main.Main",
        "javaVersion": {"majorVersion": 21},
        "assets": "17",
        "libraries": [
            {"name": "org.lwjgl:lwjgl:3.3.3"},
            {"name": "org.lwjgl:lwjgl-glfw:3.3.3"},
        ],
    }), encoding="utf-8")
    return instance


class InstanceVersionsTests(unittest.TestCase):
    def test_components_include_minecraft_java_lwjgl_and_loader(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _make_instance(root)
            instances = MagicMock()
            info = MagicMock()
            info.name = "test-instance"
            info.path = root / "instances" / "test-instance"
            info.version_id = "1.21.1"
            info.minecraft_version = "1.21.1"
            info.loader = "vanilla"
            info.game_edition = "java"
            info.kind = "client"
            instances.get.return_value = info
            workspace = MagicMock()
            workspace.settings.return_value = {"library_overrides": {}}

            payload = _instance_versions_payload(instances, workspace, "test-instance")

            by_id = {component["id"]: component for component in payload["components"]}
            self.assertEqual(by_id["minecraft"]["version"], "1.21.1")
            self.assertEqual(by_id["java"]["version"], "21")
            self.assertEqual(by_id["lwjgl"]["version"], "3.3.3")
            self.assertEqual(by_id["assets"]["version"], "17")
            self.assertFalse(by_id["minecraft"]["removable"])
            self.assertFalse(by_id["lwjgl"]["overridden"])

    def test_custom_version_import_writes_and_validates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _make_instance(root)
            instances = MagicMock()
            info = MagicMock()
            info.name = "test-instance"
            info.path = root / "instances" / "test-instance"
            info.running = False
            info.game_edition = "java"
            info.kind = "client"
            instances.get.return_value = info
            installer = MagicMock()
            installer.load_existing.return_value = MagicMock(version_id="my-custom")
            config = MagicMock()
            lock = MagicMock()
            config.paths.data_home = root / "runtime"

            payload_json = {"id": "my-custom", "mainClass": "net.minecraft.client.main.Main", "libraries": []}
            content = base64.b64encode(json.dumps(payload_json).encode("utf-8")).decode("ascii")
            result = _import_custom_version(config, instances, installer, "test-instance", "my-custom.json", content)

            self.assertEqual(result["version_id"], "my-custom")
            written = json.loads((info.path / "versions" / "my-custom" / "my-custom.json").read_text(encoding="utf-8"))
            self.assertEqual(written["id"], "my-custom")

    def test_custom_version_import_rejects_json_without_main_class(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instances = MagicMock()
            info = MagicMock()
            info.name = "test-instance"
            info.running = False
            info.game_edition = "java"
            info.kind = "client"
            instances.get.return_value = info
            content = base64.b64encode(json.dumps({"id": "broken"}).encode("utf-8")).decode("ascii")
            with self.assertRaises(LauncherError):
                _import_custom_version(MagicMock(), instances, MagicMock(), "test-instance", "broken.json", content)


if __name__ == "__main__":
    unittest.main()
