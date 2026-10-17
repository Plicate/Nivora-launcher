"""Тесты лёгкого Python-слоя Bedrock.

Тяжёлая логика (установка версий, вход в Store, запуск игры, контент,
фоновые задания) переехала в Rust-движок и покрыта его тестами
(frontend/src-tauri/src/rust_engine/bedrock.rs). Здесь проверяется
только то, что осталось в Python: пути профилей для вкладки «Файлы»,
удаление профиля при сносе экземпляра и официальный каталог Windows.
"""
from __future__ import annotations

import fcntl
import hashlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.bedrock import (
    MAX_CONTENT_UPLOAD_BYTES,
    BedrockProfileStore,
    _profile_name,
    windows_content_root,
)
from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.instances import INSTANCE_METADATA_FILE, InstanceInfo, InstanceStore

# Тот же файл, который поставляется с приложением и проверяется Rust-движком.
_VENDOR_PYZ = Path(__file__).parent.parent / "nivora_launcher" / "vendor" / "bedrock-on-linux-2.2.9.pyz"
_BUNDLED_SHA256 = "bb894a86ed19575635d63f11cef466b70da2b99a12a3bda8468386630c40ed5c"


def make_info(name: str, bedrock_profile: str, path: Path) -> InstanceInfo:
    return InstanceInfo(
        name=name,
        path=path,
        version_id="",
        minecraft_version="",
        loader="vanilla",
        display_name="",
        description="",
        group="",
        icon="",
        banner="",
        played_seconds=0,
        kind="client",
        game_edition="bedrock",
        bedrock_profile=bedrock_profile,
        server_core="",
        server_core_version="",
        server_port=0,
        server_eula=False,
        running=False,
        notes="",
        tags=[],
    )


class BedrockShimTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.environment = patch.dict(
            os.environ,
            {
                "XDG_CONFIG_HOME": str(self.root / "config"),
                "XDG_DATA_HOME": str(self.root / "data"),
                "XDG_CACHE_HOME": str(self.root / "cache"),
            },
            clear=False,
        )
        self.environment.start()
        self.config = ConfigStore(Paths())
        self.config.set_value("game_dir", str(self.root / "game"))
        self.instances = InstanceStore(self.config)
        self.bedrock = BedrockProfileStore(self.config, self.instances)

    def tearDown(self) -> None:
        self.environment.stop()
        self.temporary.cleanup()

    def make_profile(self, name: str = "bedrock-one") -> tuple[InstanceInfo, Path]:
        profile_name = _profile_name(name)
        profile = self.bedrock.profiles_root / profile_name
        profile.mkdir(parents=True)
        (profile / "profile.json").write_text(
            json.dumps({"name": profile_name, "slug": profile_name}),
            encoding="utf-8",
        )
        instance_path = self.instances.root / name
        instance_path.mkdir(parents=True, exist_ok=True)
        (instance_path / INSTANCE_METADATA_FILE).write_text(
            json.dumps({"kind": "client", "game_edition": "bedrock", "bedrock_profile": profile_name}),
            encoding="utf-8",
        )
        return make_info(name, profile_name, instance_path), profile

    def test_profile_names_are_stable_sha_prefixes(self) -> None:
        digest = hashlib.sha256(b"bedrock-one").hexdigest()
        self.assertEqual(_profile_name("bedrock-one"), f"nivora-{digest[:16]}")

    def test_bundled_pyz_hash_matches_pinned_constant(self) -> None:
        # Целостность встроенной среды BedrockOnLinux: тот же SHA-256, что
        # проверяет Rust-движок при запуске (rust_engine/bedrock.rs).
        self.assertTrue(_VENDOR_PYZ.is_file(), "bedrock-on-linux-2.2.9.pyz должен поставляться с приложением")
        self.assertEqual(hashlib.sha256(_VENDOR_PYZ.read_bytes()).hexdigest(), _BUNDLED_SHA256)

    def test_upload_limit_matches_rust_engine(self) -> None:
        # Паритет лимитов Python-воркера и Rust-движка.
        self.assertEqual(MAX_CONTENT_UPLOAD_BYTES, 128 * 1024 * 1024)

    def test_profile_path_resolves_valid_profile(self) -> None:
        info, profile = self.make_profile()
        self.assertEqual(self.bedrock.profile_path(info), profile)

    def test_profile_path_rejects_mismatched_profile(self) -> None:
        info, _ = self.make_profile("bedrock-one")
        wrong = make_info("bedrock-one", "nivora-deadbeefdeadbeef", info.path)
        with self.assertRaisesRegex(LauncherError, "не совпадает"):
            self.bedrock.profile_path(wrong)

    def test_profile_path_rejects_missing_and_tampered_profiles(self) -> None:
        info, profile = self.make_profile()
        # slug в profile.json не совпадает с именем профиля
        (profile / "profile.json").write_text(json.dumps({"slug": "nivora-other"}), encoding="utf-8")
        with self.assertRaisesRegex(LauncherError, "не совпадает"):
            self.bedrock.profile_path(info)
        # профиль исчез
        import shutil

        shutil.rmtree(profile)
        with self.assertRaisesRegex(LauncherError, "не найден"):
            self.bedrock.profile_path(info)

    def test_profile_path_rejects_other_editions(self) -> None:
        info, _ = self.make_profile()
        java = InstanceInfo(**{**info.__dict__, "game_edition": "java"})
        with self.assertRaisesRegex(LauncherError, "только для Minecraft Bedrock"):
            self.bedrock.profile_path(java)

    def test_delete_profile_removes_profile_and_shortcuts(self) -> None:
        info, profile = self.make_profile()
        applications = self.bedrock.root / ".nivora-xdg-data" / "applications"
        applications.mkdir(parents=True, exist_ok=True)
        shortcut = applications / f"bedrock-on-linux-profile-{_profile_name(info.name)}.desktop"
        shortcut.write_text("[Desktop Entry]", encoding="utf-8")
        self.bedrock.delete_profile(info)
        self.assertFalse(profile.exists())
        self.assertFalse(shortcut.exists())

    def test_delete_profile_is_noop_without_profile(self) -> None:
        info, profile = self.make_profile()
        import shutil

        shutil.rmtree(profile)
        self.bedrock.delete_profile(info)  # не должна падать

    def test_delete_profile_rejects_busy_lock(self) -> None:
        info, profile = self.make_profile()
        lock_path = profile / ".launch.lock"
        with open(lock_path, "w", encoding="utf-8"):
            pass
        with open(lock_path, "r", encoding="utf-8") as held:
            fcntl.flock(held.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                with self.assertRaisesRegex(LauncherError, "используется"):
                    self.bedrock.delete_profile(info)
            finally:
                fcntl.flock(held.fileno(), fcntl.LOCK_UN)

    def test_windows_content_root_uses_official_package_layout(self) -> None:
        with patch.dict(os.environ, {"LOCALAPPDATA": str(self.root / "appdata")}, clear=False):
            root = windows_content_root("release")
            self.assertEqual(
                root,
                Path(self.root / "appdata")
                / "Packages"
                / "Microsoft.MinecraftUWP_8wekyb3d8bbwe"
                / "LocalState"
                / "games"
                / "com.mojang",
            )
            preview = windows_content_root("preview")
            self.assertIn("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe", str(preview))

    def test_windows_content_root_requires_localappdata(self) -> None:
        environment = {key: value for key, value in os.environ.items() if key != "LOCALAPPDATA"}
        with patch.dict(os.environ, environment, clear=True):
            with self.assertRaisesRegex(LauncherError, "LOCALAPPDATA"):
                windows_content_root()

    def test_profile_path_on_windows_targets_official_game_data(self) -> None:
        info, _ = self.make_profile("bedrock-one")
        metadata = info.path / INSTANCE_METADATA_FILE
        metadata.write_text(
            json.dumps(
                {
                    "kind": "client",
                    "game_edition": "bedrock",
                    "bedrock_profile": "",
                    "bedrock_edition": "preview",
                }
            ),
            encoding="utf-8",
        )
        with patch.dict(os.environ, {"LOCALAPPDATA": str(self.root / "appdata")}, clear=False):
            with patch.object(sys, "platform", "win32"):
                root = self.bedrock.profile_path(info)
        self.assertIn("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe", str(root))
        self.assertTrue(str(root).endswith("LocalState/games/com.mojang") or root.parts[-3:] == ("LocalState", "games", "com.mojang"))

    def test_delete_profile_on_windows_is_noop(self) -> None:
        info, _ = self.make_profile("bedrock-one")
        with patch.object(sys, "platform", "win32"):
            self.bedrock.delete_profile(info)  # официальной версии чистить нечего


class BedrockWorkerRoutingTests(unittest.TestCase):
    """Bedrock-операции обслуживает Rust-движок: чистый Python-воркер
    (например, в web-режиме) больше их не принимает и честно об этом говорит."""

    def test_bedrock_operations_are_not_handled_by_python_worker(self) -> None:
        for operation in (
            "bedrock_runtime_status",
            "bedrock_versions",
            "bedrock_setup_version",
            "bedrock_create_instance",
            "bedrock_content_list",
            "bedrock_stop",
        ):
            with self.subTest(operation=operation):
                with self.assertRaisesRegex(LauncherError, "Неизвестная GUI-операция"):
                    handle({"operation": operation, "name": "bedrock-one"})


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
