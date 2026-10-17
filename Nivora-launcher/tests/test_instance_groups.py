from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.constants import MAX_INSTANCE_VERSION_JSON_BYTES
from nivora_launcher.errors import LauncherError
from nivora_launcher.instances import INSTANCE_METADATA_FILE, InstanceStore


class InstanceGroupTests(unittest.TestCase):
    def test_corrupt_live_state_does_not_break_instance_listing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.set_value("game_dir", str(root / "game"))
                store = InstanceStore(config)
                profile = store.root / "broken-state"
                profile.mkdir()
                (profile / INSTANCE_METADATA_FILE).write_text("{}", encoding="utf-8")
                (profile / ".trial-launcher-live.json").write_text("[]", encoding="utf-8")

                instance = store.get(profile.name)

        self.assertFalse(instance.running)
        self.assertEqual(instance.played_seconds, 0)

    def test_oversized_version_metadata_is_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.set_value("game_dir", str(root / "game"))
                store = InstanceStore(config)
                profile = store.root / "oversized-metadata"
                metadata = profile / "versions" / "fabric-test" / "fabric-test.json"
                metadata.parent.mkdir(parents=True)
                (profile / INSTANCE_METADATA_FILE).write_text("{}", encoding="utf-8")
                with metadata.open("wb") as handle:
                    handle.truncate(MAX_INSTANCE_VERSION_JSON_BYTES + 1)

                instance = store.get(profile.name)

        self.assertEqual(instance.version_id, "")
        self.assertEqual(instance.minecraft_version, "")

    def test_group_assignment_and_empty_group_deletion(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.set_value("game_dir", str(root / "game"))
                store = InstanceStore(config)
                profile = store.root / "survival"
                profile.mkdir()
                (profile / INSTANCE_METADATA_FILE).write_text(
                    json.dumps({"display_name": "Survival", "kind": "client", "group": ""}),
                    encoding="utf-8",
                )

                self.assertEqual(store.create_group("  Co-op  "), ["Co-op"])
                assigned = store.update_profile(
                    "survival",
                    display_name="Survival",
                    description="Friends world",
                    group="Co-op",
                )
                self.assertEqual(assigned.group, "Co-op")
                with self.assertRaisesRegex(LauncherError, "пока в ней есть экземпляры"):
                    store.delete_group("Co-op")

                unassigned = store.update_profile(
                    "survival",
                    display_name="Survival",
                    description="Friends world",
                    group="",
                )
                self.assertEqual(unassigned.group, "")
                self.assertEqual(store.delete_group("Co-op"), [])

    def test_group_names_are_trimmed_and_cannot_be_blank(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                store = InstanceStore(config)
                self.assertEqual(store.create_group("  Local  "), ["Local"])
                self.assertEqual(store.create_group("Local"), ["Local"])
                with self.assertRaisesRegex(LauncherError, "Название группы не может быть пустым"):
                    store.create_group("   ")

    def test_rename_group_moves_instances_and_keeps_pins(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.set_value("game_dir", str(root / "game"))
                store = InstanceStore(config)
                profile = store.root / "survival"
                profile.mkdir()
                (profile / INSTANCE_METADATA_FILE).write_text(
                    json.dumps({"display_name": "Survival", "kind": "client", "group": "Co-op"}),
                    encoding="utf-8",
                )
                store.create_group("Co-op")
                store.create_group("Другое")
                config.set_value("pinned_instance_groups", ["Co-op"])

                groups = store.rename_group("Co-op", "  Совместная  ")
                self.assertEqual(groups, ["Другое", "Совместная"])
                self.assertEqual(store.get("survival").group, "Совместная")
                # Закрепление переживает переименование
                self.assertEqual(config.settings.pinned_instance_groups, ["Совместная"])

                # Дубликат имени и пустое имя запрещены
                with self.assertRaisesRegex(LauncherError, "уже существует"):
                    store.rename_group("Совместная", "Другое")
                with self.assertRaisesRegex(LauncherError, "не может быть пустым"):
                    store.rename_group("Совместная", "   ")
                with self.assertRaisesRegex(LauncherError, "Группа не найдена"):
                    store.rename_group("Нет такой", "X")
                # Идемпотентность: то же имя — без ошибок
                self.assertEqual(store.rename_group("Совместная", "Совместная"), ["Другое", "Совместная"])

    def test_pinned_groups_are_limited_saved_and_pruned_when_groups_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.set_values(
                    {
                        "instance_groups": ["Alpha", "Beta", "Gamma", "Delta"],
                        "pinned_instance_groups": ["Gamma", "Alpha", "Beta", "Delta", "Gamma"],
                    }
                )
                self.assertEqual(config.settings.pinned_instance_groups, ["Gamma", "Alpha", "Beta"])
                self.assertEqual(ConfigStore(Paths()).settings.pinned_instance_groups, ["Gamma", "Alpha", "Beta"])

                config.set_value("instance_groups", ["Alpha", "Gamma"])
                self.assertEqual(config.settings.pinned_instance_groups, ["Gamma", "Alpha"])

    def test_prism_instances_can_be_discovered_and_imported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / "home"
            prism = home / ".local/share/PrismLauncher/instances/Friends Pack"
            minecraft = prism / "minecraft"
            minecraft.mkdir(parents=True)
            (prism / "instance.cfg").write_text("name=Friends Pack\n", encoding="utf-8")
            (prism / "mmc-pack.json").write_text(
                json.dumps(
                    {
                        "components": [
                            {"uid": "net.minecraft", "version": "1.20.1"},
                            {"uid": "net.fabricmc.fabric-loader", "version": "0.15.11"},
                        ]
                    }
                ),
                encoding="utf-8",
            )
            (minecraft / "mods").mkdir()
            (minecraft / "mods" / "example.jar").write_bytes(b"jar")
            with patch.dict(
                os.environ,
                {
                    "HOME": str(home),
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.set_value("game_dir", str(root / "game"))
                store = InstanceStore(config)

                candidates = store.list_import_candidates()
                self.assertEqual(len(candidates), 1)
                self.assertEqual(candidates[0].launcher, "Prism Launcher")
                self.assertEqual(candidates[0].minecraft_version, "1.20.1")
                self.assertEqual(candidates[0].loader, "fabric")

                imported = store.import_external_instance(str(candidates[0].source_path))
                self.assertEqual(imported.display_name, "Friends Pack")
                self.assertEqual(imported.loader, "fabric")
                self.assertTrue((imported.path / "mods" / "example.jar").is_file())
                self.assertTrue(any((imported.path / "versions").glob("*/*.json")))

    def test_curseforge_instances_are_detected_with_loader_requirements(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / "home"
            pack = home / "Documents/curseforge/minecraft/Instances/Better Pack"
            pack.mkdir(parents=True)
            (pack / "manifest.json").write_text(
                json.dumps(
                    {
                        "name": "Better Pack",
                        "minecraft": {
                            "version": "1.19.2",
                            "modLoaders": [{"id": "forge-43.3.0", "primary": True}],
                        },
                    }
                ),
                encoding="utf-8",
            )
            with patch.dict(
                os.environ,
                {
                    "HOME": str(home),
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                store = InstanceStore(config)
                candidates = store.list_import_candidates()

        self.assertEqual(len(candidates), 1)
        self.assertEqual(candidates[0].launcher, "CurseForge")
        self.assertEqual(candidates[0].display_name, "Better Pack")
        self.assertEqual(candidates[0].minecraft_version, "1.19.2")
        self.assertEqual(candidates[0].loader, "forge")

    def test_official_launcher_root_is_detected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / "home"
            version_dir = home / ".minecraft/versions/1.21.1"
            version_dir.mkdir(parents=True)
            (version_dir / "1.21.1.json").write_text(json.dumps({"id": "1.21.1"}), encoding="utf-8")
            with patch.dict(
                os.environ,
                {
                    "HOME": str(home),
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                store = InstanceStore(config)
                candidates = store.list_import_candidates()

        self.assertEqual(len(candidates), 1)
        self.assertEqual(candidates[0].launcher, "Minecraft Launcher")
        self.assertEqual(candidates[0].display_name, "Official Minecraft")
        self.assertEqual(candidates[0].minecraft_version, "1.21.1")
        self.assertEqual(candidates[0].loader, "vanilla")


if __name__ == "__main__":
    unittest.main()
