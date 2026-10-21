from __future__ import annotations

import hashlib
import ipaddress
import io
import json
import os
import socket
import stat
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch
from urllib.request import Request

from nivora_launcher.cloud_storage import CloudStorageError, _filtered_settings_bytes, _local_target, _safe_key, sync_sources
from nivora_launcher.config import ConfigStore, Paths, Settings, _normalize_api_url, _normalize_appearance, _normalize_behavior
from nivora_launcher.constants import MSA_LOGIN_API_URL, VERSION_MANIFEST_URL
from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import _plugin_network_host, handle
from nivora_launcher.net import HttpClient, _DNS_PIN_ATTRIBUTE, _PinnedHTTPSHandler, _SafeRedirectHandler, _validate_url, verify_file
from nivora_launcher.plugins import PluginStore, parse_plugin_yml, validate_manifest
from nivora_launcher.runtime import validate_java_path
from nivora_launcher.server import download_server_core, _normalize_server_config, _validate_server_source_url, core_options, server_core_versions
from nivora_launcher.security import _map_instance_path


def _dns_result(address: str, port: int = 443) -> tuple[int, int, int, str, tuple]:
    parsed = ipaddress.ip_address(address)
    if isinstance(parsed, ipaddress.IPv6Address):
        return socket.AF_INET6, socket.SOCK_STREAM, socket.IPPROTO_TCP, "", (address, port, 0, 0)
    return socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP, "", (address, port)


