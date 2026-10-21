"""plugin signing, publisher trust, and revocation primitives."""

from __future__ import annotations

import base64
import copy
import hashlib
import json
import math
import os
import posixpath
import re
import secrets
import stat
import threading
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from .errors import LauncherError

MAX_PACKAGE_FILES = 4096
_STORE_FILENAME = "plugin-trust.json"
_STORE_SCHEMA_VERSION = 1
_MAX_STORE_BYTES = 16 * 1024 * 1024
_MAX_REVOCATION_DOCUMENT_BYTES = 1024 * 1024
_MAX_REASON_LENGTH = 1024
_IDENTIFIER_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}\Z", re.ASCII)
_WINDOWS_DRIVE_RE = re.compile(r"[A-Za-z]:")
_SHA256_RE = re.compile(r"[0-9a-f]{64}\Z", re.ASCII)

def _load_crypto():
    """Load cryptography only when a signing or verification operation needs it."""
    try:
        from cryptography.exceptions import InvalidSignature, UnsupportedAlgorithm
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
    except ImportError as exc:
        raise LauncherError("Plugin signatures require the cryptography package") from exc
    return serialization, Ed25519PrivateKey, Ed25519PublicKey, InvalidSignature, UnsupportedAlgorithm

def _validate_json_value(value: Any, *, label: str, depth: int = 0, ancestors: set[int] | None = None) -> None:
    if depth > 128:
        raise LauncherError(f"{label} is nested too deeply")
    if value is None or type(value) in (bool, int):
        return
    if type(value) is float:
        if not math.isfinite(value):
            raise LauncherError(f"{label} contains a non-finite number")
        return
    if type(value) is str:
        try:
            value.encode("utf-8")
        except UnicodeEncodeError as exc:
            raise LauncherError(f"{label} contains text that is not valid UTF-8") from exc
        return
    if type(value) not in (list, dict):
        raise LauncherError(f"{label} contains a value that is not JSON-compatible")

    active = ancestors if ancestors is not None else set()
    identity = id(value)
    if identity in active:
        raise LauncherError(f"{label} contains a circular reference")
    active.add(identity)
    try:
        if type(value) is list:
            for item in value:
                _validate_json_value(item, label=label, depth=depth + 1, ancestors=active)
        else:
            for key, item in value.items():
                if type(key) is not str:
                    raise LauncherError(f"{label} object keys must be strings")
                _validate_json_value(key, label=label, depth=depth + 1, ancestors=active)
                _validate_json_value(item, label=label, depth=depth + 1, ancestors=active)
    finally:
        active.remove(identity)

def _canonical_json_bytes(value: Any, *, label: str) -> bytes:
    _validate_json_value(value, label=label)
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError, UnicodeEncodeError, RecursionError) as exc:
        raise LauncherError(f"{label} cannot be encoded as canonical JSON") from exc

