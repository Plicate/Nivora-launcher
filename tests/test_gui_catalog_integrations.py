from __future__ import annotations

import base64
import unittest
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import handle
from nivora_launcher.lan_sessions import LanSessionError


class GuiCatalogIntegrationTests(unittest.TestCase):
    def _handle(self, request: dict, *, plugin_store=None, lan_manager=None, plugin_catalog=None, config=None, instances=None, server_store=None, server_files=None, emit=None):
        config = config or SimpleNamespace(settings=SimpleNamespace())
        instances = instances if instances is not None else object()
        patches = [
            patch("nivora_launcher.gui_service._context", return_value=(config, object(), None, None, None, None, instances)),
            patch("nivora_launcher.gui_service.InstanceWorkspace", return_value=SimpleNamespace()),
            patch("nivora_launcher.gui_service.PluginStore", return_value=plugin_store or SimpleNamespace(list=lambda: [])),
            patch("nivora_launcher.gui_service.ServerInstanceStore", return_value=server_store or SimpleNamespace()),
            patch("nivora_launcher.gui_service.ServerFiles", return_value=server_files or SimpleNamespace()),
            patch("nivora_launcher.gui_service.ServerProcessManager", return_value=SimpleNamespace()),
        ]
        if lan_manager is not None:
            patches.append(patch("nivora_launcher.gui_service._LAN_SESSION_MANAGER", lan_manager))
        if plugin_catalog is not None:
            patches.append(patch("nivora_launcher.gui_service.PluginCatalog", plugin_catalog))
        with ExitStack() as stack:
            for patcher in patches:
                stack.enter_context(patcher)
            return handle(request, emit=emit)

    def test_lan_session_routes_keep_manager_state_and_translate_errors(self) -> None:
        status = {"room_name": "Friends", "host_control_port": 43120, "local_role": "host"}
        manager = Mock()
        manager.create.return_value = status
        manager.status.return_value = status
        manager.leave.return_value = {"left": True, "room_name": "Friends"}
        payload = {
            "room_name": "Friends",
            "password": "long-enough-password",
            "player_nickname": "Host",
            "host_control_port": 0,
            "minecraft_game_port": 25565,
        }

        self.assertEqual(self._handle({"operation": "lan_session_create", **payload}, lan_manager=manager), status)
        self.assertEqual(self._handle({"operation": "lan_session_status", **payload}, lan_manager=manager), status)
        self.assertEqual(self._handle({"operation": "lan_session_leave", **payload}, lan_manager=manager)["left"], True)
        manager.create.assert_called_once_with("Friends", "long-enough-password", "Host", 0, 25565)
        manager.status.assert_called_once_with("Friends", "long-enough-password", "Host", 0, 25565, host_address=None)

        manager.join.side_effect = LanSessionError("room is unavailable")
        with self.assertRaisesRegex(LauncherError, "room is unavailable"):
            self._handle({"operation": "lan_session_join", **payload, "host_address": "192.168.1.20"}, lan_manager=manager)

    def test_server_plugin_details_include_empty_compatible_versions_list(self) -> None:
        config = SimpleNamespace(
            settings=SimpleNamespace(
                plugin_catalogs=[{"id": "hangar", "name": "Hangar", "kind": "hangar", "api_url": "https://hangar.papermc.io/api/v1", "enabled": True}],
                active_plugin_catalog_id="hangar",
            )
        )
        instances = SimpleNamespace(get=lambda _name: None)

        class FakePluginCatalog:
            def __init__(self, _http, _catalog):
                pass

            def details(self, project_id, core, version):
                return {"project_id": project_id, "server_core": core, "game_version": version}

        result = self._handle(
            {"operation": "mods_details", "project_id": "sample-plugin", "project_type": "plugin", "game_version": "1.21.1", "server_core": "paper"},
            config=config,
            instances=instances,
            plugin_catalog=FakePluginCatalog,
        )

        self.assertEqual(result["details"]["project_id"], "sample-plugin")
        self.assertEqual(result["compatible_versions"], [])

    def test_plugin_catalog_search_marks_managed_server_plugins_as_installed(self) -> None:
        config = SimpleNamespace(
            settings=SimpleNamespace(
                plugin_catalogs=[{"id": "test", "name": "Test", "kind": "custom", "api_url": "https://catalog.example/plugins"}],
                active_plugin_catalog_id="test",
            )
        )
        instance = SimpleNamespace(
            name="server",
            kind="server",
            path=Path("/servers/server"),
            server_core="paper",
            server_core_version="1.21.1",
        )
        instances = SimpleNamespace(get=lambda _name: instance)
        expected_plugins_path = instance.path / "plugins"
        installed_paths = []

        class FakePluginCatalog:
            def __init__(self, _http, _catalog):
                pass

            def search(self, *_args, **_kwargs):
                return [SimpleNamespace(as_dict=lambda: {"project_id": "sample", "title": "Sample", "file_name": "sample.jar"})]

            def installed(self, path):
                installed_paths.append(path)
                return [{"project_id": "sample", "file_name": "sample-v2.jar"}]

        result = self._handle(
            {"operation": "mods_search", "instance_name": "server", "project_type": "plugin", "catalog_id": "test"},
            config=config,
            instances=instances,
            plugin_catalog=FakePluginCatalog,
        )

        self.assertEqual(result["results"][0]["installed"], True)
        self.assertEqual(result["results"][0]["file_name"], "sample-v2.jar")
        self.assertEqual(installed_paths, [expected_plugins_path])

    def test_server_plugin_search_marks_managed_plugins_and_uses_installed_file_name(self) -> None:
        info = SimpleNamespace(name="server", path=Path("/servers/server"))
        installed_paths = []
        server_store = SimpleNamespace(
            get=lambda _name: info,
            config=lambda _name: {"core": "paper", "version": "1.21.1"},
        )
        config = SimpleNamespace(settings=SimpleNamespace(plugin_catalogs=[{"id": "test", "name": "Test", "kind": "custom", "api_url": "https://catalog.example/plugins"}], active_plugin_catalog_id="test"))

        class FakePluginCatalog:
            def __init__(self, _http, _catalog):
                pass

            def search(self, *_args, **_kwargs):
                return [SimpleNamespace(as_dict=lambda: {"project_id": "sample", "title": "Sample", "file_name": "sample.jar"})]

            def installed(self, path):
                installed_paths.append(path)
                return [{"project_id": "sample", "file_name": "sample-v2.jar"}]

        result = self._handle(
            {"operation": "server_plugin_search", "name": "server", "catalog_id": "test"},
            config=config,
            server_store=server_store,
            plugin_catalog=FakePluginCatalog,
        )

        self.assertTrue(result["results"][0]["installed"])
        self.assertEqual(result["results"][0]["file_name"], "sample-v2.jar")
        self.assertEqual(installed_paths, [info.path / "plugins"])

    def test_server_plugin_remove_uses_managed_metadata_and_refreshes_files(self) -> None:
        info = SimpleNamespace(name="server", path=Path("/servers/server"))
        server_store = SimpleNamespace(get=lambda _name: info)
        server_files = Mock()
        server_files.list.return_value = []

        with (
            patch("nivora_launcher.gui_service.InstanceProcessLock") as process_lock,
            patch("nivora_launcher.gui_service._server_is_running", return_value=False),
            patch("nivora_launcher.gui_service.remove_installed_plugin", return_value=True) as remove_managed,
        ):
            result = self._handle(
                {"operation": "server_plugin_remove", "name": "server", "file_name": "sample.jar"},
                config=SimpleNamespace(paths=SimpleNamespace(data_home=Path("/data")), settings=SimpleNamespace()),
                server_store=server_store,
                server_files=server_files,
            )

        remove_managed.assert_called_once_with(info.path / "plugins", "sample.jar")
        server_files.delete.assert_not_called()
        server_files.list.assert_called_once_with("server", "plugins")
        process_lock.return_value.acquire.assert_called_once()
        process_lock.return_value.release.assert_called_once()
        self.assertTrue(result["removed"])

    def test_resource_remove_refuses_while_instance_is_running(self) -> None:
        info = SimpleNamespace(name="alpha", running=True)
        instances = SimpleNamespace(get=lambda _name: info)

        with patch("nivora_launcher.gui_service.InstanceProcessLock") as process_lock:
            with self.assertRaisesRegex(LauncherError, "Остановите экземпляр"):
                self._handle(
                    {"operation": "mods_remove", "instance_name": "alpha", "project_id": "sample"},
                    instances=instances,
                )

        process_lock.assert_not_called()

    def test_resource_updates_check_all_configured_modrinth_catalogs(self) -> None:
        catalogs = [
            {"id": "modrinth", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2"},
            {"id": "team-mods", "name": "Team Mods", "kind": "modrinth", "api_url": "https://mods.example/api/v2"},
        ]
        config = SimpleNamespace(settings=SimpleNamespace(resource_catalogs=catalogs, active_catalog_id="modrinth"))
        instance = SimpleNamespace(name="alpha", kind="client", minecraft_version="1.21.1", loader="fabric", path="instance")
        instances = SimpleNamespace(get=lambda _name: instance)
        checked_catalogs: list[str] = []

        class FakeModrinthProvider:
            def __init__(self, _http, _base_url, *, catalog_id):
                self.catalog_id = catalog_id

            def check_updates(self, *_args, **_kwargs):
                checked_catalogs.append(self.catalog_id)
                if self.catalog_id == "team-mods":
                    return [{"project_id": "team-project", "catalog_id": self.catalog_id}]
                return []

        with patch("nivora_launcher.gui_service.ModrinthProvider", FakeModrinthProvider):
            result = self._handle(
                {"operation": "mods_check_updates", "instance_name": "alpha", "project_type": "mod"},
                config=config,
                instances=instances,
            )

        self.assertEqual(checked_catalogs, ["modrinth", "team-mods"])
        self.assertEqual(result["updates"][0]["project_id"], "team-project")

    def test_resource_update_check_keeps_results_when_a_catalog_is_unavailable(self) -> None:
        catalogs = [
            {"id": "first", "name": "First", "kind": "modrinth", "api_url": "https://first.example/api/v2"},
            {"id": "second", "name": "Second", "kind": "modrinth", "api_url": "https://second.example/api/v2"},
        ]
        config = SimpleNamespace(settings=SimpleNamespace(resource_catalogs=catalogs, active_catalog_id="first"))
        instance = SimpleNamespace(name="alpha", kind="client", minecraft_version="1.21.1", loader="fabric", path="instance")
        instances = SimpleNamespace(get=lambda _name: instance)

        class FakeModrinthProvider:
            def __init__(self, _http, _base_url, *, catalog_id):
                self.catalog_id = catalog_id

            def check_updates(self, *_args, **_kwargs):
                if self.catalog_id == "first":
                    raise LauncherError("catalog unavailable")
                return [{"project_id": "available-project", "catalog_id": self.catalog_id}]

        with patch("nivora_launcher.gui_service.ModrinthProvider", FakeModrinthProvider):
            result = self._handle(
                {"operation": "mods_check_updates", "instance_name": "alpha", "project_type": "mod"},
                config=config,
                instances=instances,
            )

        self.assertEqual([item["project_id"] for item in result["updates"]], ["available-project"])
        self.assertEqual(result["errors"], ["First: catalog unavailable"])

    def test_github_install_uses_verified_bytes_and_emits_plugin_refresh(self) -> None:
        plugin = {"id": "sample-plugin", "name": "Sample Plugin", "installed": True, "enabled": False}
        store = Mock()
        store.install_archive.return_value = plugin
        store.list.return_value = [plugin]
        service = Mock()
        metadata = {"full_name": "octocat/sample-plugin", "version": "1.2.3", "size": 96, "sha256": "d" * 64}
        service.download_plugin_archive.return_value = (metadata, b"verified archive")
        events = []

        with patch("nivora_launcher.gui_service.GitHubPluginService", return_value=service) as service_class, patch(
            "nivora_launcher.gui_service.SecretToolTokenProvider", return_value=object()
        ):
            result = self._handle(
                {"operation": "github_plugin_install", "owner": "octocat", "repo": "sample-plugin", "replace": True},
                plugin_store=store,
                emit=events.append,
            )

        service_class.assert_called_once()
        service.download_plugin_archive.assert_called_once_with("octocat", "sample-plugin")
        store.install_archive.assert_called_once_with(b"verified archive", replace=True)
        self.assertEqual(result["metadata"], {"full_name": "octocat/sample-plugin", "version": "1.2.3", "size": 96, "sha256": "d" * 64})
        self.assertEqual(events[0]["event"], "plugins-changed")

    def test_github_search_and_publication_dispatch_to_service(self) -> None:
        service = Mock()
        service.search.return_value = [{"full_name": "octocat/sample-plugin"}]
        service.publish_plugin.return_value = {"full_name": "octocat/published", "sha256": "e" * 64}
        encoded = base64.b64encode(b"plugin zip").decode("ascii")

        with patch("nivora_launcher.gui_service.GitHubPluginService", return_value=service), patch(
            "nivora_launcher.gui_service.SecretToolTokenProvider", return_value=object()
        ):
            search = self._handle({"operation": "github_plugin_search", "query": "sample", "page": 2})
            published = self._handle({"operation": "github_plugin_publish", "data": encoded, "description": "Community plugin"})

        service.search.assert_called_once_with("sample", page=2)
        service.publish_plugin.assert_called_once_with(b"plugin zip", description="Community plugin")
        self.assertEqual(search["page"], 2)
        self.assertEqual(published["full_name"], "octocat/published")

    def test_modpack_search_ignores_bedrock_instance_and_uses_explicit_version(self) -> None:
        config = SimpleNamespace(
            settings=SimpleNamespace(
                resource_catalogs=[{"id": "modrinth", "name": "Modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2", "enabled": True}]
            )
        )
        bedrock = SimpleNamespace(
            name="Bedrock",
            kind="client",
            game_edition="bedrock",
            loader="vanilla",
            minecraft_version="",
            version_id="",
            path=Path("/bedrock"),
        )
        instances = SimpleNamespace(get=lambda _name: bedrock)
        searches = []

        class FakeModpackProvider:
            def __init__(self, _http, _catalog):
                pass

            def search(self, query, game_version, limit):
                searches.append((query, game_version, limit))
                return [SimpleNamespace(as_dict=lambda: {"project_id": "pack", "title": "Sample Pack"})]

        with patch("nivora_launcher.gui_service.ModpackProvider", FakeModpackProvider):
            result = self._handle(
                {"operation": "mods_search", "instance_name": "Bedrock", "project_type": "modpack", "game_version": "1.20.4", "catalog_id": "modrinth"},
                config=config,
                instances=instances,
            )

        self.assertEqual(searches, [("", "1.20.4", 12)])
        self.assertEqual(result["results"][0]["title"], "Sample Pack")

    def test_java_content_targets_reject_bedrock_instances_with_clear_error(self) -> None:
        bedrock = SimpleNamespace(
            name="Bedrock",
            kind="client",
            game_edition="bedrock",
            loader="vanilla",
            minecraft_version="",
            version_id="",
            path=Path("/bedrock"),
        )
        instances = SimpleNamespace(get=lambda _name: bedrock)
        with self.assertRaisesRegex(LauncherError, "только для Java-экземпляров"):
            self._handle(
                {"operation": "mods_check_updates", "instance_name": "Bedrock", "project_type": "mod", "catalog_id": "modrinth"},
                config=SimpleNamespace(settings=SimpleNamespace(resource_catalogs=[{"id": "modrinth", "kind": "modrinth", "api_url": "https://api.modrinth.com/v2"}])),
                instances=instances,
            )

    def test_system_notify_uses_notify_send_and_tolerates_missing_notifier(self) -> None:
        calls = []

        def fake_run(args, **_kwargs):
            calls.append(args)
            return SimpleNamespace(returncode=0)

        with patch("nivora_launcher.gui_service.shutil.which", return_value="/usr/bin/notify-send"), patch(
            "nivora_launcher.gui_service.subprocess.run", side_effect=fake_run
        ):
            sent = self._handle({"operation": "system_notify", "title": "Nivora Launcher", "body": "Minecraft did not start."})

        self.assertEqual(sent, {"sent": True})
        self.assertEqual(calls[0][:3], ["/usr/bin/notify-send", "--app-name", "Nivora Launcher"])
        self.assertEqual(calls[0][-2:], ["Nivora Launcher", "Minecraft did not start."])

        with patch("nivora_launcher.gui_service.shutil.which", return_value=None):
            missing = self._handle({"operation": "system_notify", "title": "Nivora Launcher", "body": "oops"})

        self.assertEqual(missing, {"sent": False})


if __name__ == "__main__":
    unittest.main()
