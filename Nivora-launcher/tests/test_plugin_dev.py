from __future__ import annotations

import base64
import copy
import hashlib
import hmac
import importlib.util
import io
import json
import stat
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.cli import build_parser
from nivora_launcher.errors import LauncherError
from nivora_launcher.plugin_dev import (
    generate_keypair,
    init_plugin,
    lint_plugin,
    pack_plugin,
    verify_plugin,
)


def _fake_security_provider():
    """Test-only provider for the signing adapter when crypto is unavailable."""
    secret = b"test-only-plugin-signing-secret"

    def canonical(manifest: dict) -> bytes:
        payload = {key: value for key, value in manifest.items() if key != "signature"}
        return json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")

    def file_hashes(file_bytes: dict[str, bytes]) -> dict[str, str]:
        return {name: hashlib.sha256(content).hexdigest() for name, content in sorted(file_bytes.items())}

    def signature_for(manifest: dict) -> str:
        return base64.b64encode(hmac.new(secret, canonical(manifest), hashlib.sha512).digest()).decode("ascii")

    def sign_manifest(manifest: dict, file_bytes: dict[str, bytes], _private_key_pem: bytes) -> dict:
        signed = copy.deepcopy(manifest)
        signed["integrity"] = {"files": file_hashes(file_bytes)}
        signed.pop("signature", None)
        signed["signature"] = {
            "algorithm": "ed25519",
            "key_id": "test-key",
            "public_key": base64.b64encode(b"p" * 32).decode("ascii"),
            "value": signature_for(signed),
        }
        return signed

    def verify_manifest(manifest: dict, file_bytes: dict[str, bytes]) -> dict:
        signature = manifest.get("signature")
        valid = (
            isinstance(signature, dict)
            and signature.get("algorithm") == "ed25519"
            and signature.get("key_id") == "test-key"
            and signature.get("public_key") == base64.b64encode(b"p" * 32).decode("ascii")
            and signature.get("value") == signature_for(manifest)
            and manifest.get("integrity") == {"files": file_hashes(file_bytes)}
        )
        return {
            "signed": isinstance(signature, dict),
            "valid": valid,
            "key_id": signature.get("key_id") if isinstance(signature, dict) else None,
            "error": None if valid else "Plugin signature is invalid",
        }

    return sign_manifest, verify_manifest


def _rewrite_manifest(path: Path, update) -> None:
    with zipfile.ZipFile(path, "r") as archive:
        entries = {name: archive.read(name) for name in archive.namelist()}
    manifest = json.loads(entries["plugin.json"].decode("utf-8"))
    update(manifest)
    entries["plugin.json"] = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode("utf-8")
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, content in sorted(entries.items()):
            archive.writestr(name, content)


