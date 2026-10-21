from __future__ import annotations

import gzip
import json
import hashlib
import os
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.accounts import Account, AccountStore, OfflineDebugAccount
from nivora_launcher.errors import LauncherError
from nivora_launcher.instance_workspace import InstanceWorkspace, _read_nbt
from nivora_launcher.instances import InstanceStore
from nivora_launcher.gui_service import _effective_launch_settings, _instance_mod_context, handle
from nivora_launcher.installer import _safe_extract, _validate_loader_installer_url
from nivora_launcher.launcher import GameLauncher, _AUTH_TOKEN_PLACEHOLDER, _create_java_argument_file, _looks_like_glx_context_error, _materialize_auth_token
from nivora_launcher.modpacks import ModpackImporter, ModpackProvider
from nivora_launcher.security import _sandbox_java_path, load_protected_settings, prepare_process
from nivora_launcher.server import ServerFiles
from nivora_launcher.snapshots import MAX_SNAPSHOT_MANIFEST_BYTES, SnapshotStore


class SecurityRegressionTests(unittest.TestCase):
    def test_sandbox_java_in_home_bin_does_not_mount_the_home_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory) / "home"
            java_dir = home / "bin"
            java_dir.mkdir(parents=True)
            java = java_dir / "java"
            java.write_bytes(b"java")

            with patch("nivora_launcher.security.Path.home", return_value=home):
                root, mapped = _sandbox_java_path(str(java))

        self.assertEqual(root, java_dir)
        self.assertEqual(mapped, "/trial-java/java")

    def test_sandbox_java_maps_only_a_detected_runtime_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory) / "jdk"
            (runtime / "bin").mkdir(parents=True)
            (runtime / "lib").mkdir()
            (runtime / "release").write_text("JAVA_VERSION=\"21\"\n", encoding="utf-8")
            java = runtime / "bin" / "java"
            java.write_bytes(b"java")

            root, mapped = _sandbox_java_path(str(java))

        self.assertEqual(root, runtime)
        self.assertEqual(mapped, "/trial-java/bin/java")

    def test_sandbox_java_accepts_runtime_without_release_file(self) -> None:
        # дистрибутивные JDK бывают без файла release: раньше такой каталог
        # считался «просто bin», и в песочнице Java не находила conf/security
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory) / "jdk"
            (runtime / "bin").mkdir(parents=True)
            (runtime / "lib").mkdir()
            (runtime / "conf" / "security").mkdir(parents=True)
            (runtime / "conf" / "security" / "java.security").write_text("securerandom.source=Test\n", encoding="utf-8")
            java = runtime / "bin" / "java"
            java.write_bytes(b"java")

            root, mapped = _sandbox_java_path(str(java))

        self.assertEqual(root, runtime)
        self.assertEqual(mapped, "/trial-java/bin/java")

    def test_modrinth_modpack_import_downloads_index_files_and_overrides(self) -> None:
        class FakeHttp:
            def download(self, _url, destination, **_kwargs):
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(b"downloaded mod")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "pack.mrpack"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr(
                    "modrinth.index.json",
                    json.dumps(
                        {
                            "formatVersion": 1,
                            "files": [
                                {
                                    "path": "mods/example.jar",
                                    "downloads": ["https://cdn.example.invalid/example.jar"],
                                    "hashes": {},
                                }
                            ],
                        }
                    ),
                )
                archive.writestr("overrides/config/example.cfg", "enabled=true")
            target = root / "instance"
            target.mkdir()

            result = ModpackImporter(FakeHttp()).import_file(
                archive_path,
                target,
                source="modrinth",
                project_id="example-pack",
                display_name="Example Pack",
                version="1.0.0",
                confirm_unverified=True,
            )

            self.assertEqual(result["files"], ["config/example.cfg", "mods/example.jar"])
            self.assertEqual((target / "mods" / "example.jar").read_bytes(), b"downloaded mod")
            self.assertEqual((target / "config" / "example.cfg").read_text(encoding="utf-8"), "enabled=true")
            self.assertIn("example-pack", (target / ".trial-launcher-modpacks.json").read_text(encoding="utf-8"))

    def test_modpack_import_rejects_archive_traversal(self) -> None:
        class FakeHttp:
            def download(self, *_args, **_kwargs):
                raise AssertionError("archive traversal should fail before downloads")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "unsafe.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("../escape.txt", "blocked")
            target = root / "instance"
            target.mkdir()

            with self.assertRaises(Exception):
                ModpackImporter(FakeHttp()).import_file(
                    archive_path,
                    target,
                    source="local",
                    project_id="",
                    display_name="Unsafe",
                    version="",
                )
            self.assertFalse((root / "escape.txt").exists())

    def test_modpack_provider_uses_modrinth_modpack_facet(self) -> None:
        class FakeHttp:
            def __init__(self):
                self.url = ""

            def get_json(self, url):
                self.url = url
                return {"hits": [{"project_id": "pack", "title": "Pack", "downloads": 4}]}

        http = FakeHttp()
        result = ModpackProvider(http, {"kind": "modrinth", "api_url": "https://api.modrinth.com/v2"}).search("pack", "1.21.1", 5)

        self.assertEqual(result[0].project_id, "pack")
        self.assertIn("project_type%3Amodpack", http.url)

    def test_ftb_manifest_install_downloads_client_files_and_skips_server_only(self) -> None:
        payload = b"client mod"
        digest = hashlib.sha1(payload).hexdigest()

        class FakeHttp:
            def __init__(self):
                self.downloads = []

            def get_json(self, url):
                if url.endswith("/modpack/pack"):
                    return {
                        "id": "pack",
                        "name": "Example FTB Pack",
                        "versions": [{"id": "100504", "game_versions": ["1.20.1"]}],
                    }
                return {
                    "id": "pack",
                    "name": "Example FTB Pack 1.0",
                    "files": [
                        {
                            "path": "mods",
                            "name": "example.jar",
                            "url": "https://cdn.example.invalid/example.jar",
                            "hashes": {"sha1": digest},
                            "size": len(payload),
                        },
                        {
                            "path": "config",
                            "name": "server-only.cfg",
                            "url": "https://cdn.example.invalid/server-only.cfg",
                            "serveronly": True,
                        },
                    ],
                }

            def download(self, url, destination, **kwargs):
                self.downloads.append((url, destination, kwargs))
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(payload)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "instance"
            target.mkdir()
            http = FakeHttp()

            result = ModpackProvider(http, {"kind": "ftb", "api_url": "https://api.feed-the-beast.com/v1/modpacks/public"}).install(
                "pack",
                target,
                "1.20.1",
                version="100504",
                display_name="Example FTB Pack",
            )

            self.assertEqual(result["files"], ["mods/example.jar"])
            self.assertEqual((target / "mods" / "example.jar").read_bytes(), payload)
            self.assertFalse((target / "config" / "server-only.cfg").exists())
            self.assertEqual(http.downloads[0][2]["sha1"], digest)
            self.assertIn("ftb", (target / ".trial-launcher-modpacks.json").read_text(encoding="utf-8"))

    def test_ftb_manifest_rejects_unsafe_paths_before_download(self) -> None:
        class FakeHttp:
            def download(self, *_args, **_kwargs):
                raise AssertionError("unsafe manifest path should fail before download")

        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "instance"
            target.mkdir()

            with self.assertRaises(Exception):
                ModpackImporter(FakeHttp()).import_manifest(
                    [{"path": "../escape", "name": "bad.jar", "url": "https://cdn.example.invalid/bad.jar"}],
                    target,
                    source="ftb",
                    project_id="pack",
                    display_name="Unsafe",
                    version="100504",
                )
            self.assertFalse((Path(directory) / "escape" / "bad.jar").exists())

    def test_launcher_profile_is_normalized_and_selected_for_launch(self) -> None:
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
                        "launcher_profiles": [
                            {
                                "id": "Competitive Mode",
                                "name": "Competitive",
                                "memory": "4GiB",
                                "graphics_mode": "mesa",
                                "account_id": "offline-debug",
                                "window_width": 1600,
                                "jvm_arguments": ["-XX:+UseG1GC"],
                                "game_arguments": ["--demo"],
                            }
                        ],
                        "active_launcher_profile_id": "competitive-mode",
                    }
                )

                selected = _effective_launch_settings(
                    InstanceWorkspace(config, InstanceStore(config)),
                    config,
                    None,
                    {"memory": "2GiB"},
                )

            self.assertEqual(selected["profile_id"], "competitive-mode")
            self.assertEqual(selected["memory"], "4GiB")
            self.assertEqual(selected["graphics_mode"], "mesa")
            self.assertEqual(selected["account_id"], "offline-debug")
            self.assertEqual(selected["window_width"], 1600)
            self.assertEqual(selected["jvm_arguments"], ["-XX:+UseG1GC"])

    def test_instance_auto_graphics_mode_inherits_global_setting(self) -> None:
        config = SimpleNamespace(
            settings=SimpleNamespace(
                memory="4GiB",
                java_path="",
                graphics_mode="nvidia",
                launcher_profiles=[],
            )
        )
        workspace = SimpleNamespace(
            settings=lambda _name: {"memory": "auto", "java_path": "", "graphics_mode": "auto"}
        )

        selected = _effective_launch_settings(workspace, config, "test-instance", {})

        self.assertEqual(selected["graphics_mode"], "nvidia")

    def test_client_mod_context_uses_client_version_and_loader(self) -> None:
        version, loader = _instance_mod_context(
            SimpleNamespace(
                name="fabric-client",
                kind="client",
                minecraft_version="1.20.1",
                server_core_version="",
                version_id="1.20.1",
                loader="fabric",
            )
        )
        self.assertEqual(version, "1.20.1")
        self.assertEqual(loader, "fabric")

    def test_plugin_search_routes_server_core_and_version_to_plugin_catalog(self) -> None:
        server = SimpleNamespace(
            path=Path("/servers/paper-server"),
            name="paper-server",
            kind="server",
            minecraft_version="",
            server_core_version="1.21.1",
            version_id="",
            loader="vanilla",
            server_core="paper",
        )
        instances = SimpleNamespace(get=lambda _name: server)
        config = SimpleNamespace(
            settings=SimpleNamespace(
                plugin_catalogs=[{"id": "hangar", "name": "Hangar", "kind": "hangar", "api_url": "https://hangar.papermc.io/api/v1", "enabled": True}],
                active_plugin_catalog_id="hangar",
            )
        )
        seen = {}

        class FakeResult:
            def as_dict(self):
                return {"project_id": "example-plugin", "title": "Example Plugin"}

        class FakePluginCatalog:
            def __init__(self, _http, catalog):
                seen["catalog"] = catalog

            def search(self, query, core, version, limit, *, sort_by="relevance"):
                seen["search"] = (query, core, version, limit, sort_by)
                return [FakeResult()]

            def installed(self, _path):
                return []

        with patch("nivora_launcher.gui_service._context", return_value=(config, object(), None, None, None, None, instances)), \
            patch("nivora_launcher.gui_service.InstanceWorkspace", return_value=SimpleNamespace()), \
            patch("nivora_launcher.gui_service.PluginStore"), \
            patch("nivora_launcher.gui_service.ServerInstanceStore"), \
            patch("nivora_launcher.gui_service.ServerFiles"), \
            patch("nivora_launcher.gui_service.ServerProcessManager"), \
            patch("nivora_launcher.gui_service.PluginCatalog", FakePluginCatalog):
            result = handle({"operation": "mods_search", "instance_name": "paper-server", "project_type": "plugin", "query": "example", "limit": 5, "sort_by": "newest"})

        self.assertEqual(seen["catalog"]["id"], "hangar")
        self.assertEqual(seen["search"], ("example", "paper", "1.21.1", 5, "newest"))
        self.assertEqual(result["results"][0]["project_type"], "plugin")
        self.assertEqual(result["server_core"], "paper")

    def test_plugin_search_without_server_uses_requested_context(self) -> None:
        config = SimpleNamespace(
            settings=SimpleNamespace(
                plugin_catalogs=[{"id": "modrinth-plugins", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2", "enabled": True}],
                active_plugin_catalog_id="modrinth-plugins",
            )
        )
        instances = SimpleNamespace(get=lambda _name: None)
        seen = {}

        class FakePluginCatalog:
            def __init__(self, _http, _catalog):
                pass

            def search(self, query, core, version, limit, *, sort_by="relevance"):
                seen["search"] = (query, core, version, limit, sort_by)
                return []

        with patch("nivora_launcher.gui_service._context", return_value=(config, object(), None, None, None, None, instances)), \
            patch("nivora_launcher.gui_service.InstanceWorkspace", return_value=SimpleNamespace()), \
            patch("nivora_launcher.gui_service.PluginStore"), \
            patch("nivora_launcher.gui_service.ServerInstanceStore"), \
            patch("nivora_launcher.gui_service.ServerFiles"), \
            patch("nivora_launcher.gui_service.ServerProcessManager"), \
            patch("nivora_launcher.gui_service.PluginCatalog", FakePluginCatalog):
            result = handle({"operation": "mods_search", "project_type": "plugin", "query": "example", "game_version": "1.21.1", "server_core": "paper"})

        self.assertEqual(seen["search"], ("example", "paper", "1.21.1", 12, "relevance"))
        self.assertEqual(result["instance_name"], "")

    def test_offline_account_is_saved_and_becomes_default(self) -> None:
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
                store = AccountStore(config)
                account = OfflineDebugAccount(config, store).create("Player")
                loaded = store.get(account_id="offline-debug")

                self.assertIsNotNone(loaded)
                self.assertEqual(loaded.username, "Player")
                self.assertTrue(loaded.is_default)
                self.assertEqual(json.loads(config.paths.accounts_file.read_text(encoding="utf-8"))[0]["kind"], "offline")

    def test_software_graphics_mode_selects_mesa_without_driver_overrides(self) -> None:
        launcher = object.__new__(GameLauncher)
        launcher.config = SimpleNamespace(settings=SimpleNamespace(graphics_mode="auto"))
        with patch(
            "nivora_launcher.launcher.safe_process_environment",
            return_value={
                "__GLX_VENDOR_LIBRARY_NAME": "nvidia",
                "MESA_LOADER_DRIVER_OVERRIDE": "llvmpipe",
                "__NV_PRIME_RENDER_OFFLOAD": "1",
                "__VK_LAYER_NV_optimus": "NVIDIA_only",
            },
        ):
            environment = launcher._launch_environment(
                {"security_profile": "compat", "graphics_mode": "software"},
                "1.21.1",
            )
        self.assertEqual(environment["__GLX_VENDOR_LIBRARY_NAME"], "mesa")
        self.assertNotIn("__NV_PRIME_RENDER_OFFLOAD", environment)
        self.assertNotIn("__VK_LAYER_NV_optimus", environment)
        self.assertEqual(environment["GALLIUM_DRIVER"], "llvmpipe")
        self.assertNotIn("MESA_LOADER_DRIVER_OVERRIDE", environment)
        self.assertEqual(environment["MESA_GL_VERSION_OVERRIDE"], "3.3")
        self.assertEqual(environment["MESA_GLSL_VERSION_OVERRIDE"], "330")

    def test_glx_bad_fbconfig_error_code_triggers_software_retry(self) -> None:
        self.assertTrue(_looks_like_glx_context_error(["Unknown error: GLXBadFBConfig (65543)"]))
        self.assertFalse(
            _looks_like_glx_context_error(
                [
                    "Could not find any graphics adapters!",
                    "Sodium workaround: NO_ERROR_CONTEXT_UNSUPPORTED",
                    "Failed to fetch user properties: Status: 401",
                ]
            )
        )

    def test_server_mod_context_uses_server_core_version(self) -> None:
        version, loader = _instance_mod_context(
            SimpleNamespace(
                name="paper-server",
                kind="server",
                minecraft_version="",
                server_core_version="1.21.1",
                version_id="",
                loader="vanilla",
            )
        )
        self.assertEqual(version, "1.21.1")
        self.assertIsNone(loader)

    def test_asset_listing_reads_mod_metadata_and_local_icon(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            mods = Path(directory) / "mods"
            mods.mkdir()
            archive_path = mods / "cool-mod-1.2.3.jar"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr(
                    "fabric.mod.json",
                    json.dumps({"id": "cool-mod", "name": "Cool Mod", "version": "1.2.3", "authors": ["Author"]}),
                )
                archive.writestr("icon.png", b"\x89PNG\r\n\x1a\nicon")

            assets = InstanceWorkspace(None, None)._list_assets_from_directory(mods, "mods")

            self.assertEqual(assets[0]["display_name"], "Cool Mod")
            self.assertEqual(assets[0]["version"], "1.2.3")
            self.assertEqual(assets[0]["author"], "Author")
            self.assertTrue(assets[0]["icon_data"].startswith("data:image/png;base64,"))

    def test_writable_instance_config_cannot_disable_security(self) -> None:
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
                instance = config.game_dir / "instances" / "escape-poc"
                instance.mkdir(parents=True)
                settings_path = instance / ".trial-launcher.json"
                settings_path.write_text(
                    json.dumps(
                        {
                            "memory": "1GiB",
                            "security_profile": "off",
                            "security_permissions": ["files:host-write"],
                        }
                    ),
                    encoding="utf-8",
                )

                workspace = InstanceWorkspace(config, InstanceStore(config))
                first = workspace.settings("escape-poc")
                self.assertEqual(first["security_profile"], "strict")
                self.assertNotIn("files:host-write", first["security_permissions"])
                self.assertNotIn("security_profile", json.loads(settings_path.read_text(encoding="utf-8")))

                settings_path.write_text(
                    json.dumps({"security_profile": "off", "security_permissions": ["files:host-write"]}),
                    encoding="utf-8",
                )
                second = workspace.settings("escape-poc")
                self.assertEqual(second["security_profile"], "strict")
                protected = load_protected_settings(
                    config,
                    "escape-poc",
                    {"security_profile": "strict", "security_permissions": ["network:game", "files:instance"]},
                )
                self.assertEqual(protected["security_profile"], "strict")

    def test_legacy_policy_makes_previously_implicit_desktop_access_explicit(self) -> None:
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
                instance = config.game_dir / "instances" / "legacy"
                instance.mkdir(parents=True)
                protected_path = config.paths.instance_security_dir / "legacy.json"
                protected_path.parent.mkdir(parents=True, exist_ok=True)
                protected_path.write_text(
                    json.dumps(
                        {
                            "format": 1,
                            "java_path": "",
                            "security_profile": "strict",
                            "security_permissions": ["network:game", "files:instance"],
                            "security_policy_version": 2,
                        }
                    ),
                    encoding="utf-8",
                )

                settings = InstanceWorkspace(config, InstanceStore(config)).settings("legacy")
                protected = json.loads(protected_path.read_text(encoding="utf-8"))

                self.assertIn("desktop:display", settings["security_permissions"])
                self.assertIn("desktop:audio", settings["security_permissions"])
                self.assertIn("network:game", settings["security_permissions"])
                self.assertEqual(protected["security_policy_version"], 4)

    def test_strict_loader_command_is_sandboxed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            fake_bwrap = instance / "bwrap"
            fake_bwrap.write_text("", encoding="utf-8")
            with patch("nivora_launcher.security.shutil.which", return_value=str(fake_bwrap)):
                command, cwd, environment, warning = prepare_process(
                    [
                        sys.executable,
                        f"-Djava.library.path={instance / 'natives'}",
                        "-cp",
                        f"{instance / 'libraries' / 'a.jar'}:{instance / 'client.jar'}",
                        str(instance / "installer.jar"),
                        "--target",
                        str(instance),
                    ],
                    instance,
                    {"PATH": "/usr/bin"},
                    {"security_profile": "strict", "security_permissions": ["network:game", "files:instance"]},
                )
            self.assertEqual(cwd, instance)
            self.assertIsNone(warning)
            self.assertEqual(command[0], str(fake_bwrap))
            self.assertIn("--bind", command)
            self.assertIn("/instance", command)
            self.assertIn("/trial-java", command)
            self.assertIn(["--tmpfs", "/etc"], [command[index : index + 2] for index in range(len(command) - 1)])
            self.assertNotIn(
                ["--ro-bind", "/etc", "/etc"],
                [command[index : index + 3] for index in range(len(command) - 2)],
            )
            self.assertIn("/etc/hosts", command)
            self.assertNotIn("/etc/shadow", command)
            self.assertIn("-Djava.library.path=/instance/natives", command)
            self.assertIn("/instance/libraries/a.jar:/instance/client.jar", command)
            self.assertEqual(environment["HOME"], "/instance/.trial-cell/home")

    def test_strict_sandbox_hides_desktop_and_audio_channels_without_grants(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            fake_bwrap = instance / "bwrap"
            fake_bwrap.write_text("", encoding="utf-8")
            environment = {
                "PATH": "/usr/bin",
                "DISPLAY": ":0",
                "WAYLAND_DISPLAY": "wayland-1",
                "XDG_RUNTIME_DIR": "/run/user/1000",
                "PULSE_SERVER": "unix:/run/user/1000/pulse/native",
                "PIPEWIRE_REMOTE": "pipewire-0",
                "PIPEWIRE_LATENCY": "128/48000",
            }
            with patch("nivora_launcher.security.shutil.which", return_value=str(fake_bwrap)):
                command, _cwd, prepared_environment, warning = prepare_process(
                    [sys.executable],
                    instance,
                    environment,
                    {"security_profile": "strict", "security_permissions": ["files:instance"]},
                )

            self.assertNotIn("DISPLAY", prepared_environment)
            self.assertNotIn("WAYLAND_DISPLAY", prepared_environment)
            self.assertNotIn("XAUTHORITY", prepared_environment)
            self.assertNotIn("PULSE_SERVER", prepared_environment)
            self.assertNotIn("PIPEWIRE_REMOTE", prepared_environment)
            self.assertNotIn("PIPEWIRE_LATENCY", prepared_environment)
            self.assertIn("desktop:display", warning or "")
            self.assertNotIn("/dev/snd", command)

    def test_strict_sandbox_binds_only_requested_desktop_sockets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            fake_bwrap = instance / "bwrap"
            fake_bwrap.write_text("", encoding="utf-8")
            environment = {
                "PATH": "/usr/bin",
                "WAYLAND_DISPLAY": "wayland-1",
                "XDG_RUNTIME_DIR": "/run/user/1000",
            }
            with (
                patch("nivora_launcher.security.shutil.which", return_value=str(fake_bwrap)),
                patch("nivora_launcher.security._append_runtime_socket") as bind_socket,
            ):
                _command, _cwd, prepared_environment, _warning = prepare_process(
                    [sys.executable],
                    instance,
                    environment,
                    {
                        "security_profile": "strict",
                        "security_permissions": ["files:instance", "desktop:display", "desktop:audio"],
                    },
                )

            self.assertEqual(prepared_environment["WAYLAND_DISPLAY"], "wayland-1")
            self.assertEqual(
                [call.args[1] for call in bind_socket.call_args_list],
                [
                    Path("/run/user/1000/wayland-1"),
                    Path("/run/user/1000/pipewire-0"),
                    Path("/run/user/1000/pulse/native"),
                ],
            )

    def test_strict_graphics_mounts_sysfs_read_only_but_hides_it_for_software(self) -> None:
        for graphics_mode, expected in (
            ("nvidia", ("--ro-bind", "/sys", "/sys")),
            ("mesa", ("--ro-bind", "/sys", "/sys")),
            ("auto", ("--ro-bind", "/sys", "/sys")),
            ("software", ("--tmpfs", "/sys")),
            ("", ("--tmpfs", "/sys")),
        ):
            with self.subTest(graphics_mode=graphics_mode), tempfile.TemporaryDirectory() as directory:
                instance = Path(directory) / "instance"
                instance.mkdir()
                fake_bwrap = instance / "bwrap"
                fake_bwrap.write_text("", encoding="utf-8")
                environment = {"PATH": "/usr/bin"}
                if graphics_mode:
                    environment["TRIAL_LAUNCHER_GRAPHICS_MODE"] = graphics_mode
                with patch("nivora_launcher.security.shutil.which", return_value=str(fake_bwrap)):
                    command, _cwd, _environment, warning = prepare_process(
                        [sys.executable],
                        instance,
                        environment,
                        {
                            "security_profile": "strict",
                            "security_permissions": ["network:game", "files:instance", "desktop:display"],
                        },
                    )
                self.assertIsNone(warning)
                self.assertIn(list(expected), [command[index : index + len(expected)] for index in range(len(command))])

    def test_strict_mounts_distro_java_config_from_etc_only(self) -> None:
        from nivora_launcher import security as security_module

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            java_home = root / "jvm"
            (java_home / "lib" / "security").mkdir(parents=True)
            etc_java = root / "etc-java"
            (etc_java / "security").mkdir(parents=True)
            (etc_java / "security" / "java.security").write_text("x", encoding="utf-8")
            (java_home / "conf").symlink_to("/etc/java-17-openjdk")
            (java_home / "lib" / "security" / "cacerts").symlink_to("/etc/ssl/certs/java/cacerts")
            (java_home / "lib" / "evil").symlink_to("/etc/shadow")

            targets = security_module._java_system_config_targets(java_home)
            self.assertIn(Path("/etc/java-17-openjdk"), targets)
            self.assertIn(Path("/etc/ssl/certs/java/cacerts"), targets)
            self.assertNotIn(Path("/etc/shadow"), targets)

    def test_new_instance_defaults_allow_game_network(self) -> None:
        from nivora_launcher.instance_workspace import INSTANCE_SECURITY_DEFAULTS
        from nivora_launcher.security import normalize_security_permissions

        self.assertIn("network:game", INSTANCE_SECURITY_DEFAULTS["security_permissions"])
        self.assertIn("network:game", normalize_security_permissions(None))

    def test_loader_installer_redirect_stays_on_official_maven(self) -> None:
        _validate_loader_installer_url("https://maven.minecraftforge.net/net/minecraftforge/forge/a.jar")
        with self.assertRaises(Exception):
            _validate_loader_installer_url("https://downloads.example.invalid/a.jar")

    def test_instance_export_excludes_runtime_secrets(self) -> None:
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
                instance = config.game_dir / "instances" / "export-poc"
                (instance / ".trial-cell" / "home").mkdir(parents=True)
                (instance / ".trial-cell" / "xauthority").write_text("secret", encoding="utf-8")
                (instance / ".trial-launcher.json").write_text("{}", encoding="utf-8")
                (instance / ".trial-launcher-live.json").write_text("{}", encoding="utf-8")
                (instance / "keep.txt").write_text("game data", encoding="utf-8")
                archive_path = root / "export.zip"
                InstanceStore(config).export_zip("export-poc", archive_path)
                with zipfile.ZipFile(archive_path) as archive:
                    names = set(archive.namelist())
                self.assertIn("export-poc/keep.txt", names)
                self.assertFalse(any(".trial-cell" in name for name in names))
                self.assertFalse(any(name.endswith(".trial-launcher.json") for name in names))
                self.assertFalse(any(name.endswith(".trial-launcher-live.json") for name in names))

    def test_microsoft_tokens_use_secret_service_only(self) -> None:
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
                account = Account(
                    account_id="microsoft",
                    kind="microsoft",
                    username="Player",
                    uuid="uuid",
                    access_token="access-secret",
                    refresh_token="refresh-secret",
                )
                stored = []

                def run_secret(command, **kwargs):
                    if command[1] == "clear":
                        return type("Result", (), {"returncode": 1, "stdout": "", "stderr": ""})()
                    if command[1] == "store":
                        stored.append((command, kwargs["input"]))
                        return type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})()
                    if command[1] == "lookup":
                        return type(
                            "Result",
                            (),
                            {
                                "returncode": 0,
                                "stdout": '{"access_token":"access-secret","refresh_token":"refresh-secret"}',
                                "stderr": "",
                            },
                        )()
                    raise AssertionError(command)

                with patch("nivora_launcher.accounts.shutil.which", return_value="/usr/bin/secret-tool"), patch(
                    "nivora_launcher.accounts.subprocess.run", side_effect=run_secret
                ):
                    AccountStore(config).save(account)
                    loaded = AccountStore(config).get(account_id="microsoft")

                self.assertEqual(loaded.access_token, "access-secret")
                self.assertEqual(loaded.refresh_token, "refresh-secret")
                self.assertEqual(len(stored), 1)
                self.assertEqual(stored[0][0][4:], ["service", "trial-launcher-microsoft", "account-id", "microsoft"])
                self.assertEqual(json.loads(stored[0][1]), {"access_token": "access-secret", "refresh_token": "refresh-secret", "client_token": ""})
                on_disk = json.loads(config.paths.accounts_file.read_text(encoding="utf-8"))
                self.assertNotIn("access_token", on_disk[0])
                self.assertNotIn("refresh_token", on_disk[0])

    def test_microsoft_access_token_is_not_in_java_argv(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            token = "access-secret"
            command = ["/usr/bin/java", "--accessToken", _AUTH_TOKEN_PLACEHOLDER, "net.minecraft.client.Main"]
            materialized = _materialize_auth_token(command, token)
            self.assertIn(token, materialized)
            argument_file = _create_java_argument_file(materialized[1:], root)
            try:
                launch_command = [materialized[0], argument_file.argument]
                self.assertNotIn(token, launch_command)
                self.assertEqual(argument_file.pass_fds, (argument_file.fd,) if argument_file.fd is not None else ())
                if argument_file.path:
                    self.assertEqual(argument_file.path.stat().st_mode & 0o777, 0o600)
                    self.assertIn(token, argument_file.path.read_text(encoding="utf-8"))
                elif argument_file.fd is not None:
                    os.lseek(argument_file.fd, 0, os.SEEK_SET)
                    self.assertIn(token, os.read(argument_file.fd, 4096).decode("utf-8"))
            finally:
                path = argument_file.path
                argument_file.close()
                if path:
                    self.assertFalse(path.exists())

    def test_build_command_allows_software_fallback_without_leaking_microsoft_token(self) -> None:
        launcher = object.__new__(GameLauncher)
        launcher.config = SimpleNamespace(settings=SimpleNamespace(launcher_version="test", client_id="client", memory=""))
        launcher.runtime = SimpleNamespace(java_for=lambda _version_json, _java_override=None: Path("/usr/bin/java"))
        install = SimpleNamespace(
            version_id="1.21.1",
            instance_dir=Path("/tmp/trial-instance"),
            version_json={
                "type": "release",
                "mainClass": "net.minecraft.client.Main",
                "arguments": {"game": [{"value": ["--accessToken", "${auth_access_token}"]}]},
            },
            client_jar=Path("/tmp/client.jar"),
            libraries=[],
            natives_dir=Path("/tmp/natives"),
            assets_root=Path("/tmp/assets"),
            asset_index_name="1.21",
            logging_file=None,
        )
        account = Account("microsoft", "microsoft", "Player", "uuid", "access-secret")

        command = launcher.build_command(install, account, {"graphics_mode": "nvidia"})

        self.assertNotIn(account.access_token, command)
        self.assertIn(_AUTH_TOKEN_PLACEHOLDER, command)
        self.assertIn("-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true", command)

    def test_strict_process_maps_java_argument_file_inside_sandbox(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance = root / "instance"
            instance.mkdir()
            fake_bwrap = instance / "bwrap"
            fake_bwrap.write_text("", encoding="utf-8")
            argument_file = _create_java_argument_file(["--accessToken", "access-secret"], instance)
            try:
                with patch("nivora_launcher.security.shutil.which", return_value=str(fake_bwrap)):
                    command, _cwd, _environment, warning = prepare_process(
                        ["/usr/bin/java", argument_file.argument],
                        instance,
                        {"PATH": "/usr/bin"},
                        {"security_profile": "strict", "security_permissions": ["files:instance"]},
                        argument_file_fd=argument_file.fd,
                        argument_file_target="/instance/.trial-cell/tmp/.trial-launcher-java-args",
                    )
                self.assertIn("network:game", warning or "")
                self.assertNotIn("access-secret", command)
                self.assertIn("--ro-bind-data", command)
                self.assertIn("/instance/.trial-cell/tmp/.trial-launcher-java-args", command)
                self.assertIn("@/instance/.trial-cell/tmp/.trial-launcher-java-args", command)
            finally:
                argument_file.close()

    def test_server_file_api_rejects_symlink_parent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "server"
            outside = Path(directory) / "outside"
            root.mkdir()
            outside.mkdir()
            files = ServerFiles(SimpleNamespace(get=lambda _name: SimpleNamespace(path=root)))
            (root / "escape").symlink_to(outside, target_is_directory=True)

            with self.assertRaises(Exception):
                files.write_text("server", "escape/pwned.txt", "blocked")
            with self.assertRaises(Exception):
                files.upload("server", "escape/pwned.jar", b"blocked")
            self.assertFalse((outside / "pwned.txt").exists())
            self.assertFalse((outside / "pwned.jar").exists())

    def test_snapshot_manifest_cannot_reference_external_archive(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "instance"
            (root / "plugins").mkdir(parents=True)
            snapshot_dir = root / ".trial-launcher-snapshots"
            snapshot_dir.mkdir()
            snapshot_id = "1700000000-deadbeef"
            (snapshot_dir / f"plugins-{snapshot_id}.json").write_text(
                json.dumps({"id": snapshot_id, "category": "plugins", "archive": "../outside.zip"}),
                encoding="utf-8",
            )
            snapshots = SnapshotStore(root)
            self.assertEqual(snapshots.list("plugins"), [])
            with self.assertRaises(Exception):
                snapshots.restore("plugins", snapshot_id)

    def test_snapshot_listing_skips_dangling_symlinks_pipes_and_oversized_manifests(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "instance"
            snapshot_dir = root / ".trial-launcher-snapshots"
            snapshot_dir.mkdir(parents=True)
            (snapshot_dir / "plugins-1700000000-deadbeef.json").symlink_to(snapshot_dir / "missing.json")
            (snapshot_dir / "plugins-1700000001-cafebabe.json").write_bytes(
                b" " * (MAX_SNAPSHOT_MANIFEST_BYTES + 1)
            )
            if hasattr(os, "mkfifo"):
                os.mkfifo(snapshot_dir / "plugins-1700000002-feedface.json")

            snapshots = SnapshotStore(root)

            self.assertEqual(snapshots.list("plugins"), [])

    def test_bedrock_snapshot_categories_use_com_mojang_layout(self) -> None:
        # Bedrock снапшотирует resource_packs/behavior_packs/minecraftWorlds
        # вместо Java-каталогов: свои категории, тот же механизм.
        categories = {
            "resourcepacks": "resource_packs",
            "addons": "behavior_packs",
            "worlds": "minecraftWorlds",
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "com.mojang"
            (root / "resource_packs" / "Vanilla").mkdir(parents=True)
            (root / "resource_packs" / "Vanilla" / "manifest.json").write_text("{}", encoding="utf-8")
            store = SnapshotStore(root, categories=categories)

            snapshot = store.create("resourcepacks", retention_hours=1, max_count=1)

            self.assertTrue(snapshot["id"])
            listed = store.list("resourcepacks")
            self.assertEqual(len(listed), 1)
            self.assertEqual(listed[0]["id"], snapshot["id"])
            # Java-категории в Bedrock-корне недоступны
            with self.assertRaisesRegex(LauncherError, "Неизвестная категория"):
                store.create("mods")
            restored = store.restore("resourcepacks", snapshot["id"])
            self.assertTrue(restored)
    def test_nbt_gzip_expansion_is_bounded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "level.dat"
            with patch("nivora_launcher.instance_workspace.MAX_NBT_DECOMPRESSED_BYTES", 32):
                path.write_bytes(gzip.compress(b"x" * 33))
                with self.assertRaisesRegex(LauncherError, "Распакованный NBT-файл слишком большой"):
                    _read_nbt(path)

    def test_snapshot_restore_rejects_archive_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "instance"
            (root / "plugins").mkdir(parents=True)
            snapshots = SnapshotStore(root)
            snapshot_id = "1700000000-cafebabe"
            (root / ".trial-launcher-snapshots").mkdir()
            archive_path = root / ".trial-launcher-snapshots" / f"plugins-{snapshot_id}.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("../escape.txt", "blocked")
            (archive_path.parent / f"plugins-{snapshot_id}.json").write_text(
                json.dumps(
                    {
                        "id": snapshot_id,
                        "category": "plugins",
                        "archive": archive_path.name,
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaises(Exception):
                snapshots.restore("plugins", snapshot_id)
            self.assertFalse((root / "escape.txt").exists())

    def test_native_archive_cannot_follow_symlink_parent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "native.jar"
            destination = root / "native"
            outside = root / "outside"
            destination.mkdir()
            outside.mkdir()
            (destination / "escape").symlink_to(outside, target_is_directory=True)
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("escape/pwned.so", b"blocked")

            with self.assertRaises(Exception):
                _safe_extract(archive_path, destination, set())
            self.assertFalse((outside / "pwned.so").exists())


if __name__ == "__main__":
    unittest.main()
