from __future__ import annotations

import base64
import hashlib
import importlib.util
import io
import json
import os
import stat
import tempfile
import unittest
import zipfile
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.gui_service import _require_plugin_permission, handle
from nivora_launcher.instances import InstanceInfo
from nivora_launcher.plugin_security import sign_manifest
from nivora_launcher.plugins import PluginStore


CRYPTOGRAPHY_AVAILABLE = importlib.util.find_spec("cryptography") is not None
if CRYPTOGRAPHY_AVAILABLE:
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def _private_pem(private_key) -> bytes:
    return private_key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.PKCS8,
        encryption_algorithm=serialization.NoEncryption(),
    )


def _raw_public_key(private_key) -> bytes:
    return private_key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )


def _plugin_archive(
    *,
    plugin_id: str = "sample-v2-plugin",
    permissions: list[str] | None = None,
    network_hosts: list[str] | None = None,
    private_key=None,
    api_version: str = "2",
    entrypoint: str = "main.js",
    extra_files: dict[str, bytes] | None = None,
    ui: dict | None = None,
    manifest_overrides: dict | None = None,
) -> bytes:
    files = {
        "main.js": b'export default { activate() { return undefined; } };\n',
        "README.md": b"Plugin package test fixture.\n",
    }
    files.update(extra_files or {})
    manifest = {
        "id": plugin_id,
        "name": "Sample V2 Plugin",
        "version": "1.0.0",
        "api_version": api_version,
        "entrypoint": entrypoint,
        "permissions": permissions or [],
    }
    if network_hosts is not None:
        manifest["network_hosts"] = network_hosts
    if ui is not None:
        manifest["ui"] = ui
    if manifest_overrides:
        manifest.update(manifest_overrides)
    if private_key is not None:
        manifest = sign_manifest(manifest, files, _private_pem(private_key))
    else:
        manifest["integrity"] = {
            "files": {name: hashlib.sha256(content).hexdigest() for name, content in sorted(files.items())}
        }

    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("plugin.json", json.dumps(manifest, sort_keys=True, separators=(",", ":")))
        for name, content in files.items():
            archive.writestr(name, content)
    return output.getvalue()


def _fake_signed_archive(*, permissions: list[str] | None = None) -> tuple[bytes, str, str]:
    files = {
        "main.js": b'export default { activate() { return undefined; } };\n',
        "README.md": b"Plugin package test fixture.\n",
    }
    raw_public_key = b"p" * 32
    key_id = "test-publisher"
    manifest = {
        "id": "sample-v2-plugin",
        "name": "Sample V2 Plugin",
        "version": "1.0.0",
        "api_version": "2",
        "entrypoint": "main.js",
        "permissions": permissions or [],
        "integrity": {
            "files": {name: hashlib.sha256(content).hexdigest() for name, content in sorted(files.items())}
        },
        "signature": {
            "algorithm": "ed25519",
            "key_id": key_id,
            "public_key": base64.b64encode(raw_public_key).decode("ascii"),
            "value": base64.b64encode(b"s" * 64).decode("ascii"),
        },
    }
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("plugin.json", json.dumps(manifest, sort_keys=True, separators=(",", ":")))
        for name, content in files.items():
            archive.writestr(name, content)
    return output.getvalue(), key_id, base64.b64encode(raw_public_key).decode("ascii")


def _fake_verify_manifest(manifest: dict, file_bytes: dict[str, bytes]) -> dict:
    signature = manifest.get("signature") if isinstance(manifest, dict) else None
    result = {
        "signed": isinstance(signature, dict),
        "valid": False,
        "key_id": None,
        "public_key": None,
        "fingerprint": None,
        "error": "test signature is invalid",
    }
    if not isinstance(signature, dict):
        result["error"] = "Plugin manifest is unsigned"
        return result
    key_id = signature.get("key_id")
    public_key = signature.get("public_key")
    result.update({"key_id": key_id, "public_key": public_key})
    try:
        raw_public_key = base64.b64decode(public_key, validate=True)
        expected = manifest["integrity"]["files"]
        valid_hashes = set(expected) == set(file_bytes) and all(
            expected[name] == hashlib.sha256(content).hexdigest() for name, content in file_bytes.items()
        )
        valid_signature = (
            signature.get("algorithm") == "ed25519"
            and signature.get("value") == base64.b64encode(b"s" * 64).decode("ascii")
            and raw_public_key == b"p" * 32
        )
        if valid_hashes and valid_signature and isinstance(key_id, str):
            result.update(
                {
                    "valid": True,
                    "fingerprint": f"sha256:{hashlib.sha256(raw_public_key).hexdigest()}",
                    "error": None,
                }
            )
    except (KeyError, TypeError, ValueError):
        pass
    return result