class PluginDevTests(unittest.TestCase):
    def test_init_creates_safe_v3_starter_and_lints(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "My Sample Plugin"
            created = init_plugin(project)
            manifest = json.loads((created / "plugin.json").read_text(encoding="utf-8"))

            self.assertEqual(created, project.resolve())
            self.assertEqual(manifest["id"], "my-sample-plugin")
            self.assertEqual(manifest["api_version"], "3")
            self.assertEqual(manifest["entrypoint"], "main.js")
            self.assertEqual(manifest["sandbox_profile"], "strict")
            self.assertTrue((created / "README.md").is_file())
            self.assertEqual(lint_plugin(created), [])
            with self.assertRaisesRegex(LauncherError, "will not overwrite"):
                init_plugin(created)

    def test_lint_reports_network_and_filesystem_apis_as_advisory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = init_plugin(Path(directory) / "sample")
            (project / "main.js").write_text(
                'globalThis.fetch("https://example.invalid");\nreadFile("secret");\n',
                encoding="utf-8",
            )

            warnings = lint_plugin(project)

            self.assertEqual(len(warnings), 2)
            self.assertTrue(any("network API" in item and "advisory" in item for item in warnings))
            self.assertTrue(any("filesystem API" in item and "QuickJS" in item for item in warnings))

    def test_lint_rejects_static_esm_commonjs_and_reexport_imports(self) -> None:
        unsupported = (
            'import { value } from "./dependency.js";\n',
            'const value = require("./dependency.cjs");\n',
            'export { value } from "./dependency.js";\n',
        )
        with tempfile.TemporaryDirectory() as directory:
            project = init_plugin(Path(directory) / "sample")
            for source in unsupported:
                with self.subTest(source=source), self.assertRaisesRegex(LauncherError, "bundle dependencies"):
                    (project / "main.js").write_text(source, encoding="utf-8")
                    lint_plugin(project)

    def test_lint_checks_schema_and_entrypoint(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = init_plugin(Path(directory) / "sample")
            manifest_path = project / "plugin.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["permissions"] = ["filesystem:unrestricted"]
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaises(LauncherError):
                lint_plugin(project)

            manifest["permissions"] = []
            manifest["entrypoint"] = "missing.js"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(LauncherError, "entrypoint"):
                lint_plugin(project)

    def test_lint_uses_v2_manifest_schema(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = init_plugin(Path(directory) / "sample")
            manifest_path = project / "plugin.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["events"] = ["unsupported.event"]
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

            with self.assertRaisesRegex(LauncherError, "unsupported launcher event"):
                lint_plugin(project)

    def test_lint_pack_and_verify_accept_v3_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            project = init_plugin(base / "sample")
            manifest_path = project / "plugin.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest.update({
                "api_version": "3",
                "permissions": ["runtime:observe", "network:https", "network:proxy-control"],
                "network_hosts": ["example.com"],
                "sandbox_profile": "strict",
                "compatibility_matrix": {"min_launcher_version": "0.1.0"},
            })
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

            self.assertEqual(lint_plugin(project), [])
            archive = pack_plugin(project, base / "plugin-v3.zip")
            verified = verify_plugin(archive)
            self.assertEqual(verified["id"], "sample")
            self.assertEqual(verified["version"], "0.1.0")
            self.assertFalse(verified["signed"])

    def test_pack_is_deterministic_excludes_dependency_trees_and_verifies_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            project = init_plugin(base / "sample")
            (project / "assets").mkdir()
            (project / "assets" / "badge.txt").write_text("asset", encoding="utf-8")
            for excluded in (".git", "node_modules", ".venv", "__pycache__"):
                path = project / excluded
                path.mkdir()
                (path / "private.txt").write_text("excluded", encoding="utf-8")
            first = pack_plugin(project, base / "first.zip")
            second = pack_plugin(project, base / "second.zip")

            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first, "r") as archive:
                names = set(archive.namelist())
                self.assertEqual(names, {"README.md", "assets/badge.txt", "main.js", "plugin.json"})
                manifest = json.loads(archive.read("plugin.json").decode("utf-8"))
            self.assertEqual(
                set(manifest["integrity"]["files"]),
                {"README.md", "assets/badge.txt", "main.js"},
            )
            self.assertEqual(verify_plugin(first), {"id": "sample", "name": "Sample Plugin", "version": "0.1.0", "signed": False})

    def test_pack_rejects_symlinks_unsafe_paths_and_oversized_payloads(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            project = init_plugin(base / "sample")
            (project / "unsafe\\name.js").write_text("unsafe", encoding="utf-8")
            with self.assertRaisesRegex(LauncherError, "unsafe path|invalid path"):
                pack_plugin(project, base / "unsafe.zip")
            (project / "unsafe\\name.js").unlink()

            try:
                (project / "linked.js").symlink_to(project / "main.js")
            except (OSError, NotImplementedError):
                self.skipTest("symbolic links are unavailable on this platform")
            with self.assertRaisesRegex(LauncherError, "symbolic link"):
                pack_plugin(project, base / "linked.zip")
            (project / "linked.js").unlink()

            with patch("nivora_launcher.plugin_dev.PLUGIN_MAX_UNPACKED_BYTES", 32):
                with self.assertRaisesRegex(LauncherError, "unpacked size limit"):
                    pack_plugin(project, base / "oversized.zip")

    def test_verify_rejects_unsafe_zip_member_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive_path = Path(directory) / "unsafe.zip"
            manifest = {
                "id": "sample",
                "name": "Sample",
                "version": "1.0.0",
                "api_version": "2",
                "entrypoint": "main.js",
                "permissions": [],
            }
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as archive:
                archive.writestr("plugin.json", json.dumps(manifest))
                archive.writestr("../outside.txt", b"unsafe")
            archive_path.write_bytes(buffer.getvalue())

            with self.assertRaisesRegex(LauncherError, "unsafe path"):
                verify_plugin(archive_path)

    def test_verify_rejects_v2_manifest_fields_the_installer_rejects(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            project = init_plugin(base / "sample")
            archive = pack_plugin(project, base / "plugin.zip")
            _rewrite_manifest(archive, lambda manifest: manifest.update({"events": ["unsupported.event"]}))

            with self.assertRaisesRegex(LauncherError, "unsupported launcher event"):
                verify_plugin(archive)

    def test_signed_pack_and_signature_tamper_use_shared_security_api(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            project = init_plugin(base / "sample")
            key = base / "private.pem"
            key.write_bytes(b"test key bytes")
            archive = base / "signed.zip"
            provider = _fake_security_provider()
            with patch("nivora_launcher.plugin_dev._plugin_security", return_value=provider):
                pack_plugin(project, archive, key=key)
                self.assertTrue(verify_plugin(archive)["signed"])

                _rewrite_manifest(archive, lambda manifest: manifest["signature"].update({"value": base64.b64encode(b"x" * 64).decode("ascii")}))
                with self.assertRaisesRegex(LauncherError, "signature verification failed"):
                    verify_plugin(archive)

    def test_signed_pack_reports_missing_cryptography_from_shared_provider(self) -> None:
        if importlib.util.find_spec("cryptography") is not None:
            self.skipTest("cryptography is installed; this test checks its missing-dependency path")
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            project = init_plugin(base / "sample")
            key = base / "private.pem"
            key.write_bytes(b"not used before the crypto dependency check")

            with self.assertRaisesRegex(LauncherError, "cryptography package"):
                pack_plugin(project, base / "signed.zip", key=key)

    def test_keygen_uses_cryptography_or_reports_missing_dependency(self) -> None:
        try:
            from cryptography.hazmat.primitives import serialization
            from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        except ImportError:
            with tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                with self.assertRaisesRegex(LauncherError, "cryptography package"):
                    generate_keypair(base / "private.pem", base / "public.pem")
                self.assertFalse((base / "private.pem").exists())
                self.assertFalse((base / "public.pem").exists())
        else:
            with tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                private_path, public_path = generate_keypair(base / "private.pem", base / "public.pem")
                private_key = serialization.load_pem_private_key(private_path.read_bytes(), password=None)
                self.assertIsInstance(private_key, Ed25519PrivateKey)
                self.assertEqual(stat.S_IMODE(private_path.stat().st_mode), 0o600)
                self.assertIn(b"PUBLIC KEY", public_path.read_bytes())

    def test_plugin_cli_parser(self) -> None:
        parser = build_parser()
        cases = (
            (["plugin", "init", "./sample"], "init"),
            (["plugin", "lint", "./sample"], "lint"),
            (["plugin", "pack", "./sample", "./sample.zip", "--key", "./private.pem"], "pack"),
            (["plugin", "verify", "./sample.zip"], "verify"),
            (["plugin", "keygen", "./private.pem", "./public.pem"], "keygen"),
        )
        for argv, expected in cases:
            with self.subTest(argv=argv):
                args = parser.parse_args(argv)
                self.assertEqual(args.command, "plugin")
                self.assertEqual(args.plugin_command, expected)
        self.assertEqual(parser.parse_args(cases[2][0]).key, "./private.pem")


if __name__ == "__main__":
    unittest.main()