class SecurityBoundaryTests(unittest.TestCase):
    def test_validate_url_rejects_local_and_http(self) -> None:
        with self.assertRaises(Exception):
            _validate_url("http://example.com/file")
        with self.assertRaises(Exception):
            _validate_url("https://127.0.0.1/file", resolve_dns=False)

    def test_validate_url_allows_public_reserved_ipv6_but_rejects_cgnat(self) -> None:
        _validate_url("https://[96ab:6dc6:1:100::a2fe:c605]/manifest.json", resolve_dns=False)
        with self.assertRaises(Exception):
            _validate_url("https://100.64.0.1/metadata", resolve_dns=False)

    def test_mojang_manifest_allows_its_reserved_public_ipv6_dns_answer(self) -> None:
        answer = (socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("96ab:6dc6:1:100::a2fe:c605", 443, 0, 0))
        with patch("nivora_launcher.net.socket.getaddrinfo", return_value=[answer]):
            _validate_url("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")

    def test_validate_url_rejects_empty_or_mixed_public_private_dns_answers(self) -> None:
        with patch("nivora_launcher.net.socket.getaddrinfo", return_value=[]), self.assertRaises(LauncherError):
            _validate_url("https://empty.example/resource")
        with patch(
            "nivora_launcher.net.socket.getaddrinfo",
            return_value=[_dns_result("93.184.216.34"), _dns_result("127.0.0.1")],
        ), self.assertRaises(LauncherError):
            _validate_url("https://mixed.example/resource")

    def test_httpclient_pins_prevalidated_a_and_aaaa_after_dns_rebinding(self) -> None:
        hostname = "rebinding.example"
        pinned_records = [_dns_result("2606:4700:4700::1111"), _dns_result("93.184.216.34")]
        dns_state = {"records": pinned_records}
        dns_queries: list[tuple[str, int]] = []
        attempted_peers: list[tuple[int, tuple]] = []
        tls_hostnames: list[str] = []
        request_buffer: list[bytes] = []

        def resolve(host: str, port: int, **_kwargs):
            dns_queries.append((host, port))
            return dns_state["records"]

        class FakeSocket:
            def __init__(self, family: int) -> None:
                self.family = family

            def settimeout(self, _timeout) -> None:
                pass

            def bind(self, _source_address) -> None:
                pass

            def connect(self, address: tuple) -> None:
                attempted_peers.append((self.family, address))
                if self.family == socket.AF_INET6:
                    raise OSError("simulated unreachable IPv6 address")

            def setsockopt(self, *_args) -> None:
                pass

            def close(self) -> None:
                pass

        class FakeTLSContext:
            def wrap_socket(self, connection, *, server_hostname: str):
                tls_hostnames.append(server_hostname)
                return connection

        class FakeResponse:
            headers = {}
            code = 200
            msg = "OK"

            def __enter__(self):
                return self

            def __exit__(self, *_args) -> None:
                pass

            def read(self, _limit: int) -> bytes:
                return b"{}"

            def info(self):
                return self.headers

        def observe_request(event: str, _url: str, _attempt: int) -> None:
            if event == "start":
                dns_state["records"] = [_dns_result("127.0.0.1")]

        client = HttpClient(retries=1, request_observer=observe_request)
        handler = next(item for item in client.opener.handlers if isinstance(item, _PinnedHTTPSHandler))

        def fake_do_open(http_class, request, **kwargs):
            connection = http_class(request.host, timeout=request.timeout, **kwargs)
            connection.connect()
            connection.putrequest(request.get_method(), request.selector)
            request_buffer.extend(connection._buffer)
            return FakeResponse()

        with (
            patch("nivora_launcher.net.socket.getaddrinfo", side_effect=resolve) as resolver,
            patch(
                "nivora_launcher.net.socket.socket",
                side_effect=lambda family, _kind: FakeSocket(family),
            ),
            patch.object(handler, "_context", FakeTLSContext()),
            patch.object(handler, "do_open", side_effect=fake_do_open),
        ):
            self.assertEqual(client.request(f"https://{hostname}/resource", max_bytes=16), b"{}")

        self.assertEqual(resolver.call_count, 1)
        self.assertEqual(dns_queries, [(hostname, 443)])
        self.assertEqual(
            attempted_peers,
            # IPv4 пробуется первым: на Windows «полуживой» IPv6 иначе
            # съедал таймаут подключения на каждом AAAA-адресе.
            [(socket.AF_INET, ("93.184.216.34", 443))],
        )
        self.assertEqual(tls_hostnames, [hostname])
        self.assertIn(f"Host: {hostname}".encode(), request_buffer)

    def test_redirect_revalidates_and_carries_its_own_dns_pins(self) -> None:
        target = "https://cdn.example/resource"
        answer = _dns_result("93.184.216.34")
        validate_redirect = Mock()
        handler = _SafeRedirectHandler(validate_redirect=validate_redirect)
        with patch("nivora_launcher.net.socket.getaddrinfo", return_value=[answer]):
            redirected = handler.redirect_request(
                Request("https://source.example/start"),
                io.BytesIO(),
                302,
                "Found",
                {},
                target,
            )
        self.assertIsNotNone(redirected)
        self.assertEqual(redirected.full_url, target)
        self.assertEqual([str(address) for address in getattr(redirected, _DNS_PIN_ATTRIBUTE)], ["93.184.216.34"])
        validate_redirect.assert_called_once_with(target)

    def test_plugin_fetch_fails_closed_when_an_explicit_proxy_is_configured(self) -> None:
        http = SimpleNamespace(proxy="http://proxy.example:8080", request=Mock())
        with (
            patch("nivora_launcher.gui_service._context", return_value=(None, http, None, None, None, None, None)),
            patch("nivora_launcher.gui_service.InstanceWorkspace"),
            patch("nivora_launcher.gui_service.InstanceFiles"),
            patch("nivora_launcher.gui_service.PluginStore"),
            patch("nivora_launcher.gui_service.ServerInstanceStore"),
            patch("nivora_launcher.gui_service.ServerFiles"),
            patch("nivora_launcher.gui_service.ServerProcessManager"),
            patch(
                "nivora_launcher.gui_service._require_plugin_permission",
                return_value={"network_hosts": ["allowed.example"]},
            ),
            patch("nivora_launcher.gui_service._resolve_public_plugin_host") as resolve_plugin_host,
        ):
            with self.assertRaises(LauncherError):
                handle(
                    {
                        "operation": "plugin_network_fetch",
                        "plugin_id": "sample",
                        "url": "https://allowed.example/data",
                    }
                )
        http.request.assert_not_called()
        resolve_plugin_host.assert_not_called()

    def test_api_urls_are_fixed_without_developer_mode(self) -> None:
        self.assertEqual(
            _normalize_api_url("https://collector.example/token", MSA_LOGIN_API_URL, allowed_hosts={"login.microsoftonline.com"}),
            MSA_LOGIN_API_URL,
        )
        self.assertEqual(
            _normalize_api_url(
                "https://collector.example/manifest",
                VERSION_MANIFEST_URL,
                allowed_hosts={"piston-meta.mojang.com"},
                developer_mode=True,
            ),
            "https://collector.example/manifest",
        )

    def test_appearance_normalization_keeps_theme_modpack_pages_and_drops_lan_rooms(self) -> None:
        # Страница LAN-комнат удалена: сохранённые конфиги с "lan-rooms"
        # не должны ломать нормализацию и возвращаются к экрану по умолчанию.
        settings = Settings()
        settings.active_theme_id = "  Aurora-Blue  "
        settings.startup_view = "lan-rooms"
        settings.last_view = "lan-rooms"
        settings.page_themes = {"modpacks": "light", "lan-rooms": "dark"}
        settings.page_layouts = {"modpacks": "wide", "lan-rooms": "compact"}
        settings.animation_settings = {
            "page_styles": {"modpacks": "scale", "lan-rooms": "scale"},
            "page_durations_ms": {"modpacks": 420, "lan-rooms": 420},
        }

        normalized = _normalize_appearance(settings)
        behavior = _normalize_behavior(settings)

        self.assertEqual(normalized["active_theme_id"], "aurora-blue")
        self.assertEqual(normalized["page_themes"]["modpacks"], "light")
        self.assertEqual(normalized["page_layouts"]["modpacks"], "wide")
        self.assertEqual(normalized["animation_settings"]["page_styles"]["modpacks"], "scale")
        self.assertEqual(normalized["animation_settings"]["page_durations_ms"]["modpacks"], 420)
        self.assertNotIn("lan-rooms", normalized["page_themes"])
        self.assertNotIn("lan-rooms", normalized["page_layouts"])
        self.assertNotIn("lan-rooms", normalized["animation_settings"]["page_styles"])
        self.assertNotIn("lan-rooms", normalized["animation_settings"]["page_durations_ms"])
        self.assertEqual(behavior["startup_view"], "instances")
        self.assertEqual(behavior["last_view"], "instances")

    def test_radius_tokens_are_clamped_and_round_trip(self) -> None:
        settings = Settings()
        settings.radius_tokens = {"card": 99, "button": -7, "control": "broken", "unknown": 42}

        normalized = _normalize_appearance(settings)

        self.assertEqual(normalized["radius_tokens"], {"card": 24, "button": 0, "control": 10})
        self.assertNotIn("unknown", normalized["radius_tokens"])

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
                store = ConfigStore(Paths())
                store.set_value("radius_tokens", {"card": 18, "button": 12, "control": 14})
                self.assertEqual(store.settings.radius_tokens, {"card": 18, "button": 12, "control": 14})
                store.set_value("radius_tokens", {"card": 5000})
                self.assertEqual(store.settings.radius_tokens, {"card": 24, "button": 12, "control": 14})

                restored = ConfigStore(Paths())
                self.assertEqual(restored.settings.radius_tokens, {"card": 24, "button": 12, "control": 14})

    def test_gui_save_settings_keeps_radius_tokens_structured(self) -> None:
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
                result = handle({"operation": "save_settings", "values": {"radius_tokens": {"card": 20, "button": 4, "control": 99}}})
                self.assertEqual(result["settings"]["radius_tokens"], {"card": 20, "button": 4, "control": 24})
                restored = ConfigStore(Paths())
                self.assertEqual(restored.settings.radius_tokens, {"card": 20, "button": 4, "control": 24})

    def test_language_setting_round_trips_and_rejects_unknown(self) -> None:
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
                result = handle({"operation": "save_settings", "values": {"language": "EN"}})
                self.assertEqual(result["settings"]["language"], "en")
                restored = ConfigStore(Paths())
                self.assertEqual(restored.settings.language, "en")
                # null — валидный способ «сбросить к выбору при старте».
                cleared = handle({"operation": "save_settings", "values": {"language": None}})
                self.assertIsNone(cleared["settings"]["language"])
                # Неверный код языка — ошибка настройки (её заворачивает мост).
                with self.assertRaises(LauncherError):
                    handle({"operation": "save_settings", "values": {"language": "jp"}})


    def test_cloud_keys_and_targets_exclude_control_files(self) -> None:
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
                self.assertEqual(_safe_key("game/instances/one/mods/example.jar"), "game/instances/one/mods/example.jar")
                with self.assertRaises(CloudStorageError):
                    _safe_key("../settings.json")
                self.assertIsNone(_local_target(config, "config/accounts.json", False))
                self.assertIsNone(_local_target(config, "data/plugins/example/plugin.yml", False))
                self.assertIsNone(_local_target(config, "data/nested/accounts.json", False))
                self.assertIsNone(_local_target(config, "data/nested/cloud-manifest.key", False))
                self.assertIsNone(_local_target(config, "game/instances/server/.trial-launcher-server.json", False))

    def test_cloud_settings_upload_is_allowlisted(self) -> None:
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
                config.settings.game_dir = str(root / "private-game")
                config.settings.cloud_url = "https://private.example.invalid/dav"
                config.settings.wallpaper_path = str(root / "secret-wallpaper.png")
                config.set_values({"instance_groups": ["Pinned"], "pinned_instance_groups": ["Pinned"]})
                filtered = json.loads(_filtered_settings_bytes(config).decode("utf-8"))
                self.assertIn("theme", filtered)
                self.assertEqual(filtered["pinned_instance_groups"], ["Pinned"])
                self.assertNotIn("game_dir", filtered)
                self.assertNotIn("cloud_url", filtered)
                self.assertNotIn("wallpaper_path", filtered)

    def test_cloud_sources_exclude_overlapping_config_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {
                    "XDG_CONFIG_HOME": str(root / "shared"),
                    "XDG_DATA_HOME": str(root / "shared"),
                    "XDG_CACHE_HOME": str(root / "cache"),
                },
                clear=False,
            ):
                config = ConfigStore(Paths())
                config.paths.accounts_file.write_text("[]", encoding="utf-8")
                protected = config.paths.instance_security_dir / "escape-poc.json"
                protected.write_text("{}", encoding="utf-8")
                keys = {key for key, _ in sync_sources(config, False)}
                self.assertIn("config/settings.json", keys)
                self.assertNotIn("data/settings.json", keys)
                self.assertNotIn("data/accounts.json", keys)
                self.assertNotIn("data/instance-security/escape-poc.json", keys)

    def test_java_path_requires_jdk_bin_and_safe_permissions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            tmp_path = Path(directory)
            java = tmp_path / "jdk" / "bin" / "java"
            java.parent.mkdir(parents=True)
            # Права каталогов фиксируем явно: umask окружения (например, 002
            # в CI-контейнерах) создаёт group-writable 0o775, который
            # валидатор обязан отвергать — тест не должен зависеть от umask.
            (tmp_path / "jdk").chmod(0o755)
            (tmp_path / "jdk" / "bin").chmod(0o755)
            java.write_bytes(b"not executed")
            java.chmod(stat.S_IRUSR | stat.S_IXUSR)
            self.assertEqual(validate_java_path(java), java.resolve())
            with self.assertRaises(Exception):
                validate_java_path(tmp_path / "script")
            self.assertFalse(java.stat().st_mode & (stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))

    def test_instance_path_mapping_does_not_replace_substrings(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            self.assertEqual(_map_instance_path(str(instance / "mods"), instance), "/instance/mods")
            self.assertEqual(_map_instance_path(f"{instance}-backup", instance), f"{instance}-backup")

    def test_plugin_manifest_rejects_ip_and_launcher_permissions(self) -> None:
        with self.assertRaises(Exception):
            validate_manifest(
                {
                    "id": "sample-plugin",
                    "name": "Sample",
                    "version": "1",
                    "entrypoint": "main.js",
                    "network_hosts": ["192.168.1.1"],
                }
            )
        with self.assertRaises(Exception):
            validate_manifest(
                {
                    "id": "sample-plugin",
                    "name": "Sample",
                    "version": "1",
                    "entrypoint": "main.js",
                    "permissions": ["files:host-read"],
                }
            )
        with self.assertRaises(Exception):
            _plugin_network_host({"network_hosts": ["example.com"]}, "127.0.0.1")

    def test_plugin_sidebar_tab_is_bounded_declarative_ui(self) -> None:
        base = {
            "id": "sample-plugin",
            "name": "Sample",
            "version": "1.0.0",
            "entrypoint": "main.js",
            "permissions": ["ui:compose"],
            "sidebar_tab": {"label": "Test tab"},
        }
        self.assertEqual(validate_manifest(base)["sidebar_tab"], {"label": "Test tab"})
        with self.assertRaises(Exception):
            validate_manifest(base | {"permissions": []})
        with self.assertRaises(Exception):
            validate_manifest(base | {"sidebar_tab": {"label": "x" * 41}})
        with self.assertRaises(Exception):
            validate_manifest(base | {"sidebar_tab": {"label": "Test", "html": "<script>"}})

    def test_plugin_archive_sidebar_tab_appears_only_when_enabled(self) -> None:
        fixture = Path(__file__).resolve().parents[1] / "examples" / "test-sidebar-tab"
        manifest_text = (fixture / "plugin.yml").read_text(encoding="utf-8")
        parsed = parse_plugin_yml(manifest_text)
        self.assertEqual(parsed["sidebar_tab"], {"label": "Test Tab"})
        archive_data = io.BytesIO()
        with zipfile.ZipFile(archive_data, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name in ("plugin.yml", "main.js", "LICENSE"):
                archive.write(fixture / name, name)

        with tempfile.TemporaryDirectory() as directory:
            store = PluginStore(SimpleNamespace(paths=SimpleNamespace(data_home=Path(directory))))
            installed = store.install_archive(archive_data.getvalue())
            self.assertFalse(installed["enabled"])
            self.assertEqual(installed["sidebar_tab"], {"label": "Test Tab"})
            self.assertFalse(store.list()[0]["enabled"])

            enabled = store.set_enabled("test-sidebar-tab", True)
            self.assertTrue(enabled["enabled"])
            self.assertEqual(store.list()[0]["sidebar_tab"], {"label": "Test Tab"})

    def test_sha256_is_checked_for_downloads(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "file.jar"
            path.write_bytes(b"trusted")
            digest = hashlib.sha256(b"trusted").hexdigest()
            self.assertTrue(verify_file(path, sha256=digest))
            self.assertFalse(verify_file(path, sha256="0" * 64))

    def test_server_core_sources_are_official_and_core_names_are_normalized(self) -> None:
        core_ids = {item["id"] for item in core_options()}
        self.assertTrue({"spigot", "paper", "purpur", "folia", "pufferfish", "patina", "leaf"}.issubset(core_ids))
        self.assertEqual(_normalize_server_config({"core": "leaves", "version": "1.21.1"})["core"], "leaf")
        _validate_server_source_url("https://api.leafmc.one/v2/projects/leaf")
        _validate_server_source_url("https://fill-data.papermc.io/v1/objects/hash/paper.jar")
        _validate_server_source_url("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
        _validate_server_source_url("https://raw.githubusercontent.com/PatinaMC/Patina/ver/1.21.1/README.md")
        with self.assertRaises(Exception):
            _validate_server_source_url("https://downloads.example.invalid/server.jar")

    def test_server_core_versions_use_the_selected_provider(self) -> None:
        class FakeHttp:
            def get_json(self, url: str):
                self.url = url
                return {"versions": {"1.21": ["1.21.8", "1.21.7"], "1.20": ["1.20.6"]}}

        http = FakeHttp()
        self.assertEqual(server_core_versions(http, "paper"), ["1.21.8", "1.21.7", "1.20.6"])
        self.assertIn("fill.papermc.io", http.url)

    def test_server_core_versions_sorted_newest_first_and_drop_prebuilds(self) -> None:
        # Purpur отдаёт список от старых к новым, fill-каталоги подмешивают rc:
        # выбор версии не должен зависеть от порядка источника
        class FakeHttp:
            def get_json(self, url: str):
                if "purpurmc" in url:
                    return {"versions": ["1.14.4", "1.20.6", "26.3", "1.21.8"]}
                return {"versions": {"26.3": ["26.3", "26.3-rc-3"], "1.20": ["1.20.6"]}}

        http = FakeHttp()
        self.assertEqual(server_core_versions(http, "purpur"), ["26.3", "1.21.8", "1.20.6", "1.14.4"])
        self.assertEqual(server_core_versions(http, "paper"), ["26.3", "1.20.6"])

    def test_papermc_v3_download_uses_official_checksum_and_url(self) -> None:
        class FakeHttp:
            def get_json(self, url: str):
                self.url = url
                return [{
                    "id": 60,
                    "downloads": {
                        "server:default": {
                            "name": "paper-1.21.8-60.jar",
                            "url": "https://fill-data.papermc.io/v1/objects/trusted/paper.jar",
                            "checksums": {"sha256": hashlib.sha256(b"trusted").hexdigest()},
                            "size": 7,
                        },
                    },
                }]

            def download(self, url: str, destination: Path, *, sha256: str | None = None, size: int | None = None, redirect_validator=None):
                self.download_url = url
                self.download_sha256 = sha256
                self.download_size = size
                destination.write_bytes(b"trusted")

        with tempfile.TemporaryDirectory() as directory:
            http = FakeHttp()
            result = download_server_core(http, Path(directory), "paper", "1.21.8")
            self.assertEqual(result["build"], "60")
            self.assertEqual(result["verified_by"], "papermc-api")
            self.assertEqual(http.download_url, "https://fill-data.papermc.io/v1/objects/trusted/paper.jar")
            self.assertEqual(http.download_sha256, hashlib.sha256(b"trusted").hexdigest())
            self.assertEqual(http.download_size, 7)

    def test_non_papermc_core_catalogs_are_restricted_to_supported_branches(self) -> None:
        class FakeHttp:
            def get_json(self, url: str):
                if "pufferfish-gg" in url:
                    return [{"name": "ver/1.21"}, {"name": "ver/1.20"}]
                if "PatinaMC" in url:
                    return [{"name": "ver/1.21.1"}, {"name": "ver/1.20.4"}]
                return {"versions": [{"id": "1.21.8", "type": "release"}, {"id": "1.20.6", "type": "release"}, {"id": "24w01a", "type": "snapshot"}]}

        http = FakeHttp()
        self.assertEqual(server_core_versions(http, "pufferfish"), ["1.21.8", "1.20.6"])
        self.assertEqual(server_core_versions(http, "patina"), ["1.21.1", "1.20.4"])
        self.assertEqual(server_core_versions(http, "spigot"), ["1.21.8", "1.20.6"])

    def test_patina_download_uses_the_authors_published_jar_link(self) -> None:
        class FakeHttp:
            def get_json(self, url: str):
                return {"default_branch": "ver/1.21.1"}

            def request(self, url: str, **kwargs):
                return b"## Download\n+ [1.21.1](https://github.com/PatinaMC/Patina/raw/refs/heads/releases/1.21/patina-paperclip-1.21-R0.1-SNAPSHOT-reobf.jar)\n"

            def download(self, url: str, destination: Path, **kwargs):
                self.download_url = url
                destination.write_bytes(b"patina")

        with tempfile.TemporaryDirectory() as directory:
            http = FakeHttp()
            result = download_server_core(http, Path(directory), "patina", "1.21.1", confirm_unverified=True)
            self.assertEqual(result["core"], "patina")
            self.assertIn("patina-paperclip-1.21", http.download_url)


if __name__ == "__main__":
    unittest.main()
