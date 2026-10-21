"""Tests for the optimizer module."""
from __future__ import annotations

import json
import os
import sys
import unittest
import tempfile
import time
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent.parent))

from nivora_launcher.optimizer import (
    JVM_PRESETS,
    GpuInfo,
    OptimizationReport,
    analyze_instance,
    cache_stats,
    classify_instance_weight,
    count_mods_in_instance,
    prune_stale_metadata_cache,
    read_launch_history,
    record_launch_end,
    record_launch_start,
    recommend_memory_mib,
    recommend_optimizer_mods,
    system_ram_kib,
)


# ── JVM_PRESETS ──────────────────────────────────────────────────

class TestJvmPresets(unittest.TestCase):
    def test_all_preset_ids_present(self):
        assert set(JVM_PRESETS.keys()) >= {"fps", "balanced", "heavy_modpack", "compatibility", "streaming", "none"}

    def test_each_preset_has_required_fields(self):
        for preset_id, preset in JVM_PRESETS.items():
            assert preset["id"] == preset_id, f"{preset_id}: id mismatch"
            assert isinstance(preset["name"], str) and preset["name"]
            assert isinstance(preset["flags"], list)
            assert isinstance(preset["recommended_for"], list)

    def test_fps_flags_include_g1gc(self):
        flags = JVM_PRESETS["fps"]["flags"]
        assert any("UseG1GC" in f for f in flags)
        assert any("MaxGCPauseMillis" in f for f in flags)

    def test_heavy_modpack_has_aikar_flags(self):
        flags = JVM_PRESETS["heavy_modpack"]["flags"]
        assert any("G1NewSizePercent" in f for f in flags)
        assert any("aikars" in f.lower() for f in flags)

    def test_compatibility_uses_serial_gc(self):
        flags = JVM_PRESETS["compatibility"]["flags"]
        assert any("SerialGC" in f for f in flags)
        assert not any("G1GC" in f for f in flags)

    def test_none_preset_has_empty_flags(self):
        assert JVM_PRESETS["none"]["flags"] == []


# ── Memory recommendation ────────────────────────────────────────

class TestRecommendMemory(unittest.TestCase):
    def test_vanilla_gets_at_least_1536_mib(self):
        result = recommend_memory_mib(weight="vanilla", system_ram_kib=16 * 1024 * 1024)
        assert result >= 1536

    def test_heavy_gets_more_than_light(self):
        light = recommend_memory_mib(weight="light", system_ram_kib=16 * 1024 * 1024)
        heavy = recommend_memory_mib(weight="heavy", system_ram_kib=16 * 1024 * 1024)
        assert heavy > light

    def test_high_mod_count_bumps_memory(self):
        base = recommend_memory_mib(weight="heavy", system_ram_kib=32 * 1024 * 1024, mod_count=80)
        extra = recommend_memory_mib(weight="heavy", system_ram_kib=32 * 1024 * 1024, mod_count=200)
        assert extra >= base

    def test_never_exceed_60pct_system_ram(self):
        small_sys = 4 * 1024 * 1024  # 4 GiB in KiB
        result = recommend_memory_mib(weight="heavy", system_ram_kib=small_sys, mod_count=400)
        ceiling = (small_sys * 6) // (10 * 1024)  # 60 % in MiB
        assert result <= ceiling

    def test_always_positive(self):
        assert recommend_memory_mib(weight="vanilla", system_ram_kib=0) > 0

    def test_legacy_gets_low_memory(self):
        result = recommend_memory_mib(weight="legacy", system_ram_kib=8 * 1024 * 1024)
        assert result <= 2048


# ── Instance classification ───────────────────────────────────────