def _legacy_archive() -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(
            "plugin.yml",
            "id: legacy-static-plugin\n"
            "name: Legacy Static Plugin\n"
            "version: 1.0.0\n"
            "entrypoint: main.js\n"
            "permissions:\n"
            "  - storage:workspace\n",
        )
        archive.writestr("main.js", "// Legacy static plugin; Python never executes this file.\n")
    return output.getvalue()


def _store(directory: str | Path) -> PluginStore:
    return PluginStore(SimpleNamespace(paths=SimpleNamespace(data_home=Path(directory))))


def _gui_request(store: PluginStore, request: dict, *, config=None, http=None, instances=None) -> dict:
    config = config or SimpleNamespace(paths=SimpleNamespace(data_home=store.data_home), settings=SimpleNamespace())
    http = http or object()
    instances = instances or object()
    patches = [
        patch("nivora_launcher.gui_service._context", return_value=(config, http, None, None, None, None, instances)),
        patch("nivora_launcher.gui_service.InstanceWorkspace", return_value=SimpleNamespace()),
        patch("nivora_launcher.gui_service.InstanceFiles", return_value=SimpleNamespace()),
        patch("nivora_launcher.gui_service.PluginStore", return_value=store),
        patch("nivora_launcher.gui_service.ServerInstanceStore", return_value=SimpleNamespace()),
        patch("nivora_launcher.gui_service.ServerFiles", return_value=SimpleNamespace()),
        patch("nivora_launcher.gui_service.ServerProcessManager", return_value=SimpleNamespace()),
    ]
    with ExitStack() as stack:
        for item in patches:
            stack.enter_context(item)
        return handle(request)


class PluginV2TrustIntegrationTests(unittest.TestCase):
    def test_signed_plugin_stays_disabled_until_its_publisher_is_pinned(self) -> None:
        content, key_id, public_key = _fake_signed_archive()
        with tempfile.TemporaryDirectory() as directory, patch(
            "nivora_launcher.plugins.verify_manifest", side_effect=_fake_verify_manifest
        ):
            store = _store(directory)
            untrusted = store.install_archive(content, enable=True)

            self.assertTrue(untrusted["package_valid"])
            self.assertFalse(untrusted["publisher_trusted"])
            self.assertFalse(untrusted["enabled"])
            self.assertEqual(untrusted["execution_blocked_reason"], "publisher-not-trusted")
            with self.assertRaisesRegex(LauncherError, "not pinned|trust store"):
                store.runtime_source("sample-v2-plugin")

            status = _gui_request(
                store,
                {"operation": "plugin_publisher_pin", "key_id": key_id, "public_key": public_key},
            )
            self.assertTrue(status["publisher"]["pinned"])
            self.assertFalse(status["plugins"][0]["enabled"])
            authority = _gui_request(
                store,
                {"operation": "plugin_publisher_revocation_authority_set", "key_id": key_id, "enabled": True},
            )
            self.assertTrue(authority["plugins"][0]["publisher_revocation_authority"])
            self.assertTrue(authority["publishers"][0]["revocation_authority"])
            enabled = store.set_enabled("sample-v2-plugin", True)
            self.assertTrue(enabled["execution_allowed"])
            self.assertIn("activate", _gui_request(store, {"operation": "plugin_api_source", "plugin_id": "sample-v2-plugin"})["source"])

    def test_signed_v2_host_access_is_granted_only_for_declared_permissions(self) -> None:
        content, key_id, public_key = _fake_signed_archive(permissions=["storage:workspace"])
        with tempfile.TemporaryDirectory() as directory, patch(
            "nivora_launcher.plugins.verify_manifest", side_effect=_fake_verify_manifest
        ):
            store = _store(directory)
            store.trust_store.pin_publisher_key(key_id, public_key)
            store.install_archive(content, enable=True)
            with self.assertRaises(LauncherError):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")
            store.set_permission_grants("sample-v2-plugin", ["storage:workspace"])
            _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")
            with self.assertRaises(LauncherError):
                store.set_permission_grants("sample-v2-plugin", ["files:plugin"])

            store.set_permission_grants("sample-v2-plugin", [])
            with self.assertRaisesRegex(LauncherError, "Пользователь"):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

    def test_signed_package_tampering_and_local_revocation_block_runtime(self) -> None:
        content, key_id, public_key = _fake_signed_archive(permissions=["storage:workspace"])
        with tempfile.TemporaryDirectory() as directory, patch(
            "nivora_launcher.plugins.verify_manifest", side_effect=_fake_verify_manifest
        ):
            store = _store(directory)
            store.trust_store.pin_publisher_key(key_id, public_key)
            store.install_archive(content, enable=True)
            store.set_permission_grants("sample-v2-plugin", ["storage:workspace"])
            store.trust_store.revoke_plugin("sample-v2-plugin", "local test revoke")
            status = store.list()[0]

            self.assertTrue(status["revoked"])
            self.assertFalse(status["execution_allowed"])
            with self.assertRaisesRegex(LauncherError, "отозван|revoked"):
                store.runtime_source("sample-v2-plugin")
            with self.assertRaisesRegex(LauncherError, "отозван|revoked"):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

            store.trust_store.unrevoke_plugin("sample-v2-plugin")
            (store.root / "sample-v2-plugin" / "main.js").write_bytes(b"tampered\n")
            self.assertFalse(store.list()[0]["package_valid"])
            with self.assertRaises(LauncherError):
                store.runtime_source("sample-v2-plugin")