def _copy_json_object(value: Any, *, label: str) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise LauncherError(f"{label} must be a JSON object")
    try:
        copied = dict(value)
    except Exception as exc:
        raise LauncherError(f"{label} must be a JSON object") from exc
    encoded = _canonical_json_bytes(copied, label=label)
    try:
        result = json.loads(encoded.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise LauncherError(f"{label} is not valid JSON data") from exc
    if not isinstance(result, dict):
        raise LauncherError(f"{label} must be a JSON object")
    return result

def _validate_identifier(value: Any, *, label: str) -> str:
    if type(value) is not str or not _IDENTIFIER_RE.fullmatch(value):
        raise LauncherError(f"{label} must be a non-empty ASCII identifier")
    return value

def _validate_reason(value: Any, *, label: str = "Revocation reason") -> str:
    if type(value) is not str or len(value) > _MAX_REASON_LENGTH or "\x00" in value:
        raise LauncherError(f"{label} must be text of at most {_MAX_REASON_LENGTH} characters")
    try:
        value.encode("utf-8")
    except UnicodeEncodeError as exc:
        raise LauncherError(f"{label} must be valid UTF-8 text") from exc
    return value

def _validate_package_path(value: Any) -> str:
    if type(value) is not str or not value or "\x00" in value:
        raise LauncherError("Plugin package paths must be non-empty relative paths")
    if "\\" in value:
        raise LauncherError("Plugin package paths must use forward slashes, not backslashes")
    if value.startswith("/") or _WINDOWS_DRIVE_RE.match(value):
        raise LauncherError("Plugin package paths must not be absolute")
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or posixpath.normpath(value) != value:
        raise LauncherError("Plugin package paths must be normalized and cannot contain '.' or '..'")
    if value == "plugin.json":
        raise LauncherError("plugin.json is excluded from the plugin file hash map")
    return value

def _normalize_file_bytes(file_bytes: Any) -> dict[str, bytes]:
    if not isinstance(file_bytes, Mapping):
        raise LauncherError("Plugin file bytes must be a mapping of relative paths to bytes")
    try:
        items = list(file_bytes.items())
    except Exception as exc:
        raise LauncherError("Plugin file bytes could not be read") from exc
    if len(items) > MAX_PACKAGE_FILES:
        raise LauncherError(f"Plugin packages may contain at most {MAX_PACKAGE_FILES} files")

    result: dict[str, bytes] = {}
    for path, content in items:
        normalized_path = _validate_package_path(path)
        if not isinstance(content, (bytes, bytearray, memoryview)):
            raise LauncherError(f"Plugin file contents must be bytes: {normalized_path}")
        try:
            result[normalized_path] = bytes(content)
        except (TypeError, ValueError) as exc:
            raise LauncherError(f"Plugin file contents could not be read: {normalized_path}") from exc
    return result

def _decode_base64(value: Any, *, expected_length: int, label: str) -> bytes:
    if type(value) is not str:
        raise LauncherError(f"{label} must be a base64 string")
    expected_encoded_length = 4 * ((expected_length + 2) // 3)
    if len(value) != expected_encoded_length:
        raise LauncherError(f"{label} has an invalid encoded length")
    try:
        encoded = value.encode("ascii")
        decoded = base64.b64decode(encoded, validate=True)
    except (UnicodeEncodeError, ValueError) as exc:
        raise LauncherError(f"{label} is not valid base64") from exc
    if base64.b64encode(decoded).decode("ascii") != value:
        raise LauncherError(f"{label} must use canonical base64 encoding")
    if len(decoded) != expected_length:
        raise LauncherError(f"{label} must decode to exactly {expected_length} bytes")
    return decoded

def _coerce_public_key(value: Any, *, label: str = "Public key") -> bytes:
    if isinstance(value, str):
        return _decode_base64(value, expected_length=32, label=label)
    if isinstance(value, (bytes, bytearray, memoryview)):
        raw = bytes(value)
        if len(raw) == 32:
            return raw
    raise LauncherError(f"{label} must be 32 raw Ed25519 bytes or their base64 encoding")

def _fingerprint(public_key: bytes) -> str:
    return f"sha256:{hashlib.sha256(public_key).hexdigest()}"

def _parse_signature(signature: Any, *, label: str) -> tuple[str, bytes, bytes]:
    if type(signature) is not dict or set(signature) != {"algorithm", "key_id", "public_key", "value"}:
        raise LauncherError(f"{label} signature must contain exactly algorithm, key_id, public_key, and value")
    if signature["algorithm"] != "ed25519":
        raise LauncherError(f"{label} signature algorithm must be ed25519")
    key_id = _validate_identifier(signature["key_id"], label=f"{label} signature key_id")
    public_key = _decode_base64(signature["public_key"], expected_length=32, label=f"{label} public_key")
    signature_value = _decode_base64(signature["value"], expected_length=64, label=f"{label} signature value")
    return key_id, public_key, signature_value

def _validate_integrity(manifest: dict[str, Any], file_bytes: Any) -> dict[str, bytes]:
    integrity = manifest.get("integrity")
    if type(integrity) is not dict or set(integrity) != {"files"}:
        raise LauncherError("Plugin manifest integrity must contain exactly a files object")
    expected_raw = integrity["files"]
    if type(expected_raw) is not dict:
        raise LauncherError("Plugin manifest integrity.files must be an object")
    if len(expected_raw) > MAX_PACKAGE_FILES:
        raise LauncherError(f"Plugin packages may contain at most {MAX_PACKAGE_FILES} files")

    expected: dict[str, str] = {}
    for path, digest in expected_raw.items():
        normalized_path = _validate_package_path(path)
        if type(digest) is not str or not _SHA256_RE.fullmatch(digest):
            raise LauncherError(f"Plugin SHA-256 hashes must be 64 lowercase hexadecimal characters: {normalized_path}")
        expected[normalized_path] = digest

    actual = _normalize_file_bytes(file_bytes)
    if set(expected) != set(actual):
        raise LauncherError("Plugin integrity.files must exactly cover all package files except plugin.json")
    for path, content in actual.items():
        digest = hashlib.sha256(content).hexdigest()
        if expected[path] != digest:
            raise LauncherError(f"Plugin file hash mismatch: {path}")
    return actual

def sign_manifest(manifest: Mapping[str, Any], file_bytes: Mapping[str, bytes], private_key_pem: bytes | str) -> dict[str, Any]:
    """return a manifest signed over canonical JSON and exact package file hashes."""
    if not isinstance(manifest, Mapping):
        raise LauncherError("Plugin manifest must be a JSON object")
    try:
        manifest_source = dict(manifest)
    except Exception as exc:
        raise LauncherError("Plugin manifest must be a JSON object") from exc

    existing_signature = manifest_source.get("signature")
    key_id_hint = None
    if isinstance(existing_signature, Mapping) and "key_id" in existing_signature:
        key_id_hint = _validate_identifier(existing_signature["key_id"], label="Plugin signature key_id")

    unsigned = dict(manifest_source)
    unsigned.pop("signature", None)
    unsigned = _copy_json_object(unsigned, label="Plugin manifest")
    normalized_files = _normalize_file_bytes(file_bytes)
    hashes = {path: hashlib.sha256(content).hexdigest() for path, content in normalized_files.items()}
    unsigned["integrity"] = {"files": dict(sorted(hashes.items()))}

    serialization, Ed25519PrivateKey, _Ed25519PublicKey, _InvalidSignature, UnsupportedAlgorithm = _load_crypto()
    if isinstance(private_key_pem, str):
        try:
            pem_bytes = private_key_pem.encode("utf-8")
        except UnicodeEncodeError as exc:
            raise LauncherError("private_key_pem must be an unencrypted Ed25519 PEM") from exc
    elif isinstance(private_key_pem, (bytes, bytearray, memoryview)):
        pem_bytes = bytes(private_key_pem)
    else:
        raise LauncherError("private_key_pem must be an unencrypted Ed25519 PEM")
    if not pem_bytes or len(pem_bytes) > 1024 * 1024:
        raise LauncherError("private_key_pem must be an unencrypted Ed25519 PEM")
    try:
        private_key = serialization.load_pem_private_key(pem_bytes, password=None)
    except (TypeError, ValueError, UnsupportedAlgorithm) as exc:
        raise LauncherError("private_key_pem must be an unencrypted Ed25519 private key") from exc
    if not isinstance(private_key, Ed25519PrivateKey):
        raise LauncherError("private_key_pem must contain an Ed25519 private key")

    public_key = private_key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )
    key_id = key_id_hint or _fingerprint(public_key)
    canonical = _canonical_json_bytes(unsigned, label="Plugin manifest")
    unsigned["signature"] = {
        "algorithm": "ed25519",
        "key_id": key_id,
        "public_key": base64.b64encode(public_key).decode("ascii"),
        "value": base64.b64encode(private_key.sign(canonical)).decode("ascii"),
    }
    return unsigned

def verify_manifest(manifest: Mapping[str, Any], file_bytes: Mapping[str, bytes]) -> dict[str, Any]:
    """verify manifest signature and exact file coverage without consulting trust."""
    signed = isinstance(manifest, Mapping) and "signature" in manifest
    result: dict[str, Any] = {
        "signed": signed,
        "valid": False,
        "key_id": None,
        "public_key": None,
        "fingerprint": None,
        "error": None,
    }
    try:
        manifest_copy = _copy_json_object(manifest, label="Plugin manifest")
        if "signature" not in manifest_copy:
            raise LauncherError("Plugin manifest is unsigned")
        key_id, public_key, signature_value = _parse_signature(
            manifest_copy["signature"],
            label="Plugin manifest",
        )
        result.update(
            {
                "key_id": key_id,
                "public_key": base64.b64encode(public_key).decode("ascii"),
                "fingerprint": _fingerprint(public_key),
            }
        )
        _validate_integrity(manifest_copy, file_bytes)
        unsigned = dict(manifest_copy)
        unsigned.pop("signature")
        canonical = _canonical_json_bytes(unsigned, label="Plugin manifest")
        _serialization, _Ed25519PrivateKey, Ed25519PublicKey, InvalidSignature, _UnsupportedAlgorithm = _load_crypto()
        try:
            Ed25519PublicKey.from_public_bytes(public_key).verify(signature_value, canonical)
        except InvalidSignature as exc:
            raise LauncherError("Plugin signature is invalid") from exc
        result["valid"] = True
    except LauncherError as exc:
        result["error"] = str(exc)
    except (TypeError, ValueError, RecursionError, OverflowError):
        result["error"] = "Plugin manifest contains invalid data"
    return result

def _initial_store_state() -> dict[str, Any]:
    return {
        "schema_version": _STORE_SCHEMA_VERSION,
        "publishers": {},
        "local_revoked_plugins": {},
        "local_revoked_publishers": {},
        "imported_revocations": {
            "sequence": -1,
            "authority_key_id": "",
            "revoked_plugins": {},
            "revoked_publishers": {},
        },
    }

def _validate_reason_map(value: Any, *, label: str, publisher_ids: bool = False) -> None:
    if type(value) is not dict:
        raise LauncherError(f"Plugin trust store {label} must be an object")
    for identifier, reason in value.items():
        _validate_identifier(identifier, label="Publisher key ID" if publisher_ids else "Plugin ID")
        _validate_reason(reason, label=f"Plugin trust store {label} reason")

def _validate_store_state(value: Any) -> dict[str, Any]:
    required = {
        "schema_version",
        "publishers",
        "local_revoked_plugins",
        "local_revoked_publishers",
        "imported_revocations",
    }
    if type(value) is not dict or set(value) != required:
        raise LauncherError("Plugin trust store has an unsupported or invalid format")
    if type(value["schema_version"]) is not int or value["schema_version"] != _STORE_SCHEMA_VERSION:
        raise LauncherError("Plugin trust store has an unsupported schema version")
    publishers = value["publishers"]
    if type(publishers) is not dict:
        raise LauncherError("Plugin trust store publishers must be an object")
    seen_public_keys: set[bytes] = set()
    for key_id, publisher in publishers.items():
        _validate_identifier(key_id, label="Publisher key ID")
        if type(publisher) is not dict or set(publisher) != {"public_key", "revocation_authority"}:
            raise LauncherError("Plugin trust store contains an invalid publisher record")
        public_key = _decode_base64(publisher["public_key"], expected_length=32, label="Pinned public key")
        if type(publisher["revocation_authority"]) is not bool:
            raise LauncherError("Plugin trust store revocation_authority flag must be boolean")
        if public_key in seen_public_keys:
            raise LauncherError("Plugin trust store pins one public key under multiple key IDs")
        seen_public_keys.add(public_key)

    _validate_reason_map(value["local_revoked_plugins"], label="local_revoked_plugins")
    _validate_reason_map(value["local_revoked_publishers"], label="local_revoked_publishers", publisher_ids=True)
    imported = value["imported_revocations"]
    if type(imported) is not dict or set(imported) != {
        "sequence",
        "authority_key_id",
        "revoked_plugins",
        "revoked_publishers",
    }:
        raise LauncherError("Plugin trust store imported revocations are invalid")
    sequence = imported["sequence"]
    if type(sequence) is not int or sequence < -1:
        raise LauncherError("Plugin trust store revocation sequence is invalid")
    authority_key_id = imported["authority_key_id"]
    if sequence == -1:
        if authority_key_id != "":
            raise LauncherError("Plugin trust store has an authority without imported revocations")
    else:
        _validate_identifier(authority_key_id, label="Revocation authority key ID")
    _validate_reason_map(imported["revoked_plugins"], label="imported revoked_plugins")
    _validate_reason_map(imported["revoked_publishers"], label="imported revoked_publishers", publisher_ids=True)
    if sequence == -1 and (imported["revoked_plugins"] or imported["revoked_publishers"]):
        raise LauncherError("Plugin trust store has revocations without an imported sequence")
    return value

def _json_no_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, item in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = item
    return result

def _reject_json_constant(_value: str) -> None:
    raise ValueError("non-finite JSON number")

def _parse_revocation_entries(value: Any, *, label: str, id_field: str, publisher_ids: bool = False) -> dict[str, str]:
    if type(value) is not list:
        raise LauncherError(f"Revocation document {label} must be an array")
    result: dict[str, str] = {}
    for entry in value:
        if type(entry) is not dict or set(entry) != {id_field, "reason"}:
            raise LauncherError(f"Each {label} entry must contain exactly {id_field} and reason")
        identifier = _validate_identifier(
            entry[id_field],
            label="Publisher key ID" if publisher_ids else "Plugin ID",
        )
        if identifier in result:
            raise LauncherError(f"Revocation document contains a duplicate {id_field}")
        result[identifier] = _validate_reason(entry["reason"])
    return result

class PluginTrustStore:
    """private user-pinned publisher trust and revocation state."""

    def __init__(self, root: Path):
        try:
            self.root = Path(root).expanduser()
        except (TypeError, ValueError) as exc:
            raise LauncherError("Plugin trust store root must be a filesystem path") from exc
        self.path = self.root / _STORE_FILENAME
        self._lock = threading.RLock()
        self._ensure_root()
        self._state = self._load_state()

    def _ensure_root(self) -> None:
        try:
            self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
            metadata = self.root.lstat()
            if not stat.S_ISDIR(metadata.st_mode):
                raise LauncherError("Plugin trust store root must be a real directory, not a symlink")
            os.chmod(self.root, 0o700)
        except LauncherError:
            raise
        except OSError as exc:
            raise LauncherError("Could not create a private plugin trust store directory") from exc

    def _load_state(self) -> dict[str, Any]:
        try:
            metadata = self.path.lstat()
        except FileNotFoundError:
            return _initial_store_state()
        except OSError as exc:
            raise LauncherError("Could not inspect the plugin trust store file") from exc
        if not stat.S_ISREG(metadata.st_mode):
            raise LauncherError("Plugin trust store must be a regular file, not a symlink")
        if metadata.st_size > _MAX_STORE_BYTES:
            raise LauncherError("Plugin trust store file is too large")

        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        try:
            fd = os.open(self.path, flags)
            with os.fdopen(fd, "rb") as handle:
                if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                    raise LauncherError("Plugin trust store must be a regular file")
                if hasattr(os, "fchmod"):
                    os.fchmod(handle.fileno(), 0o600)
                raw = handle.read(_MAX_STORE_BYTES + 1)
        except LauncherError:
            raise
        except OSError as exc:
            raise LauncherError("Could not securely read the plugin trust store") from exc
        if len(raw) > _MAX_STORE_BYTES:
            raise LauncherError("Plugin trust store file is too large")
        try:
            state = json.loads(
                raw.decode("utf-8"),
                object_pairs_hook=_json_no_duplicate_keys,
                parse_constant=_reject_json_constant,
            )
        except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
            raise LauncherError("Plugin trust store contains invalid JSON") from exc
        return _validate_store_state(state)

    def _write_state(self, state: dict[str, Any]) -> None:
        _validate_store_state(state)
        try:
            payload = (
                json.dumps(state, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False).encode("utf-8")
                + b"\n"
            )
        except (TypeError, ValueError, UnicodeEncodeError) as exc:
            raise LauncherError("Plugin trust store data cannot be serialized") from exc
        if len(payload) > _MAX_STORE_BYTES:
            raise LauncherError("Plugin trust store would exceed its maximum size")
        self._ensure_root()

        temporary_path: Path | None = None
        fd = -1
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        for _attempt in range(8):
            candidate = self.root / f".{_STORE_FILENAME}.{secrets.token_hex(8)}.tmp"
            try:
                fd = os.open(candidate, flags, 0o600)
                temporary_path = candidate
                break
            except FileExistsError:
                continue
            except OSError as exc:
                raise LauncherError("Could not create a private temporary plugin trust store") from exc
        if fd < 0 or temporary_path is None:
            raise LauncherError("Could not allocate a temporary plugin trust store file")

        try:
            with os.fdopen(fd, "wb") as handle:
                fd = -1
                if hasattr(os, "fchmod"):
                    os.fchmod(handle.fileno(), 0o600)
                handle.write(payload)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary_path, self.path)
            temporary_path = None
            try:
                directory_fd = os.open(self.root, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
                try:
                    os.fsync(directory_fd)
                finally:
                    os.close(directory_fd)
            except OSError:
                pass
        except OSError as exc:
            raise LauncherError("Could not atomically save the plugin trust store") from exc
        finally:
            if fd >= 0:
                os.close(fd)
            if temporary_path is not None:
                try:
                    temporary_path.unlink()
                except OSError:
                    pass

    def _commit(self, state: dict[str, Any]) -> None:
        old_state = self._state
        self._state = state
        try:
            self._write_state(state)
        except BaseException:
            self._state = old_state
            raise

    def pin_publisher_key(self, key_id: str, public_key: bytes | str, *, revocation_authority: bool = False) -> None:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        raw_key = _coerce_public_key(public_key)
        if type(revocation_authority) is not bool:
            raise LauncherError("revocation_authority must be a boolean")
        encoded_key = base64.b64encode(raw_key).decode("ascii")
        with self._lock:
            current = self._state["publishers"].get(key_id)
            if current is not None:
                existing_key = _decode_base64(current["public_key"], expected_length=32, label="Pinned public key")
                if existing_key != raw_key:
                    raise LauncherError("Publisher key ID is already pinned to another public key; unpin it first")
                if revocation_authority and not current["revocation_authority"]:
                    state = copy.deepcopy(self._state)
                    state["publishers"][key_id]["revocation_authority"] = True
                    self._commit(state)
                return
            if any(
                _decode_base64(record["public_key"], expected_length=32, label="Pinned public key") == raw_key
                for record in self._state["publishers"].values()
            ):
                raise LauncherError("Publisher public key is already pinned under a different key ID")
            state = copy.deepcopy(self._state)
            state["publishers"][key_id] = {
                "public_key": encoded_key,
                "revocation_authority": revocation_authority,
            }
            self._commit(state)

    def unpin_publisher_key(self, key_id: str) -> bool:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        with self._lock:
            if key_id not in self._state["publishers"]:
                return False
            state = copy.deepcopy(self._state)
            del state["publishers"][key_id]
            self._commit(state)
            return True

    def is_publisher_pinned(self, key_id: str, public_key: bytes | str | None = None) -> bool:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        raw_key = _coerce_public_key(public_key) if public_key is not None else None
        with self._lock:
            record = self._state["publishers"].get(key_id)
            if record is None:
                return False
            if raw_key is None:
                return True
            stored_key = _decode_base64(record["public_key"], expected_length=32, label="Pinned public key")
            return stored_key == raw_key

    def is_trusted_publisher(self, key_id: str, public_key: bytes | str | None = None) -> bool:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        raw_key = _coerce_public_key(public_key) if public_key is not None else None
        with self._lock:
            record = self._state["publishers"].get(key_id)
            if record is None:
                return False
            if raw_key is not None:
                stored_key = _decode_base64(record["public_key"], expected_length=32, label="Pinned public key")
                if stored_key != raw_key:
                    return False
            return not self._publisher_revocation_status(key_id)["revoked"]

    def publisher_status(self, key_id: str, public_key: bytes | str | None = None) -> dict[str, Any]:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        raw_key = _coerce_public_key(public_key) if public_key is not None else None
        with self._lock:
            record = self._state["publishers"].get(key_id)
            pinned = record is not None
            if pinned and raw_key is not None:
                stored_key = _decode_base64(record["public_key"], expected_length=32, label="Pinned public key")
                pinned = stored_key == raw_key
            revocation = self._publisher_revocation_status(key_id)
            return {
                "key_id": key_id,
                "pinned": pinned,
                "trusted": pinned and not revocation["revoked"],
                "revoked": revocation["revoked"],
                "reason": revocation["reason"],
                "revocation_authority": bool(record and record["revocation_authority"]),
            }

    def set_revocation_authority(self, key_id: str, enabled: bool = True) -> None:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        if type(enabled) is not bool:
            raise LauncherError("enabled must be a boolean")
        with self._lock:
            record = self._state["publishers"].get(key_id)
            if record is None:
                raise LauncherError("A publisher key must be pinned before it can be a revocation authority")
            if record["revocation_authority"] == enabled:
                return
            state = copy.deepcopy(self._state)
            state["publishers"][key_id]["revocation_authority"] = enabled
            self._commit(state)

    def mark_revocation_authority(self, key_id: str) -> None:
        self.set_revocation_authority(key_id, True)

    def revoke_plugin(self, plugin_id: str, reason: str = "") -> None:
        plugin_id = _validate_identifier(plugin_id, label="Plugin ID")
        reason = _validate_reason(reason)
        with self._lock:
            if self._state["local_revoked_plugins"].get(plugin_id) == reason and plugin_id in self._state["local_revoked_plugins"]:
                return
            state = copy.deepcopy(self._state)
            state["local_revoked_plugins"][plugin_id] = reason
            self._commit(state)

    def unrevoke_plugin(self, plugin_id: str) -> bool:
        plugin_id = _validate_identifier(plugin_id, label="Plugin ID")
        with self._lock:
            if plugin_id not in self._state["local_revoked_plugins"]:
                return False
            state = copy.deepcopy(self._state)
            del state["local_revoked_plugins"][plugin_id]
            self._commit(state)
            return True

    def revoke_publisher(self, key_id: str, reason: str = "") -> None:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        reason = _validate_reason(reason)
        with self._lock:
            if self._state["local_revoked_publishers"].get(key_id) == reason and key_id in self._state["local_revoked_publishers"]:
                return
            state = copy.deepcopy(self._state)
            state["local_revoked_publishers"][key_id] = reason
            self._commit(state)

    def unrevoke_publisher(self, key_id: str) -> bool:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        with self._lock:
            if key_id not in self._state["local_revoked_publishers"]:
                return False
            state = copy.deepcopy(self._state)
            del state["local_revoked_publishers"][key_id]
            self._commit(state)
            return True

    def is_plugin_revoked(self, plugin_id: str) -> bool:
        plugin_id = _validate_identifier(plugin_id, label="Plugin ID")
        with self._lock:
            return self._plugin_revocation_status(plugin_id)["revoked"]

    def is_publisher_revoked(self, key_id: str) -> bool:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        with self._lock:
            return self._publisher_revocation_status(key_id)["revoked"]

    def plugin_revocation_reason(self, plugin_id: str) -> str | None:
        plugin_id = _validate_identifier(plugin_id, label="Plugin ID")
        with self._lock:
            return self._plugin_revocation_status(plugin_id)["reason"]

    def publisher_revocation_reason(self, key_id: str) -> str | None:
        key_id = _validate_identifier(key_id, label="Publisher key ID")
        with self._lock:
            return self._publisher_revocation_status(key_id)["reason"]

    def import_revocations_json(self, content: Any) -> dict[str, Any]:
        """Parse a bounded JSON document without losing duplicate-key evidence."""
        if type(content) is not str:
            raise LauncherError("Revocation document must be UTF-8 JSON text")
        try:
            encoded = content.encode("utf-8")
        except UnicodeEncodeError as exc:
            raise LauncherError("Revocation document must be valid UTF-8") from exc
        if len(encoded) > _MAX_REVOCATION_DOCUMENT_BYTES:
            raise LauncherError("Revocation document exceeds the 1 MiB limit")
        try:
            document = json.loads(
                encoded.decode("utf-8"),
                object_pairs_hook=_json_no_duplicate_keys,
                parse_constant=_reject_json_constant,
            )
        except (UnicodeDecodeError, ValueError, RecursionError) as exc:
            raise LauncherError(f"Revocation document is not valid JSON: {exc}") from exc
        if type(document) is not dict:
            raise LauncherError("Revocation document must be a JSON object")
        return self.import_revocations(document)

    def _plugin_revocation_status(self, plugin_id: str) -> dict[str, Any]:
        local = self._state["local_revoked_plugins"]
        imported = self._state["imported_revocations"]["revoked_plugins"]
        local_revoked = plugin_id in local
        external_revoked = plugin_id in imported
        reason = local[plugin_id] if local_revoked else imported.get(plugin_id)
        return {"revoked": local_revoked or external_revoked, "reason": reason, "local": local_revoked, "external": external_revoked}

    def _publisher_revocation_status(self, key_id: str) -> dict[str, Any]:
        local = self._state["local_revoked_publishers"]
        imported = self._state["imported_revocations"]["revoked_publishers"]
        local_revoked = key_id in local
        external_revoked = key_id in imported
        reason = local[key_id] if local_revoked else imported.get(key_id)
        return {"revoked": local_revoked or external_revoked, "reason": reason, "local": local_revoked, "external": external_revoked}

    def import_revocations(self, document: Mapping[str, Any]) -> dict[str, Any]:
        """Import a newer revocation set signed by a user-pinned authority."""
        copied = _copy_json_object(document, label="Revocation document")
        required = {"schema_version", "sequence", "revoked_plugins", "revoked_publishers", "signature"}
        if set(copied) != required:
            raise LauncherError("Revocation document must contain exactly schema_version, sequence, revoked_plugins, revoked_publishers, and signature")
        if type(copied["schema_version"]) is not int or copied["schema_version"] != 1:
            raise LauncherError("Revocation document schema_version must be 1")
        sequence = copied["sequence"]
        if type(sequence) is not int or sequence < 0:
            raise LauncherError("Revocation document sequence must be a non-negative integer")
        revoked_plugins = _parse_revocation_entries(
            copied["revoked_plugins"],
            label="revoked_plugins",
            id_field="id",
        )
        revoked_publishers = _parse_revocation_entries(
            copied["revoked_publishers"],
            label="revoked_publishers",
            id_field="key_id",
            publisher_ids=True,
        )
        authority_key_id, document_public_key, signature_value = _parse_signature(
            copied["signature"],
            label="Revocation document",
        )
        unsigned = dict(copied)
        unsigned.pop("signature")
        canonical = _canonical_json_bytes(unsigned, label="Revocation document")

        with self._lock:
            authority = self._state["publishers"].get(authority_key_id)
            if authority is None:
                raise LauncherError("Revocation document was signed by an unknown authority key")
            if not authority["revocation_authority"]:
                raise LauncherError("Revocation document signer is not pinned as a revocation authority")
            if self._publisher_revocation_status(authority_key_id)["revoked"]:
                raise LauncherError("Revocation document signer is revoked")
            pinned_key = _decode_base64(authority["public_key"], expected_length=32, label="Pinned authority public key")
            if pinned_key != document_public_key:
                raise LauncherError("Revocation document public key does not match the pinned authority")

            _serialization, _Ed25519PrivateKey, Ed25519PublicKey, InvalidSignature, _UnsupportedAlgorithm = _load_crypto()
            try:
                Ed25519PublicKey.from_public_bytes(pinned_key).verify(signature_value, canonical)
            except InvalidSignature as exc:
                raise LauncherError("Revocation document signature is invalid") from exc

            previous_sequence = self._state["imported_revocations"]["sequence"]
            if sequence <= previous_sequence:
                raise LauncherError("Revocation document sequence must be greater than the last imported sequence")
            state = copy.deepcopy(self._state)
            state["imported_revocations"] = {
                "sequence": sequence,
                "authority_key_id": authority_key_id,
                "revoked_plugins": revoked_plugins,
                "revoked_publishers": revoked_publishers,
            }
            self._commit(state)

        return {
            "sequence": sequence,
            "authority_key_id": authority_key_id,
            "revoked_plugins": sorted(revoked_plugins),
            "revoked_publishers": sorted(revoked_publishers),
        }
