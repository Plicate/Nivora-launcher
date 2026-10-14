import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from nivora_launcher.modpack_export import _loader_version, export_instance_as_mrpack


class LoaderVersionExtractionTests(unittest.TestCase):
    def test_fabric_and_quilt_do_not_swallow_minecraft_suffix(self) -> None:
        self.assertEqual(_loader_version("fabric-loader-0.16.14-1.21.1", "1.21.1"), "0.16.14")
        self.assertEqual(_loader_version("quilt-loader-0.29.0-beta.1-1.21.1", "1.21.1"), "0.29.0-beta.1")

    def test_forge_and_neoforge_keep_working(self) -> None:
        self.assertEqual(_loader_version("1.20.4-forge-49.1.30", "1.20.4"), "49.1.30")
        self.assertEqual(_loader_version("1.21.1-neoforge-21.1.143", "1.21.1"), "21.1.143")


class ExportedDependencyTests(unittest.TestCase):
    def test_exported_mrpack_declares_exact_loader_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instances" / "pack"
            (instance_dir / "config").mkdir(parents=True)
            (instance_dir / "config" / "keep.txt").write_text("x", encoding="utf-8")
            instance = SimpleNamespace(
                name="pack",
                display_name="Pack",
                description="",
                path=instance_dir,
                game_edition="java",
                loader="fabric",
                version_id="fabric-loader-0.16.14-1.21.1",
                minecraft_version="1.21.1",
            )
            result = export_instance_as_mrpack(object(), instance, root / "exports")
            with open(result["path"], "rb") as handle:
                import io
                import zipfile

                with zipfile.ZipFile(io.BytesIO(handle.read())) as archive:
                    index = json.loads(archive.read("modrinth.index.json"))
            self.assertEqual(index["dependencies"]["minecraft"], "1.21.1")
            self.assertEqual(index["dependencies"]["fabric-loader"], "0.16.14")


if __name__ == "__main__":
    unittest.main()
