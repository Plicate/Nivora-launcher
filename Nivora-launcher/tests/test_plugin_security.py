from __future__ import annotations

import base64
import copy
import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.plugin_security import PluginTrustStore, sign_manifest, verify_manifest


CRYPTOGRAPHY_AVAILABLE = importlib.util.find_spec("cryptography") is not None
if CRYPTOGRAPHY_AVAILABLE:
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


FILES = {"main.py": b"print('hello')\n", "assets/icon.bin": b"\x00\x01\xff"}


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


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _signed_revocations(private_key, key_id: str, sequence: int, *, plugins=None, publishers=None) -> dict:
    public_key = _raw_public_key(private_key)
    document = {
        "schema_version": 1,
        "sequence": sequence,
        "revoked_plugins": plugins or [],
        "revoked_publishers": publishers or [],
    }
    document["signature"] = {
        "algorithm": "ed25519",
        "key_id": key_id,
        "public_key": base64.b64encode(public_key).decode("ascii"),
        "value": base64.b64encode(private_key.sign(_canonical(document))).decode("ascii"),
    }
    return document


class PluginSecurityValidationTests(unittest.TestCase):
    def test_crypto_dependency_is_loaded_lazily(self) -> None:
        real_import = __import__

        def reject_crypto(name, *args, **kwargs):
            if name == "cryptography" or name.startswith("cryptography."):
                raise ImportError("blocked for lazy import test")
            return real_import(name, *args, **kwargs)

        with patch("builtins.__import__", side_effect=reject_crypto):
            with self.assertRaisesRegex(LauncherError, "cryptography package"):
                sign_manifest({"id": "example"}, {}, b"unused")

    def test_unsigned_manifest_returns_a_structured_error(self) -> None:
        result = verify_manifest({"id": "example"}, {})

        self.assertFalse(result["signed"])
        self.assertFalse(result["valid"])
        self.assertIsNone(result["key_id"])
        self.assertIn("unsigned", result["error"])

    def test_signing_rejects_unsafe_paths_before_loading_crypto(self) -> None:
        invalid_paths = (
            "../escape.txt",
            "/absolute.txt",
            "C:/absolute.txt",
            "a/./b",
            "a//b",
            "a\\b",
            "plugin.json",
        )
        for path in invalid_paths:
            with self.subTest(path=path), self.assertRaises(LauncherError):
                sign_manifest({"id": "example"}, {path: b"data"}, b"unused")

    def test_signing_rejects_too_many_files_before_loading_crypto(self) -> None:
        files = {f"file-{index}.dat": b"x" for index in range(4097)}

        with self.assertRaisesRegex(LauncherError, "at most 4096"):
            sign_manifest({"id": "example"}, files, b"unused")

    def test_revocation_json_parser_rejects_duplicate_keys_and_oversized_text(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = PluginTrustStore(Path(directory) / "trust")
            with self.assertRaisesRegex(LauncherError, "duplicate JSON key"):
                store.import_revocations_json('{"schema_version":1,"schema_version":1}')
            with self.assertRaisesRegex(LauncherError, "1 MiB limit"):
                store.import_revocations_json(" " * (1024 * 1024 + 1))


@unittest.skipUnless(CRYPTOGRAPHY_AVAILABLE, "cryptography is needed for Ed25519 tests")
class PluginSecurityCryptoTests(unittest.TestCase):
    def setUp(self) -> None:
        self.private_key = Ed25519PrivateKey.generate()
        self.pem = _private_pem(self.private_key)

    def test_manifest_is_signed_over_canonical_json_with_integrity_included(self) -> None:
        manifest = {"id": "cafe-plugin", "name": "Café", "metadata": {"z": 1, "a": 2}}
        original = copy.deepcopy(manifest)

        signed = sign_manifest(manifest, FILES, self.pem)
        signature = signed["signature"]
        public_key = self.private_key.public_key()
        unsigned = dict(signed)
        unsigned.pop("signature")
        public_key.verify(base64.b64decode(signature["value"]), _canonical(unsigned))

        self.assertEqual(manifest, original)
        self.assertEqual(set(signature), {"algorithm", "key_id", "public_key", "value"})
        self.assertEqual(signature["algorithm"], "ed25519")
        self.assertEqual(signature["key_id"], f"sha256:{hashlib.sha256(_raw_public_key(self.private_key)).hexdigest()}")
        self.assertEqual(
            signed["integrity"]["files"],
            {path: hashlib.sha256(content).hexdigest() for path, content in FILES.items()},
        )

        result = verify_manifest(signed, FILES)
        self.assertEqual(
            result,
            {
                "signed": True,
                "valid": True,
                "key_id": signature["key_id"],
                "public_key": signature["public_key"],
                "fingerprint": signature["key_id"],
                "error": None,
            },
        )

    def test_manifest_preserves_explicit_key_id_and_detects_tampering(self) -> None:
        signed = sign_manifest({"id": "example", "signature": {"key_id": "publisher:one"}}, FILES, self.pem)
        self.assertEqual(signed["signature"]["key_id"], "publisher:one")
        self.assertTrue(verify_manifest(signed, FILES)["valid"])

        changed_manifest = copy.deepcopy(signed)
        changed_manifest["name"] = "tampered"
        result = verify_manifest(changed_manifest, FILES)
        self.assertFalse(result["valid"])
        self.assertIn("signature is invalid", result["error"])

        changed_signature = copy.deepcopy(signed)
        signature_bytes = bytearray(base64.b64decode(changed_signature["signature"]["value"]))
        signature_bytes[0] ^= 1
        changed_signature["signature"]["value"] = base64.b64encode(signature_bytes).decode("ascii")
        result = verify_manifest(changed_signature, FILES)
        self.assertFalse(result["valid"])
        self.assertIn("signature is invalid", result["error"])

    def test_manifest_requires_exact_file_coverage_and_matching_hashes(self) -> None:
        signed = sign_manifest({"id": "example"}, FILES, self.pem)
        cases = (
            ({"main.py": FILES["main.py"]}, "exactly cover"),
            ({**FILES, "extra.txt": b"extra"}, "exactly cover"),
            ({**FILES, "main.py": b"changed"}, "hash mismatch"),
        )
        for file_bytes, expected_error in cases:
            with self.subTest(expected_error=expected_error):
                result = verify_manifest(signed, file_bytes)
                self.assertFalse(result["valid"])
                self.assertIn(expected_error, result["error"])

    def test_package_file_limit_accepts_exactly_4096_entries(self) -> None:
        files = {f"file-{index}.dat": b"x" for index in range(4096)}

        signed = sign_manifest({"id": "example"}, files, self.pem)

        self.assertEqual(len(signed["integrity"]["files"]), 4096)
        self.assertTrue(verify_manifest(signed, files)["valid"])

    def test_manifest_rejects_invalid_hash_format_and_invalid_file_map(self) -> None:
        signed = sign_manifest({"id": "example"}, FILES, self.pem)
        uppercase = copy.deepcopy(signed)
        uppercase["integrity"]["files"]["main.py"] = "A" * 64
        result = verify_manifest(uppercase, FILES)
        self.assertFalse(result["valid"])
        self.assertIn("lowercase hexadecimal", result["error"])

        unsafe_path = copy.deepcopy(signed)
        unsafe_path["integrity"]["files"]["../escape"] = unsafe_path["integrity"]["files"].pop("main.py")
        result = verify_manifest(unsafe_path, FILES)
        self.assertFalse(result["valid"])
        self.assertIn("normalized", result["error"])

        malformed_signature = copy.deepcopy(signed)
        malformed_signature["signature"]["value"] = "!"
        result = verify_manifest(malformed_signature, FILES)
        self.assertFalse(result["valid"])
        self.assertIn("encoded length", result["error"])

    def test_trust_store_pin_revoke_persistence_and_private_modes(self) -> None:
        raw_key = _raw_public_key(self.private_key)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "trust"
            store = PluginTrustStore(root)
            store.pin_publisher_key("publisher:one", raw_key)
            self.assertTrue(store.is_publisher_pinned("publisher:one", base64.b64encode(raw_key).decode("ascii")))
            self.assertTrue(store.is_trusted_publisher("publisher:one", raw_key))
            self.assertFalse(store.is_publisher_pinned("publisher:one", _raw_public_key(Ed25519PrivateKey.generate())))
            with self.assertRaisesRegex(LauncherError, "already pinned under a different key ID"):
                store.pin_publisher_key("publisher:alias", raw_key)
            with self.assertRaisesRegex(LauncherError, "already pinned to another public key"):
                store.pin_publisher_key("publisher:one", _raw_public_key(Ed25519PrivateKey.generate()))

            store.revoke_plugin("example-plugin", "malicious release")
            store.revoke_publisher("publisher:one", "key compromised")
            self.assertTrue(store.is_plugin_revoked("example-plugin"))
            self.assertEqual(store.plugin_revocation_reason("example-plugin"), "malicious release")
            self.assertTrue(store.is_publisher_revoked("publisher:one"))
            self.assertFalse(store.is_trusted_publisher("publisher:one"))
            self.assertTrue(store.unrevoke_plugin("example-plugin"))
            self.assertTrue(store.unrevoke_publisher("publisher:one"))
            self.assertFalse(store.is_plugin_revoked("example-plugin"))
            self.assertTrue(store.is_trusted_publisher("publisher:one"))

            reopened = PluginTrustStore(root)
            self.assertTrue(reopened.is_publisher_pinned("publisher:one", raw_key))
            self.assertTrue(reopened.is_trusted_publisher("publisher:one"))
            self.assertFalse(reopened.is_plugin_revoked("example-plugin"))
            if os.name == "posix":
                self.assertEqual(root.stat().st_mode & 0o777, 0o700)
                self.assertEqual((root / "plugin-trust.json").stat().st_mode & 0o777, 0o600)

            self.assertTrue(reopened.unpin_publisher_key("publisher:one"))
            self.assertFalse(reopened.is_publisher_pinned("publisher:one"))

    def test_local_revocations_are_separate_from_imported_revocations(self) -> None:
        authority_id = "authority:one"
        with tempfile.TemporaryDirectory() as directory:
            store = PluginTrustStore(Path(directory) / "trust")
            store.pin_publisher_key(authority_id, _raw_public_key(self.private_key), revocation_authority=True)
            store.import_revocations(
                _signed_revocations(
                    self.private_key,
                    authority_id,
                    1,
                    plugins=[{"id": "example-plugin", "reason": "signed notice"}],
                )
            )
            store.revoke_plugin("example-plugin", "local override")
            self.assertEqual(store.plugin_revocation_reason("example-plugin"), "local override")
            self.assertTrue(store.unrevoke_plugin("example-plugin"))
            self.assertTrue(store.is_plugin_revoked("example-plugin"))
            self.assertEqual(store.plugin_revocation_reason("example-plugin"), "signed notice")

    def test_revocation_authority_import_and_persistence(self) -> None:
        authority_id = "authority:one"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "trust"
            store = PluginTrustStore(root)
            store.pin_publisher_key(authority_id, _raw_public_key(self.private_key))
            document = _signed_revocations(
                self.private_key,
                authority_id,
                1,
                plugins=[{"id": "bad-plugin", "reason": "confirmed malware"}],
                publishers=[{"key_id": "publisher:bad", "reason": "signing key stolen"}],
            )
            with self.assertRaisesRegex(LauncherError, "not pinned as a revocation authority"):
                store.import_revocations(document)

            store.mark_revocation_authority(authority_id)
            result = store.import_revocations(document)
            self.assertEqual(result["sequence"], 1)
            self.assertEqual(result["authority_key_id"], authority_id)
            self.assertTrue(store.is_plugin_revoked("bad-plugin"))
            self.assertEqual(store.plugin_revocation_reason("bad-plugin"), "confirmed malware")
            self.assertTrue(store.is_publisher_revoked("publisher:bad"))
            self.assertEqual(store.publisher_revocation_reason("publisher:bad"), "signing key stolen")

            reopened = PluginTrustStore(root)
            self.assertTrue(reopened.publisher_status(authority_id)["revocation_authority"])
            self.assertTrue(reopened.is_plugin_revoked("bad-plugin"))
            self.assertTrue(reopened.is_publisher_revoked("publisher:bad"))

            cleared = _signed_revocations(self.private_key, authority_id, 2)
            reopened.import_revocations(cleared)
            self.assertFalse(reopened.is_plugin_revoked("bad-plugin"))
            self.assertFalse(reopened.is_publisher_revoked("publisher:bad"))

    def test_revocation_import_rejects_unknown_authority_tampering_and_rollback(self) -> None:
        authority_id = "authority:one"
        with tempfile.TemporaryDirectory() as directory:
            store = PluginTrustStore(Path(directory) / "trust")
            store.pin_publisher_key(authority_id, _raw_public_key(self.private_key), revocation_authority=True)

            unknown_key = Ed25519PrivateKey.generate()
            unknown_document = _signed_revocations(unknown_key, "authority:unknown", 1)
            with self.assertRaisesRegex(LauncherError, "unknown authority"):
                store.import_revocations(unknown_document)

            valid = _signed_revocations(
                self.private_key,
                authority_id,
                2,
                plugins=[{"id": "bad-plugin", "reason": "first reason"}],
            )
            tampered = copy.deepcopy(valid)
            tampered["revoked_plugins"][0]["reason"] = "changed after signing"
            with self.assertRaisesRegex(LauncherError, "signature is invalid"):
                store.import_revocations(tampered)

            store.import_revocations(valid)
            rollback = _signed_revocations(self.private_key, authority_id, 1)
            with self.assertRaisesRegex(LauncherError, "greater than the last imported sequence"):
                store.import_revocations(rollback)

    def test_revocation_json_import_is_bounded_and_preserves_duplicate_key_checks(self) -> None:
        authority_id = "authority:json"
        with tempfile.TemporaryDirectory() as directory:
            store = PluginTrustStore(Path(directory) / "trust")
            store.pin_publisher_key(authority_id, _raw_public_key(self.private_key), revocation_authority=True)
            document = _signed_revocations(
                self.private_key,
                authority_id,
                1,
                plugins=[{"id": "bad-plugin", "reason": "signed from JSON"}],
            )

            imported = store.import_revocations_json(json.dumps(document, sort_keys=True, separators=(",", ":")))
            self.assertEqual(imported["sequence"], 1)
            self.assertEqual(store.plugin_revocation_reason("bad-plugin"), "signed from JSON")

            with self.assertRaisesRegex(LauncherError, "duplicate JSON key"):
                store.import_revocations_json('{"schema_version":1,"schema_version":1}')
            with self.assertRaisesRegex(LauncherError, "1 MiB limit"):
                store.import_revocations_json(" " * (1024 * 1024 + 1))

    def test_revocation_document_rejects_bad_schema_and_duplicate_ids(self) -> None:
        authority_id = "authority:one"
        with tempfile.TemporaryDirectory() as directory:
            store = PluginTrustStore(Path(directory) / "trust")
            store.pin_publisher_key(authority_id, _raw_public_key(self.private_key), revocation_authority=True)

            invalid_sequence = _signed_revocations(self.private_key, authority_id, 1)
            invalid_sequence["sequence"] = True
            with self.assertRaisesRegex(LauncherError, "non-negative integer"):
                store.import_revocations(invalid_sequence)

            duplicate = _signed_revocations(
                self.private_key,
                authority_id,
                1,
                plugins=[{"id": "bad-plugin", "reason": "one"}, {"id": "bad-plugin", "reason": "two"}],
            )
            with self.assertRaisesRegex(LauncherError, "duplicate id"):
                store.import_revocations(duplicate)


if __name__ == "__main__":
    unittest.main()