class TestClassifyWeight(unittest.TestCase):
    def test_no_loader_is_vanilla(self):
        assert classify_instance_weight(0, "") == "vanilla"
        assert classify_instance_weight(100, "vanilla") == "vanilla"

    def test_few_mods_is_light(self):
        assert classify_instance_weight(5, "fabric") == "light"

    def test_many_mods_is_heavy(self):
        assert classify_instance_weight(120, "forge") == "heavy"

    def test_medium_threshold(self):
        assert classify_instance_weight(30, "forge") == "medium"


# ── Mod counting ────────────────────────────────────────────────────

class TestCountMods(unittest.TestCase):
    def test_counts_jar_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            instance_dir = Path(tmpdir)
            mods_dir = instance_dir / "mods"
            mods_dir.mkdir()
            for i in range(5):
                (mods_dir / f"mod{i}.jar").write_text("fake", encoding="utf-8")
            (mods_dir / "readme.txt").write_text("not a mod", encoding="utf-8")
            assert count_mods_in_instance(instance_dir) == 5

    def test_empty_directory(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            instance_dir = Path(tmpdir)
            (instance_dir / "mods").mkdir()
            assert count_mods_in_instance(instance_dir) == 0

    def test_no_mods_dir(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            assert count_mods_in_instance(Path(tmpdir)) == 0

    def test_counts_litemod_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            instance_dir = Path(tmpdir)
            mods_dir = instance_dir / "mods"
            mods_dir.mkdir()
            (mods_dir / "something.litemod").write_text("x", encoding="utf-8")
            assert count_mods_in_instance(instance_dir) == 1


# ── Optimizer mod recommendations ─────────────────────────────────

class TestRecommendMods(unittest.TestCase):
    def test_sodium_for_fabric_1_18(self):
        mods = recommend_optimizer_mods(loader="fabric", mc_version="1.18.2")
        names = [m["name"] for m in mods]
        assert "Sodium" in names

    def test_ferrite_for_forge(self):
        mods = recommend_optimizer_mods(loader="forge", mc_version="1.20.1")
        names = [m["name"] for m in mods]
        assert "FerriteCore" in names

    def test_sodium_not_for_forge(self):
        mods = recommend_optimizer_mods(loader="forge", mc_version="1.20.1")
        names = [m["name"] for m in mods]
        assert "Sodium" not in names

    def test_already_installed_flagged(self):
        mods = recommend_optimizer_mods(loader="fabric", mc_version="1.20.1", installed_mod_ids={"sodium"})
        sodium = next((m for m in mods if m["name"] == "Sodium"), None)
        assert sodium is not None
        assert sodium["already_installed"] is True

    def test_old_version_excludes_modern_mods(self):
        mods = recommend_optimizer_mods(loader="fabric", mc_version="1.12.2")
        names = [m["name"] for m in mods]
        # Sodium requires 1.16
        assert "Sodium" not in names

    def test_returns_list_of_dicts(self):
        mods = recommend_optimizer_mods(loader="fabric", mc_version="1.20.1")
        assert isinstance(mods, list)
        for m in mods:
            assert "name" in m
            assert "mod_id" in m
            assert "category" in m


# ── Launch history ─────────────────────────────────────────────────

class TestLaunchHistory(unittest.TestCase):
    def test_record_start_and_end(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            instance_dir = Path(tmpdir)
            lid = "test-launch-1"
            record_launch_start(instance_dir, lid, loader="fabric", preset="balanced", memory="4GiB", mc_version="1.20.1")
            history = read_launch_history(instance_dir)
            assert len(history) == 1
            assert history[0]["launch_id"] == lid
            assert history[0]["preset"] == "balanced"
            assert history[0]["exit_code"] is None

            record_launch_end(instance_dir, lid, exit_code=0, elapsed_seconds=120.5)
            history2 = read_launch_history(instance_dir)
            assert history2[0]["exit_code"] == 0
            assert history2[0]["elapsed_seconds"] == 120.5

    def test_history_trimmed_to_max_20(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            instance_dir = Path(tmpdir)
            for i in range(25):
                record_launch_start(instance_dir, f"lid-{i}", loader="forge", preset="fps", memory="4GiB", mc_version="1.20.1")
            history = read_launch_history(instance_dir)
            assert len(history) <= 20

    def test_empty_history_for_new_instance(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            assert read_launch_history(Path(tmpdir)) == []


# ── Cache helpers ──────────────────────────────────────────────────

class TestCacheHelpers(unittest.TestCase):
    def test_cache_stats_empty_dir(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            stats = cache_stats(Path(tmpdir))
            assert stats["total_bytes"] == 0
            assert stats["file_count"] == 0

    def test_cache_stats_counts_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            p = Path(tmpdir)
            (p / "a.json").write_text("x" * 100, encoding="utf-8")
            (p / "b.json").write_text("y" * 200, encoding="utf-8")
            stats = cache_stats(p)
            assert stats["file_count"] == 2
            assert stats["total_bytes"] >= 300

    def test_prune_deletes_old_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            p = Path(tmpdir)
            old_file = p / "old.json"
            old_file.write_text("{}", encoding="utf-8")
            # Set mtime to 72 hours ago
            old_time = time.time() - 72 * 3600
            os.utime(old_file, (old_time, old_time))
            new_file = p / "new.json"
            new_file.write_text("{}", encoding="utf-8")
            deleted = prune_stale_metadata_cache(p, max_age_hours=48)
            assert deleted == 1
            assert not old_file.exists()
            assert new_file.exists()

    def test_prune_keeps_recent_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            p = Path(tmpdir)
            (p / "recent.json").write_text("{}", encoding="utf-8")
            deleted = prune_stale_metadata_cache(p, max_age_hours=48)
            assert deleted == 0


# ── analyze_instance ────────────────────────────────────────────────

class TestAnalyzeInstance(unittest.TestCase):
    def test_returns_report_object(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="fabric",
                mc_version="1.20.1",
            )
            assert isinstance(report, OptimizationReport)

    def test_heavy_modpack_recommends_aikar_preset(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            mods_dir = Path(tmpdir) / "mods"
            mods_dir.mkdir()
            # Create 100 fake mod jars
            for i in range(100):
                (mods_dir / f"mod{i}.jar").write_text("x", encoding="utf-8")
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="forge",
                mc_version="1.20.1",
            )
            assert report.weight == "heavy"
            assert report.recommended_preset_id == "heavy_modpack"

    def test_vanilla_recommends_fps_preset(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="",
                mc_version="1.20.1",
            )
            assert report.weight == "vanilla"
            assert report.recommended_preset_id == "fps"

    def test_legacy_version_recommends_compat(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="forge",
                mc_version="1.12.2",
            )
            assert report.weight == "legacy"
            assert report.recommended_preset_id == "compatibility"

    def test_to_dict_has_expected_keys(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="fabric",
                mc_version="1.20.1",
            )
            d = report.to_dict()
            assert "system" in d
            assert "instance" in d
            assert "recommendations" in d
            assert "current" in d
            recs = d["recommendations"]
            assert "memory_mib" in recs
            assert "preset_id" in recs
            assert "optimizer_mods" in recs
            assert "warnings" in recs

    def test_low_memory_warning(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="fabric",
                mc_version="1.20.1",
                current_memory="512MiB",
            )
            warnings_text = " ".join(report.warnings)
            assert len(report.warnings) > 0

    def test_system_info_present(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            report = analyze_instance(
                instance_dir=Path(tmpdir),
                loader="fabric",
                mc_version="1.20.1",
            )
            d = report.to_dict()
            assert d["system"]["cpu_count"] >= 1
            assert d["system"]["os"] != ""


# ── system_ram_kib ───────────────────────────────────────────────

class TestSystemRamKib(unittest.TestCase):
    def test_returns_non_negative(self):
        result = system_ram_kib()
        assert result >= 0

    def test_returns_int(self):
        assert isinstance(system_ram_kib(), int)


# ── Config integration ────────────────────────────────────────────

class TestConfigIntegration(unittest.TestCase):
    def test_default_profile_has_jvm_preset(self):
        from nivora_launcher.config import DEFAULT_LAUNCHER_PROFILE
        assert "jvm_preset" in DEFAULT_LAUNCHER_PROFILE
        assert DEFAULT_LAUNCHER_PROFILE["jvm_preset"] == "none"

    def test_normalize_preserves_valid_preset(self):
        from nivora_launcher.config import _safe_preset_id
        assert _safe_preset_id("fps") == "fps"
        assert _safe_preset_id("heavy_modpack") == "heavy_modpack"
        assert _safe_preset_id("unknown") == "none"
        assert _safe_preset_id("") == "none"

    def test_normalize_launcher_profiles_preserves_jvm_preset(self):
        from nivora_launcher.config import _normalize_launcher_profiles
        profiles = _normalize_launcher_profiles([{
            "id": "test",
            "name": "Test",
            "memory": "auto",
            "java_path": "",
            "graphics_mode": "auto",
            "jvm_preset": "fps",
            "jvm_arguments": [],
            "game_arguments": [],
        }])
        assert profiles[0]["jvm_preset"] == "fps"

    def test_normalize_strips_invalid_preset(self):
        from nivora_launcher.config import _normalize_launcher_profiles
        profiles = _normalize_launcher_profiles([{
            "id": "test",
            "name": "Test",
            "jvm_preset": "evil_preset",
        }])
        assert profiles[0]["jvm_preset"] == "none"


# ── Launcher JVM injection ────────────────────────────────────────

class TestLauncherJvmInjection(unittest.TestCase):
    """Verify that JVM preset flags are injected into build_command."""

    def test_preset_flags_prepended_to_jvm_args(self):
        """build_command should prepend preset flags and strip duplicate GC args."""
        from unittest.mock import MagicMock, patch
        import importlib
        launcher_mod = importlib.import_module("nivora_launcher.launcher")
        gl = launcher_mod.GameLauncher.__new__(launcher_mod.GameLauncher)
        # Minimal config mock
        config = MagicMock()
        config.settings.java_path = ""
        config.settings.memory = "2GiB"
        config.settings.launcher_version = "0.1.0"
        config.settings.client_id = "client-id"
        config.paths.data_home = Path("/tmp")
        gl.config = config
        gl.runtime = MagicMock()
        java_path = MagicMock()
        gl.runtime.java_for.return_value = java_path

        version_json = {
            "arguments": {"jvm": ["-Xms256M"], "game": []},
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id": "17"},
            "javaVersion": {"majorVersion": 17},
        }
        install = MagicMock()
        install.version_id = "1.20.1"
        install.version_json = version_json
        install.assets_root = Path("/tmp/assets")
        install.asset_index_name = "17"
        install.natives_dir = Path("/tmp/natives")
        install.libraries = [Path("/tmp/lib.jar")]
        install.client_jar = Path("/tmp/client.jar")
        install.logging_file = None
        install.instance_dir = Path("/tmp")

        account = MagicMock()
        account.username = "Player"
        account.uuid = "uuid"
        account.access_token = ""
        account.xuid = ""
        account.kind = "offline"

        cmd = gl.build_command(install, account, {"jvm_preset": "fps", "memory": "2GiB"})
        cmd_str = " ".join(str(c) for c in cmd)
        assert "UseG1GC" in cmd_str, "FPS preset G1GC flag missing"
        assert "MaxGCPauseMillis=50" in cmd_str, "FPS MaxGCPauseMillis missing"

    def test_none_preset_does_not_inject_gc_flags(self):
        from unittest.mock import MagicMock
        import importlib
        launcher_mod = importlib.import_module("nivora_launcher.launcher")
        gl = launcher_mod.GameLauncher.__new__(launcher_mod.GameLauncher)
        config = MagicMock()
        config.settings.java_path = ""
        config.settings.memory = "2GiB"
        config.settings.launcher_version = "0.1.0"
        config.settings.client_id = "client-id"
        config.paths.data_home = Path("/tmp")
        gl.config = config
        gl.runtime = MagicMock()
        gl.runtime.java_for.return_value = MagicMock()

        version_json = {
            "arguments": {"jvm": [], "game": []},
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id": "17"},
        }
        install = MagicMock()
        install.version_id = "1.20.1"
        install.version_json = version_json
        install.assets_root = Path("/tmp/assets")
        install.asset_index_name = "17"
        install.natives_dir = Path("/tmp/natives")
        install.libraries = []
        install.client_jar = Path("/tmp/client.jar")
        install.logging_file = None
        install.instance_dir = Path("/tmp")

        account = MagicMock()
        account.username = "Player"
        account.uuid = "uuid"
        account.access_token = ""
        account.xuid = ""
        account.kind = "offline"

        cmd = gl.build_command(install, account, {"jvm_preset": "none", "memory": "2GiB"})
        cmd_str = " ".join(str(c) for c in cmd)
        assert "UseG1GC" not in cmd_str


class TestLegacyJvmSafeguards(unittest.TestCase):
    """Старые версии стартуют на Java 8: лишний флаг или большая куча на
    32-бит сборке убивают JVM до main() («Error occurred during
    initialization of VM»). Проверяем обе страховки."""

    def _launcher(self):
        from unittest.mock import MagicMock
        import importlib
        launcher_mod = importlib.import_module("nivora_launcher.launcher")
        gl = launcher_mod.GameLauncher.__new__(launcher_mod.GameLauncher)
        config = MagicMock()
        config.settings.java_path = ""
        config.settings.memory = "2GiB"
        config.settings.launcher_version = "0.1.0"
        config.settings.client_id = "client-id"
        config.paths.data_home = Path("/tmp")
        gl.config = config
        gl.runtime = MagicMock()
        gl.runtime.java_for.return_value = MagicMock()
        return launcher_mod, gl

    def _install(self, launcher_mod, version_id, version_json):
        from unittest.mock import MagicMock
        install = MagicMock()
        install.version_id = version_id
        install.version_json = version_json
        install.assets_root = Path("/tmp/assets")
        install.asset_index_name = "17"
        install.natives_dir = Path("/tmp/natives")
        install.libraries = []
        install.client_jar = Path("/tmp/client.jar")
        install.logging_file = None
        install.instance_dir = Path("/tmp")
        return install

    def _account(self):
        from unittest.mock import MagicMock
        account = MagicMock()
        account.username = "Player"
        account.uuid = "uuid"
        account.access_token = ""
        account.xuid = ""
        account.kind = "offline"
        return account

    def test_legacy_version_starts_with_ignore_unrecognized_vm_options(self):
        launcher_mod, gl = self._launcher()
        version_json = {
            "minecraftArguments": "${auth_uuid} ${auth_player_name}",
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id": "17"},
        }
        install = self._install(launcher_mod, "1.12.2", version_json)
        cmd = gl.build_command(install, self._account(), {"memory": "2GiB"})
        self.assertEqual(cmd[1], "-XX:+IgnoreUnrecognizedVMOptions")

    def test_modern_version_keeps_arguments_untouched(self):
        launcher_mod, gl = self._launcher()
        version_json = {
            "arguments": {"jvm": [], "game": []},
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id": "17"},
        }
        install = self._install(launcher_mod, "1.20.1", version_json)
        cmd = gl.build_command(install, self._account(), {"memory": "2GiB"})
        self.assertNotIn("-XX:+IgnoreUnrecognizedVMOptions", cmd)

    def test_heap_is_capped_on_32bit_java(self):
        launcher_mod, gl = self._launcher()
        notes: list[str] = []
        command = ["/opt/java32/bin/java", "-Xmx2G", "-cp", "client.jar", "Main"]
        with patch("nivora_launcher.launcher.java_bitness", return_value=32):
            capped = launcher_mod._cap_heap_for_32bit_java(command, notes.append)
        self.assertIn("-Xmx1024M", capped)
        self.assertNotIn("-Xmx2G", capped)
        self.assertTrue(notes)

    def test_heap_is_kept_on_64bit_java(self):
        launcher_mod, _ = self._launcher()
        command = ["/opt/java/bin/java", "-Xmx4G", "-cp", "client.jar", "Main"]
        with patch("nivora_launcher.launcher.java_bitness", return_value=64):
            capped = launcher_mod._cap_heap_for_32bit_java(command)
        self.assertEqual(capped, command)

    def test_unknown_bitness_keeps_command(self):
        launcher_mod, _ = self._launcher()
        command = ["/opt/java/bin/java", "-Xmx4G", "Main"]
        with patch("nivora_launcher.launcher.java_bitness", return_value=None):
            self.assertEqual(launcher_mod._cap_heap_for_32bit_java(command), command)

    def test_java8_gets_ignore_unrecognized_even_on_modern_mc(self):
        # MC 1.13–1.16.5 ходит на Java 8, но «legacy»-веткой не считается.
        # Пресет FPS подсовывает -XX:+ParallelRefProcEnabled (JDK 9+) —
        # без приёмника неизвестных флагов JVM умирает на старте.
        launcher_mod, _ = self._launcher()
        command = ["/opt/java8/bin/java", "-XX:+ParallelRefProcEnabled", "-cp", "client.jar", "Main"]
        with patch("nivora_launcher.launcher.java_probe", return_value=(8, 64)):
            adapted = launcher_mod._adapt_command_for_old_java(command)
        self.assertEqual(adapted[0], "-XX:+IgnoreUnrecognizedVMOptions")
        self.assertIn("-XX:+ParallelRefProcEnabled", adapted)

    def test_modern_java_command_is_untouched(self):
        launcher_mod, _ = self._launcher()
        command = ["/opt/java/bin/java", "-XX:+ParallelRefProcEnabled", "Main"]
        with patch("nivora_launcher.launcher.java_probe", return_value=(17, 64)):
            self.assertEqual(launcher_mod._adapt_command_for_old_java(command), command)

    def test_java8_32bit_gets_both_heap_cap_and_flag(self):
        launcher_mod, _ = self._launcher()
        notes: list[str] = []
        command = ["/opt/java8/bin/java", "-Xmx2G", "-XX:+ParallelRefProcEnabled", "Main"]
        with patch("nivora_launcher.launcher.java_probe", return_value=(8, 32)):
            adapted = launcher_mod._adapt_command_for_old_java(command, notes.append)
        self.assertEqual(adapted[0], "-XX:+IgnoreUnrecognizedVMOptions")
        self.assertIn("-Xmx1024M", adapted)
        self.assertNotIn("-Xmx2G", adapted)
        self.assertTrue(notes)

    def test_silent_java_probe_keeps_command(self):
        launcher_mod, _ = self._launcher()
        command = ["/opt/java/bin/java", "-Xmx2G", "Main"]
        with patch("nivora_launcher.launcher.java_probe", return_value=(None, None)):
            self.assertEqual(launcher_mod._adapt_command_for_old_java(command), command)

    def test_jvm_xmx_parsing(self):
        launcher_mod, _ = self._launcher()
        self.assertEqual(launcher_mod._jvm_xmx_kib("2G"), 2 * 1024 * 1024)
        self.assertEqual(launcher_mod._jvm_xmx_kib("512m"), 512 * 1024)
        self.assertEqual(launcher_mod._jvm_xmx_kib("4096k"), 4096)
        self.assertEqual(launcher_mod._jvm_xmx_kib("1024"), 1024 * 1024)
        self.assertIsNone(launcher_mod._jvm_xmx_kib("banana"))
