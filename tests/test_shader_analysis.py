from __future__ import annotations

import unittest
import zipfile
from pathlib import Path

from nivora_launcher.errors import LauncherError
from nivora_launcher.shader_analysis import _tier_from_name_and_vram, analyze_shaderpack
from nivora_launcher.shader_benchmark import _compare
from nivora_launcher.shader_benchmark import _shader_options_line, apply_shader_selection


def _make_pack(directory: Path, files: dict[str, str]) -> Path:
    pack = directory / "TestPack.zip"
    with zipfile.ZipFile(pack, "w") as archive:
        for name, content in files.items():
            archive.writestr(name, content)
    return pack


class ShaderAnalysisTests(unittest.TestCase):
    def test_heavy_pack_features_and_complexity(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            pack = _make_pack(Path(directory), {
                "shaders/shaders.properties": "shadowMapResolution=2048\nsupportedVersions=1.21\n",
                "shaders/shadow.fsh": "uniform sampler2D depth; #ifdef POM parallax\n raytracing reflections here",
                "shaders/composite.fsh": "volumetric lightshaft god_ray sample\n taa temporal filter",
                "shaders/final.fsh": "bloom and dof focal_length motion_blur",
                "readme.txt": "not a shader",
            })
            analysis = analyze_shaderpack(pack)

        self.assertIn("Тени (shadow pass)", analysis["features"])
        self.assertIn("POM / параллакс", analysis["features"])
        self.assertIn("Отражения (SSR/raytracing)", analysis["features"])
        self.assertIn("Волюметрический свет", analysis["features"])
        self.assertIn("TAA / временное сглаживание", analysis["features"])
        self.assertEqual(analysis["shadow_resolution"], 2048)
        self.assertGreater(analysis["complexity"], 45)
        self.assertIn("final", analysis["programs"])
        self.assertIn("shadow", analysis["programs"])

    def test_light_pack_is_recognized_as_easy(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            pack = _make_pack(Path(directory), {
                "shaders/final.fsh": "vec3 color = texture(gcolor, uv).rgb;",
            })
            analysis = analyze_shaderpack(pack)
        self.assertLessEqual(analysis["complexity"], 20)
        self.assertNotIn("Тени (shadow pass)", analysis["features"])

    def test_pack_without_programs_is_rejected(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            pack = _make_pack(Path(directory), {"readme.txt": "hello"})
            with self.assertRaises(LauncherError):
                analyze_shaderpack(pack)

    def test_suspicious_paths_are_flagged(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            pack = _make_pack(Path(directory), {
                "shaders/final.fsh": "void main() {}",
                "shaders/../evil.txt": "x",
            })
            analysis = analyze_shaderpack(pack)
        self.assertTrue(any("Подозрительный путь" in note for note in analysis["notes"]))


class TierTests(unittest.TestCase):
    def test_known_gpu_families_map_to_tiers(self) -> None:
        self.assertEqual(_tier_from_name_and_vram("NVIDIA GeForce RTX 4070", 8192), "enthusiast")
        self.assertEqual(_tier_from_name_and_vram("NVIDIA GeForce RTX 3060", 12288), "high")
        self.assertEqual(_tier_from_name_and_vram("AMD Radeon RX 6700 XT", 12288), "high")
        self.assertEqual(_tier_from_name_and_vram("NVIDIA GeForce GTX 1660", 6144), "mid")
        self.assertEqual(_tier_from_name_and_vram("Intel UHD Graphics 620", 0), "entry")
        self.assertEqual(_tier_from_name_and_vram("llvmpipe (LLVM 15)", 0), "software")


class BenchmarkHelpersTests(unittest.TestCase):
    def test_shader_options_line_formats_by_version(self) -> None:
        legacy = _shader_options_line("1.20.1", "BSL.zip")
        self.assertEqual(legacy, "shaderPack:BSL.zip")
        modern = _shader_options_line("1.21.4", "BSL.zip")
        self.assertIn('"file": "BSL.zip"', modern)

    def test_apply_shader_selection_roundtrip(self) -> None:
        options = "version:2970\nshaderPack:Old.zip\nfov:70\n"
        cleared = apply_shader_selection(options, None, "1.20.1")
        self.assertNotIn("shaderPack", cleared)
        selected = apply_shader_selection(options, "New.zip", "1.20.1")
        self.assertIn("shaderPack:New.zip", selected)
        self.assertNotIn("Old.zip", selected)
        self.assertIn("fov:70", selected)

    def test_compare_verdicts_by_delta(self) -> None:
        light = _compare({"gpu_percent": 30.0, "cpu_percent": 20.0}, {"gpu_percent": 35.0, "cpu_percent": 25.0})
        self.assertEqual(light["tone"], "success")
        heavy = _compare({"gpu_percent": 30.0, "cpu_percent": 20.0}, {"gpu_percent": 96.0, "cpu_percent": 60.0})
        self.assertEqual(heavy["tone"], "danger")
        fallback = _compare({"gpu_percent": None, "cpu_percent": 20.0}, {"gpu_percent": None, "cpu_percent": 70.0})
        self.assertEqual(fallback["metric"], "cpu_percent")


if __name__ == "__main__":
    unittest.main()