class PluginV2UnsignedTests(unittest.TestCase):
    def test_unsigned_plugin_install_requires_integrity_and_explicit_opt_in(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            content = _plugin_archive(plugin_id="unsigned-sample")
            installed = store.install_archive(content, enable=True)

            self.assertTrue(installed["package_valid"])
            self.assertEqual(installed["trust"], "local")
            self.assertFalse(installed["enabled"])
            self.assertFalse(installed["local_untrusted_execution_allowed"])
            with self.assertRaisesRegex(LauncherError, "explicit|untrusted"):
                store.runtime_source("unsigned-sample")

            consent = _gui_request(
                store,
                {"operation": "plugin_local_untrusted_execution", "plugin_id": "unsigned-sample", "allowed": True},
            )
            self.assertTrue(consent["plugin"]["local_untrusted_execution_allowed"])
            enabled = store.set_enabled("unsigned-sample", True)
            self.assertTrue(enabled["enabled"])
            self.assertTrue(enabled["execution_allowed"])
            source = store.runtime_source("unsigned-sample")
            self.assertIn("activate", source["source"])

            reopened = _store(directory)
            self.assertTrue(reopened.list()[0]["local_untrusted_execution_allowed"])

    def test_unsigned_consent_is_bound_to_the_verified_package(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            store.install_archive(_plugin_archive(plugin_id="unsigned-sample"))
            store.set_local_untrusted_execution("unsigned-sample", True)
            store.set_enabled("unsigned-sample", True)

            source_path = store.root / "unsigned-sample" / "main.js"
            source_path.write_bytes(b"export default { activate() { return 'changed'; } };\n")
            status = store.list()[0]

            self.assertFalse(status["package_valid"])
            self.assertFalse(status["local_untrusted_execution_allowed"])
            self.assertFalse(status["execution_allowed"])
            with self.assertRaises(LauncherError):
                store.runtime_source("unsigned-sample")

    def test_replacement_keeps_only_still_declared_grants_and_matching_unsigned_consent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            store.install_archive(
                _plugin_archive(
                    plugin_id="permission-transition",
                    permissions=["storage:workspace", "launcher:dialogs"],
                )
            )
            store.set_permission_grants("permission-transition", ["storage:workspace", "launcher:dialogs"])
            store.set_local_untrusted_execution("permission-transition", True)

            replaced = store.install_archive(
                _plugin_archive(
                    plugin_id="permission-transition",
                    permissions=["storage:workspace", "launcher:servers"],
                ),
                replace=True,
            )
            self.assertEqual(replaced["granted_permissions"], ["storage:workspace"])
            self.assertFalse(replaced["local_untrusted_execution_allowed"])

            restored = store.install_archive(
                _plugin_archive(
                    plugin_id="permission-transition",
                    permissions=["storage:workspace", "launcher:dialogs", "launcher:servers"],
                ),
                replace=True,
            )
            self.assertEqual(restored["granted_permissions"], ["storage:workspace"])
            self.assertFalse(restored["local_untrusted_execution_allowed"])

    def test_v2_schema_rejects_unsupported_api_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            with self.assertRaisesRegex(LauncherError, "api_version"):
                store.install_archive(_plugin_archive(plugin_id="bad-v2-plugin", api_version="9"))
            # api_version 1 в plugin.json — легальный статический плагин без кода
            installed = store.install_archive(_plugin_archive(plugin_id="static-json-plugin", api_version="1"))
            self.assertEqual(installed["api_version"], "1")
            self.assertNotIn("signature", installed)

    def test_v2_schema_rejects_process_wide_lan_permission(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            with self.assertRaisesRegex(LauncherError, "unknown permission"):
                store.install_archive(_plugin_archive(plugin_id="bad-lan-plugin", permissions=["network:listen"]))

    def test_server_list_requires_grant_and_is_scoped_to_each_plugin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_ids = ["first-server-plugin", "second-server-plugin"]
            for plugin_id in plugin_ids:
                store.install_archive(
                    _plugin_archive(plugin_id=plugin_id, permissions=["launcher:servers"]),
                    enable=True,
                )
                store.set_local_untrusted_execution(plugin_id, True)
                store.set_enabled(plugin_id, True)

            with self.assertRaisesRegex(LauncherError, "не выдал|not granted"):
                _gui_request(store, {"operation": "plugin_server_list_get", "plugin_id": plugin_ids[0]})

            store.set_permission_grants(plugin_ids[0], ["launcher:servers"])
            added = _gui_request(
                store,
                {
                    "operation": "plugin_server_list_add",
                    "plugin_id": plugin_ids[0],
                    "server": {"id": "main", "name": "Main", "address": "play.example.net", "port": 25565},
                },
            )
            self.assertEqual(added["server"]["address"], "play.example.net")

            store.set_permission_grants(plugin_ids[1], ["launcher:servers"])
            second_list = _gui_request(
                store,
                {"operation": "plugin_server_list_get", "plugin_id": plugin_ids[1]},
            )
            self.assertEqual(second_list["servers"], [])
            self.assertEqual(
                _gui_request(store, {"operation": "plugin_server_list_get", "plugin_id": plugin_ids[0]})["servers"],
                [added["server"]],
            )

    def test_v2_schema_rejects_unrepresentably_large_numbers(self) -> None:
        ui = {
            "views": [
                {
                    "id": "large-order",
                    "slot": "sidebar_tab",
                    "title": "Sample",
                    "root": {"type": "text", "text": "Sample"},
                    "order": 10**1000,
                }
            ]
        }
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            with self.assertRaisesRegex(LauncherError, "finite number"):
                store.install_archive(_plugin_archive(plugin_id="large-number", ui=ui))

    def test_v2_rejects_windows_drive_paths(self) -> None:
        archives = (
            _plugin_archive(plugin_id="drive-entrypoint", entrypoint="C:/main.js"),
            _plugin_archive(plugin_id="drive-file", extra_files={"C:/payload.js": b"payload"}),
        )
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            for index, archive in enumerate(archives):
                with self.subTest(index=index), self.assertRaises(LauncherError):
                    store.install_archive(archive)


class PluginV3RuntimeFoundationTests(unittest.TestCase):
    def test_v3_manifest_installs_and_reports_runtime_capabilities(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "sample-v3-plugin"
            store.install_archive(
                _plugin_archive(
                    plugin_id=plugin_id,
                    api_version="3",
                    permissions=["runtime:observe", "files:launcher-root-read", "files:launcher-config-projected"],
                    manifest_overrides={
                        "sandbox_profile": "strict",
                        "capabilities": {"network": {"proxy": True}},
                        "compatibility_matrix": {"min_launcher_version": "0.1.0"},
                    },
                )
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["runtime:observe", "files:launcher-root-read", "files:launcher-config-projected"])

            runtime = _gui_request(store, {"operation": "plugin_api_runtime_info", "plugin_id": plugin_id})
            self.assertEqual(runtime["api_version"], "3")
            self.assertIn("runtime.manifest.v3", runtime["capabilities"])
            self.assertIn("files.launcherRootReadonly.read", runtime["capabilities"])
            self.assertEqual(runtime["sandbox_profile"], "strict")

    def test_v3_can_list_and_read_launcher_root_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "launcher-root-reader"
            store.install_archive(
                _plugin_archive(plugin_id=plugin_id, api_version="3", permissions=["files:launcher-root-read"])
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["files:launcher-root-read"])

            files = _gui_request(
                store,
                {"operation": "plugin_api_files_list", "plugin_id": plugin_id, "scope": "launcher-root-readonly", "directory": "plugins"},
            )["files"]
            self.assertTrue(any(item["path"] == "plugins/api-v3.ts" for item in files))

            content = _gui_request(
                store,
                {"operation": "plugin_api_files_read", "plugin_id": plugin_id, "scope": "launcher-root-readonly", "path": "plugins/api-v3.ts"},
            )["content"]
            self.assertIn("Nivora Launcher v3 plugins", content)

    def test_v3_can_read_projected_launcher_config_without_sensitive_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config = SimpleNamespace(
                paths=SimpleNamespace(data_home=Path(directory)),
                settings=SimpleNamespace(
                    theme="dark",
                    active_theme_id="midnight-signal",
                    accent_color="#7aa2ff",
                    sidebar_position="left",
                    sidebar_mode="adaptive",
                    sidebar_width_mode="content",
                    sidebar_width=228,
                    sidebar_floating=False,
                    sidebar_visibility="always",
                    animations="full",
                    density="comfortable",
                    ui_scale=100,
                    graphics_mode="auto",
                    launcher_version="0.1.0",
                    startup_view="instances",
                    last_view="plugins",
                    remember_last_view=True,
                    default_version_filter="release",
                    default_loader="fabric",
                    max_downloads=8,
                    download_timeout_seconds=30,
                    download_retries=3,
                    show_activity_panel=True,
                    show_toasts=True,
                    font_family="system",
                    keep_logs_days=14,
                    developer_mode=False,
                    telemetry=False,
                    offline_debug_limit_seconds=0,
                    client_id="secret-client-id",
                    telemetry_endpoint="https://telemetry.example.test",
                    http_proxy="http://proxy.internal:8080",
                    cloud_username="hidden-user",
                    cloud_url="https://cloud.example.test",
                    cloud_remote_path="hidden-path",
                ),
            )
            store = PluginStore(config)
            plugin_id = "config-projection"
            store.install_archive(
                _plugin_archive(plugin_id=plugin_id, api_version="3", permissions=["files:launcher-config-projected"])
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["files:launcher-config-projected"])

            projected = _gui_request(
                store,
                {"operation": "plugin_api_config_projected_read", "plugin_id": plugin_id, "schema_id": "launcher-public-config"},
                config=config,
            )
            self.assertEqual(projected["schema_id"], "launcher-public-config")
            self.assertEqual(projected["data"]["theme"], "dark")
            self.assertNotIn("client_id", projected["data"])
            self.assertIn("client_id", projected["removed_fields"])

    def test_v3_permission_summary_and_alias_update_operations_work(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "permission-summary-v3"
            store.install_archive(
                _plugin_archive(
                    plugin_id=plugin_id,
                    api_version="3",
                    permissions=["runtime:observe", "files:launcher-root-read"],
                )
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)

            initial = _gui_request(store, {"operation": "plugin_api_permissions_get", "plugin_id": plugin_id})
            self.assertEqual(initial["requested_permissions"], ["runtime:observe", "files:launcher-root-read"])
            self.assertEqual(initial["granted_permissions"], [])

            updated = _gui_request(
                store,
                {
                    "operation": "plugin_api_permissions_set",
                    "plugin_id": plugin_id,
                    "permissions": ["runtime:observe"],
                },
            )
            self.assertEqual(updated["permissions"]["granted_permissions"], ["runtime:observe"])

    def test_v3_runtime_quota_usage_reports_namespace_sizes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "quota-reader-v3"
            store.install_archive(
                _plugin_archive(
                    plugin_id=plugin_id,
                    api_version="3",
                    permissions=["runtime:observe", "files:plugin", "storage:workspace"],
                )
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["runtime:observe", "files:plugin", "storage:workspace"])
            store.write_storage(plugin_id, "sample", {"hello": "world"})
            store.write_file(plugin_id, "notes/test.txt", "hello")

            usage = _gui_request(
                store,
                {"operation": "plugin_api_quota_usage", "plugin_id": plugin_id},
            )
            self.assertEqual(usage["plugin_id"], plugin_id)
            self.assertGreaterEqual(usage["totals"]["bytes"], 5)
            self.assertGreaterEqual(usage["totals"]["files"], 2)
            self.assertGreaterEqual(usage["directories"]["storage"]["files"], 1)
            self.assertGreaterEqual(usage["directories"]["files"]["files"], 1)

    def test_v3_instance_listing_and_process_metrics_operations_work(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "instance-reader-v3"
            store.install_archive(
                _plugin_archive(
                    plugin_id=plugin_id,
                    api_version="3",
                    permissions=["instances:read", "process:observe"],
                )
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["instances:read", "process:observe"])

            instance_root = Path(directory) / "instance-a"
            instance_root.mkdir()
            (instance_root / ".trial-launcher-live.json").write_text(
                json.dumps({"running": False, "pid": 0, "started_at": 0}),
                encoding="utf-8",
            )
            info = InstanceInfo(name="instance-a", path=instance_root, display_name="Instance A", running=False)
            fake_instances = SimpleNamespace(list=lambda: [info], get=lambda name: info)

            listed = _gui_request(
                store,
                {"operation": "plugin_api_instances_list", "plugin_id": plugin_id},
                instances=fake_instances,
            )
            self.assertEqual(listed["instances"][0]["name"], "instance-a")

            metrics = _gui_request(
                store,
                {"operation": "plugin_api_process_metrics", "plugin_id": plugin_id, "instance_name": "instance-a"},
                instances=fake_instances,
            )
            self.assertEqual(metrics["instance"]["name"], "instance-a")
            self.assertFalse(metrics["running"])
            self.assertEqual(metrics["pid"], 0)

    def test_v3_http_request_uses_new_permission_and_allowlist(self) -> None:
        class FakeHttp:
            def __init__(self):
                self.calls = []

            def request_detailed(self, url, method="GET", data=None, headers=None, max_bytes=None, redirect_validator=None):
                self.calls.append({
                    "url": url,
                    "method": method,
                    "data": data,
                    "headers": dict(headers or {}),
                    "max_bytes": max_bytes,
                })
                return {
                    "status": 200,
                    "headers": {"Content-Type": "application/json"},
                    "url": url,
                    "body": b'{"ok":true}',
                }

        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "http-v3"
            store.install_archive(
                _plugin_archive(
                    plugin_id=plugin_id,
                    api_version="3",
                    permissions=["network:https"],
                    network_hosts=["example.com"],
                )
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["network:https"])
            http = FakeHttp()

            with patch("nivora_launcher.gui_service._resolve_public_plugin_host", return_value=[(0, 0, 0, "", ("93.184.216.34", 443))]):
                result = _gui_request(
                    store,
                    {
                        "operation": "plugin_http_request",
                        "plugin_id": plugin_id,
                        "request": {"url": "https://example.com/api", "method": "POST", "body": {"hello": "world"}},
                    },
                    http=http,
                )

            self.assertEqual(result["status"], 200)
            self.assertEqual(result["body_json"], {"ok": True})
            self.assertEqual(http.calls[0]["method"], "POST")

    def test_v3_proxy_route_policy_storage_operations_work(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            plugin_id = "proxy-routes-v3"
            store.install_archive(
                _plugin_archive(
                    plugin_id=plugin_id,
                    api_version="3",
                    permissions=["network:proxy-control"],
                )
            )
            store.set_local_untrusted_execution(plugin_id, True)
            store.set_enabled(plugin_id, True)
            store.set_permission_grants(plugin_id, ["network:proxy-control"])

            added = _gui_request(
                store,
                {
                    "operation": "plugin_api_proxy_routes_add",
                    "plugin_id": plugin_id,
                    "route": {
                        "priority": 10,
                        "match": {"hostEquals": ["example.com"], "protocol": ["tcp"], "portIn": [443]},
                        "action": {"type": "PROXY", "sessionId": "primary-session"},
                    },
                },
            )
            self.assertEqual(added["route"]["action"]["type"], "PROXY")
            self.assertEqual(added["routes"][0]["match"]["hostEquals"], ["example.com"])

            listed = _gui_request(store, {"operation": "plugin_api_proxy_routes_get", "plugin_id": plugin_id})
            self.assertEqual(len(listed["routes"]), 1)

            removed = _gui_request(
                store,
                {
                    "operation": "plugin_api_proxy_routes_remove",
                    "plugin_id": plugin_id,
                    "route_id": added["route"]["id"],
                },
            )
            self.assertTrue(removed["removed"])
            self.assertEqual(removed["routes"], [])


@unittest.skipUnless(CRYPTOGRAPHY_AVAILABLE, "cryptography is needed for Ed25519 plugin tests")
class PluginV2SignedTests(unittest.TestCase):
    def _key(self):
        return Ed25519PrivateKey.generate()

    def _install_trusted(self, store: PluginStore, *, permissions: list[str] | None = None) -> tuple[dict, object]:
        private_key = self._key()
        archive = _plugin_archive(
            permissions=permissions,
            network_hosts=["example.com"] if permissions and "network:fetch" in permissions else None,
            private_key=private_key,
        )
        raw_public_key = _raw_public_key(private_key)
        key_id = f"sha256:{hashlib.sha256(raw_public_key).hexdigest()}"
        store.trust_store.pin_publisher_key(key_id, raw_public_key)
        installed = store.install_archive(archive, enable=True)
        return installed, private_key

    def test_gui_imports_a_signed_revocation_document_from_an_explicit_authority(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            installed, private_key = self._install_trusted(store)
            key_id = installed["publisher_key_id"]
            public_key = _raw_public_key(private_key)
            _gui_request(
                store,
                {"operation": "plugin_publisher_revocation_authority_set", "key_id": key_id, "enabled": True},
            )
            unsigned = {
                "schema_version": 1,
                "sequence": 1,
                "revoked_plugins": [{"id": installed["id"], "reason": "confirmed compromised"}],
                "revoked_publishers": [],
            }
            canonical = json.dumps(unsigned, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
            document = {
                **unsigned,
                "signature": {
                    "algorithm": "ed25519",
                    "key_id": key_id,
                    "public_key": base64.b64encode(public_key).decode("ascii"),
                    "value": base64.b64encode(private_key.sign(canonical)).decode("ascii"),
                },
            }

            result = _gui_request(
                store,
                {
                    "operation": "plugin_revocations_import",
                    "document_json": json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False),
                },
            )

            self.assertEqual(result["imported"]["sequence"], 1)
            self.assertTrue(result["plugins"][0]["revoked"])
            self.assertEqual(result["plugins"][0]["revocation_reason"], "confirmed compromised")

    def test_signed_install_uses_explicit_local_publisher_pin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            private_key = self._key()
            archive = _plugin_archive(private_key=private_key)
            raw_public_key = _raw_public_key(private_key)
            public_key = base64.b64encode(raw_public_key).decode("ascii")
            key_id = f"sha256:{hashlib.sha256(raw_public_key).hexdigest()}"
            pinned = _gui_request(
                store,
                {
                    "operation": "plugin_publisher_pin",
                    "key_id": key_id,
                    "public_key": public_key,
                },
            )
            self.assertTrue(pinned["publisher"]["pinned"])

            installed = store.install_archive(archive, enable=True)
            self.assertTrue(installed["package_valid"])
            self.assertTrue(installed["signature_valid"])
            self.assertTrue(installed["publisher_trusted"])
            self.assertTrue(installed["enabled"])
            self.assertTrue(installed["execution_allowed"])
            self.assertEqual(store.runtime_source("sample-v2-plugin")["source"], "export default { activate() { return undefined; } };\n")

            trust_path = Path(directory) / "plugin-trust" / "plugin-trust.json"
            self.assertTrue(trust_path.is_file())
            if os.name == "posix":
                self.assertEqual(stat.S_IMODE(trust_path.stat().st_mode), 0o600)

    def test_untrusted_signed_plugin_is_installed_disabled_and_cannot_execute(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            private_key = self._key()
            installed = store.install_archive(_plugin_archive(private_key=private_key), enable=True)

            self.assertTrue(installed["package_valid"])
            self.assertTrue(installed["signature_valid"])
            self.assertFalse(installed["publisher_trusted"])
            self.assertFalse(installed["enabled"])
            self.assertEqual(installed["execution_blocked_reason"], "publisher-not-trusted")
            with self.assertRaisesRegex(LauncherError, "not pinned|trust store"):
                store.set_enabled("sample-v2-plugin", True)
            with self.assertRaises(LauncherError):
                store.runtime_source("sample-v2-plugin")

    def test_v2_host_operations_require_declared_and_persistently_granted_permissions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            installed, _private_key = self._install_trusted(
                store,
                permissions=["storage:workspace", "network:fetch"],
            )
            self.assertTrue(installed["enabled"])
            with self.assertRaisesRegex(LauncherError, "user has not|Пользователь"):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

            status = _gui_request(
                store,
                {
                    "operation": "plugin_permission_grants_set",
                    "plugin_id": "sample-v2-plugin",
                    "permissions": ["storage:workspace"],
                },
            )
            self.assertEqual(status["plugin"]["requested_permissions"], ["storage:workspace", "network:fetch"])
            self.assertEqual(status["plugin"]["granted_permissions"], ["storage:workspace"])
            self.assertEqual(_require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")["id"], "sample-v2-plugin")
            with self.assertRaisesRegex(LauncherError, "user has not|Пользователь"):
                _require_plugin_permission(store, "sample-v2-plugin", "network:fetch")
            with self.assertRaisesRegex(LauncherError, "only|только"):
                store.set_permission_grants("sample-v2-plugin", ["files:plugin"])

            persisted = _store(directory).list()[0]
            self.assertEqual(persisted["granted_permissions"], ["storage:workspace"])

    def test_revoking_a_permission_grant_denies_the_host_operation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            self._install_trusted(store, permissions=["storage:workspace"])
            store.set_permission_grants("sample-v2-plugin", ["storage:workspace"])
            _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

            store.set_permission_grants("sample-v2-plugin", [])
            with self.assertRaisesRegex(LauncherError, "user has not|Пользователь"):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

    def test_package_tampering_after_install_blocks_listing_load_and_host_calls(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            self._install_trusted(store, permissions=["storage:workspace"])
            store.set_permission_grants("sample-v2-plugin", ["storage:workspace"])
            (store.root / "sample-v2-plugin" / "main.js").write_bytes(b"tampered\n")

            status = store.list()[0]
            self.assertFalse(status["package_valid"])
            self.assertFalse(status["execution_allowed"])
            with self.assertRaisesRegex(LauncherError, "verification|hash mismatch"):
                store.manifest("sample-v2-plugin")
            with self.assertRaises(LauncherError):
                store.runtime_source("sample-v2-plugin")
            with self.assertRaises(LauncherError):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

    def test_local_plugin_revocation_after_install_blocks_runtime_and_host_calls(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            self._install_trusted(store, permissions=["storage:workspace"])
            store.set_permission_grants("sample-v2-plugin", ["storage:workspace"])
            store.trust_store.revoke_plugin("sample-v2-plugin", "test revocation")

            status = store.list()[0]
            self.assertTrue(status["revoked"])
            self.assertFalse(status["execution_allowed"])
            with self.assertRaisesRegex(LauncherError, "revoked|отозван"):
                store.runtime_source("sample-v2-plugin")
            with self.assertRaisesRegex(LauncherError, "revoked|отозван"):
                _require_plugin_permission(store, "sample-v2-plugin", "storage:workspace")

    def test_gui_security_status_and_plugin_api_source_operations(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            private_key = self._key()
            archive = _plugin_archive(private_key=private_key)
            raw_public_key = _raw_public_key(private_key)
            public_key = base64.b64encode(raw_public_key).decode("ascii")
            key_id = f"sha256:{hashlib.sha256(raw_public_key).hexdigest()}"
            _gui_request(
                store,
                {
                    "operation": "plugin_publisher_pin",
                    "key_id": key_id,
                    "public_key": public_key,
                },
            )
            store.install_archive(archive, enable=True)

            status = _gui_request(store, {"operation": "plugin_security_status"})
            self.assertTrue(status["plugins"][0]["publisher_trusted"])
            source = _gui_request(store, {"operation": "plugin_api_source", "plugin_id": "sample-v2-plugin"})
            self.assertEqual(source["manifest"]["api_version"], "2")
            self.assertIn("activate", source["source"])

    def test_gui_plugin_api_authorization_requires_a_user_grant(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            self._install_trusted(store, permissions=["storage:workspace"])

            with self.assertRaisesRegex(LauncherError, "не выдал"):
                _gui_request(
                    store,
                    {
                        "operation": "plugin_api_authorize",
                        "plugin_id": "sample-v2-plugin",
                        "permission": "storage:workspace",
                    },
                )

            store.set_permission_grants("sample-v2-plugin", ["storage:workspace"])
            self.assertEqual(
                _gui_request(
                    store,
                    {
                        "operation": "plugin_api_authorize",
                        "plugin_id": "sample-v2-plugin",
                        "permission": "storage:workspace",
                    },
                ),
                {"authorized": True},
            )

    def test_external_plugin_links_are_authorized_and_http_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            self._install_trusted(store, permissions=["links:open"])
            store.set_permission_grants("sample-v2-plugin", ["links:open"])

            with patch("nivora_launcher.gui_service.open_path") as open_path:
                result = _gui_request(
                    store,
                    {
                        "operation": "plugin_api_open_external",
                        "plugin_id": "sample-v2-plugin",
                        "url": "https://example.com/docs",
                    },
                )
            self.assertEqual(result, {"opened": True})
            open_path.assert_called_once_with(Path("https://example.com/docs"))

            with self.assertRaisesRegex(LauncherError, r"HTTP\(S\)"):
                _gui_request(
                    store,
                    {
                        "operation": "plugin_api_open_external",
                        "plugin_id": "sample-v2-plugin",
                        "url": "javascript:alert(1)",
                    },
                )


class LegacyPluginCompatibilityTests(unittest.TestCase):
    def test_legacy_plugin_yml_static_plugin_keeps_existing_permission_behavior(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = _store(directory)
            installed = store.install_archive(_legacy_archive())
            self.assertEqual(installed["trust"], "local")
            self.assertFalse(installed["enabled"])

            enabled = store.set_enabled("legacy-static-plugin", True)
            self.assertTrue(enabled["enabled"])
            manifest = _require_plugin_permission(store, "legacy-static-plugin", "storage:workspace")
            self.assertEqual(manifest["api_version"], "1")
            self.assertFalse((store.root / "legacy-static-plugin" / "plugin.json").exists())


if __name__ == "__main__":
    unittest.main()
