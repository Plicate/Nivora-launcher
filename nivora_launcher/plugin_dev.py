from __future__ import annotations

import base64
import binascii
import copy
import hashlib
import hmac
import io
import json
import os
import re
import stat
import tempfile
import zipfile
from pathlib import Path
from typing import Any

from .errors import LauncherError
from .plugins import (
    PLUGIN_MAX_ARCHIVE_BYTES,
    PLUGIN_MAX_FILES,
    PLUGIN_MAX_MANIFEST_BYTES,
    PLUGIN_MAX_UNPACKED_BYTES,
    validate_plugin_json_manifest,
    validate_v2_manifest,
)


PLUGIN_MANIFEST = "plugin.json"
_EXCLUDED_DIRECTORIES = frozenset({".git", "node_modules", ".venv", "__pycache__"})
_SIGNATURE_KEYS = frozenset({"algorithm", "key_id", "public_key", "value"})
_NETWORK_APIS = frozenset({"fetch", "XMLHttpRequest", "WebSocket", "EventSource", "WebTransport"})
_FILESYSTEM_APIS = frozenset(
    {
        "readFile",
        "writeFile",
        "readFileSync",
        "writeFileSync",
        "createReadStream",
        "createWriteStream",
        "readdir",
        "unlink",
        "mkdir",
        "openSync",
        "Deno",
        "Bun",
    }
)


def init_plugin(directory: str | Path) -> Path:
    requested = Path(directory).expanduser()
    if requested.is_symlink():
        raise LauncherError("Plugin directory may not be a symbolic link.")
    created_directory = not requested.exists()
    try:
        requested.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise LauncherError(f"Could not create plugin directory: {exc}") from exc
    if not requested.is_dir():
        raise LauncherError("Plugin path must be a directory.")
    root = requested.resolve(strict=True)

    plugin_id = _plugin_id_from_directory(requested.name or root.name)
    manifest = {
        "id": plugin_id,
        "name": "Sample Plugin",
        "version": "0.1.0",
        "api_version": "3",
        "author": "Plugin author",
        "description": "A Nivora Launcher plugin.",
        "entrypoint": "main.js",
        "permissions": [],
        "sandbox_profile": "strict",
    }
    _validate_author_manifest(manifest)
    starter_files = {
        PLUGIN_MANIFEST: _json_bytes(manifest),
        "main.js": (
            '"use strict";\n\n'
            "const plugin = {\n"
            "  async activate(api) {\n"
            "    const runtime = await api.runtime.getInfo();\n"
            "    void runtime;\n"
            "  },\n"
            "};\n\n"
            "export default plugin;\n"
        ).encode("utf-8"),
        "README.md": (
            f"# {manifest['name']}\n\n"
            "A Nivora Launcher v3 plugin starter.\n\n"
            "Edit `plugin.json` for metadata and permissions, then implement the plugin in `main.js`.\n"
            "Use the host-provided API only; do not rely on Node.js or browser globals.\n\n"
            "Run `trial-launcher plugin lint .` before packaging.\n"
        ).encode("utf-8"),
    }

    targets = [root / name for name in starter_files]
    if any(path.is_symlink() or path.exists() for path in targets):
        raise LauncherError("Plugin starter files already exist; init will not overwrite files.")

    created_files: list[Path] = []
    try:
        for name, content in starter_files.items():
            path = root / name
            with path.open("xb") as handle:
                created_files.append(path)
                handle.write(content)
    except OSError as exc:
        for path in created_files:
            path.unlink(missing_ok=True)
        if created_directory:
            try:
                root.rmdir()
            except OSError:
                pass
        raise LauncherError(f"Could not write plugin starter files: {exc}") from exc
    return root


def lint_plugin(directory: str | Path) -> list[str]:
    _root, _manifest, _validated, files = _load_project(directory)
    return _lint_sources(files)


def pack_plugin(
    directory: str | Path,
    output: str | Path,
    *,
    key: str | Path | None = None,
) -> Path:
    root, manifest, _validated, source_files = _load_project(directory)
    _lint_sources(source_files)

    output_path = Path(output).expanduser()
    if output_path.is_symlink():
        raise LauncherError("Plugin archive output may not be a symbolic link.")
    if not output_path.name or output_path.name in {".", ".."}:
        raise LauncherError("Plugin archive output must be a file path.")
    if not output_path.parent.is_dir():
        raise LauncherError("Plugin archive output directory does not exist.")
    output_path = output_path.parent.resolve(strict=True) / output_path.name
    if output_path.exists() and not output_path.is_file():
        raise LauncherError("Plugin archive output must be a regular file.")
    if _is_within(output_path.resolve(strict=False), root):
        raise LauncherError("Plugin archive output must be outside the plugin directory.")

    file_bytes = {name: content for name, content in source_files.items() if name != PLUGIN_MANIFEST}
    package_manifest = copy.deepcopy(manifest)
    package_manifest.pop("signature", None)
    package_manifest["integrity"] = _integrity_value(file_bytes)

    if key is not None:
        private_key_pem = _read_private_key(key)
        if Path(key).expanduser().resolve(strict=True) == output_path:
            raise LauncherError("Plugin archive output must not overwrite the signing key.")
        sign_manifest, verify_manifest = _plugin_security()
        candidate = copy.deepcopy(package_manifest)
        try:
            signed = sign_manifest(candidate, file_bytes, private_key_pem)
        except ModuleNotFoundError as exc:
            if exc.name == "cryptography":
                raise LauncherError("Signing requires the cryptography package for Ed25519 support.") from exc
            raise LauncherError("The plugin signing provider is unavailable.") from exc
        except Exception as exc:
            if isinstance(exc, LauncherError):
                raise
            raise LauncherError("Could not sign the plugin manifest; check the private key and crypto support.") from None
        if signed is None:
            signed = candidate
        if not isinstance(signed, dict):
            raise LauncherError("The plugin signing provider returned an invalid manifest.")
        package_manifest = signed
        _validate_integrity(package_manifest, file_bytes)
        _validate_signature(package_manifest.get("signature"))
        _verify_signature(verify_manifest, package_manifest, file_bytes)

    packaged_files = dict(file_bytes)
    packaged_files[PLUGIN_MANIFEST] = _json_bytes(package_manifest)
    if len(packaged_files) > PLUGIN_MAX_FILES:
        raise LauncherError("Plugin package contains too many files.")

    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{output_path.name}.", suffix=".tmp", dir=output_path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w+b") as sink:
            with zipfile.ZipFile(sink, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
                for name, content in sorted(packaged_files.items()):
                    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                    info.create_system = 3
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = (stat.S_IFREG | 0o644) << 16
                    info.extra = b""
                    info.comment = b""
                    archive.writestr(info, content, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
            sink.flush()
            os.fsync(sink.fileno())
        if temporary.stat().st_size > PLUGIN_MAX_ARCHIVE_BYTES:
            raise LauncherError("Plugin archive exceeds the compressed size limit.")
        os.replace(temporary, output_path)
    except LauncherError:
        raise
    except (OSError, zipfile.BadZipFile) as exc:
        raise LauncherError(f"Could not create plugin archive: {exc}") from exc
    finally:
        temporary.unlink(missing_ok=True)
    return output_path


def verify_plugin(archive_path: str | Path) -> dict[str, Any]:
    path = Path(archive_path).expanduser()
    if path.is_symlink() or not path.is_file():
        raise LauncherError("Plugin archive must be a regular file, not a symbolic link.")
    try:
        with path.open("rb") as handle:
            content = handle.read(PLUGIN_MAX_ARCHIVE_BYTES + 1)
    except OSError as exc:
        raise LauncherError(f"Could not read plugin archive: {exc}") from exc
    if not content or len(content) > PLUGIN_MAX_ARCHIVE_BYTES:
        raise LauncherError("Plugin archive is empty or exceeds the compressed size limit.")

    manifest, validated, file_bytes = _read_archive(content)
    _validate_integrity(manifest, file_bytes)
    signed = "signature" in manifest
    if signed:
        _validate_signature(manifest.get("signature"))
        _sign, verify_manifest = _plugin_security()
        _verify_signature(verify_manifest, manifest, file_bytes)
    return {
        "id": validated.get("id", manifest.get("id", "")),
        "name": validated.get("name", manifest.get("name", "")),
        "version": validated.get("version", manifest.get("version", "")),
        "signed": signed,
    }


def generate_keypair(private_path: str | Path, public_path: str | Path) -> tuple[Path, Path]:
    private_file, public_file = _key_output_paths(private_path, public_path)
    try:
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    except ImportError as exc:
        raise LauncherError("Key generation requires the cryptography package for Ed25519 support.") from exc

    private_key = Ed25519PrivateKey.generate()
    private_pem = private_key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.PKCS8,
        encryption_algorithm=serialization.NoEncryption(),
    )
    public_pem = private_key.public_key().public_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PublicFormat.SubjectPublicKeyInfo,
    )
    written: list[Path] = []
    try:
        _write_new_file(private_file, private_pem, 0o600)
        written.append(private_file)
        _write_new_file(public_file, public_pem, 0o644)
        written.append(public_file)
    except OSError as exc:
        for path in written:
            path.unlink(missing_ok=True)
        raise LauncherError(f"Could not write Ed25519 key files: {exc}") from exc
    return private_file, public_file


def _load_project(
    directory: str | Path,
) -> tuple[Path, dict[str, Any], dict[str, Any], dict[str, bytes]]:
    root = _project_root(directory)
    files = _collect_project_files(root)
    manifest_bytes = files.get(PLUGIN_MANIFEST)
    if manifest_bytes is None:
        raise LauncherError("Plugin directory must contain a root plugin.json manifest.")
    if len(manifest_bytes) > PLUGIN_MAX_MANIFEST_BYTES:
        raise LauncherError("plugin.json exceeds the manifest size limit.")
    try:
        manifest = json.loads(manifest_bytes.decode("utf-8"), object_pairs_hook=_unique_json_object)
    except (UnicodeError, json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise LauncherError(f"plugin.json is not valid UTF-8 JSON: {exc}") from exc
    validated = _validate_author_manifest(manifest)
    entrypoint = validated.get("entrypoint", manifest.get("entrypoint"))
    if not isinstance(entrypoint, str):
        raise LauncherError("plugin.json must name a JavaScript entrypoint.")
    entrypoint = _normalize_archive_path(entrypoint)
    if entrypoint not in files:
        raise LauncherError("The plugin entrypoint is missing or is not a regular file.")
    return root, manifest, validated, files


def _project_root(directory: str | Path) -> Path:
    path = Path(directory).expanduser()
    if path.is_symlink():
        raise LauncherError("Plugin directory may not be a symbolic link.")
    if not path.is_dir():
        raise LauncherError("Plugin directory does not exist or is not a directory.")
    try:
        return path.resolve(strict=True)
    except OSError as exc:
        raise LauncherError(f"Could not resolve plugin directory: {exc}") from exc


def _collect_project_files(root: Path) -> dict[str, bytes]:
    result: dict[str, bytes] = {}
    total_size = 0

    def walk_error(error: OSError) -> None:
        raise LauncherError(f"Could not inspect plugin directory: {error}") from error

    try:
        for current, directories, filenames in os.walk(root, topdown=True, followlinks=False, onerror=walk_error):
            current_path = Path(current)
            kept_directories: list[str] = []
            for name in sorted(directories):
                if name in _EXCLUDED_DIRECTORIES:
                    continue
                child = current_path / name
                mode = child.lstat().st_mode
                if stat.S_ISLNK(mode):
                    raise LauncherError(f"Plugin package contains a symbolic link: {child.relative_to(root)}")
                if not stat.S_ISDIR(mode):
                    raise LauncherError(f"Plugin package contains a non-directory path: {child.relative_to(root)}")
                _normalize_archive_path(child.relative_to(root).as_posix())
                kept_directories.append(name)
            directories[:] = kept_directories

            for name in sorted(filenames):
                if name in _EXCLUDED_DIRECTORIES:
                    continue
                path = current_path / name
                relative = _normalize_archive_path(path.relative_to(root).as_posix())
                mode = path.lstat().st_mode
                if stat.S_ISLNK(mode):
                    raise LauncherError(f"Plugin package contains a symbolic link: {relative}")
                if not stat.S_ISREG(mode):
                    raise LauncherError(f"Plugin package contains a non-regular file: {relative}")
                remaining = PLUGIN_MAX_UNPACKED_BYTES - total_size
                per_file_limit = PLUGIN_MAX_MANIFEST_BYTES if relative == PLUGIN_MANIFEST else PLUGIN_MAX_UNPACKED_BYTES
                file_limit = min(remaining, per_file_limit)
                size_error = (
                    "plugin.json exceeds the manifest size limit."
                    if relative == PLUGIN_MANIFEST
                    else "Plugin package exceeds the unpacked size limit."
                )
                if path.lstat().st_size > file_limit:
                    raise LauncherError(size_error)
                flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
                descriptor = os.open(path, flags)
                with os.fdopen(descriptor, "rb") as handle:
                    opened_stat = os.fstat(handle.fileno())
                    if not stat.S_ISREG(opened_stat.st_mode):
                        raise LauncherError(f"Plugin package contains a non-regular file: {relative}")
                    if opened_stat.st_size > file_limit:
                        raise LauncherError(size_error)
                    content = handle.read(file_limit + 1)
                if len(content) > file_limit or len(content) != opened_stat.st_size:
                    raise LauncherError(size_error)
                result[relative] = content
                total_size += len(content)
                if len(result) > PLUGIN_MAX_FILES:
                    raise LauncherError("Plugin package contains too many files.")
    except LauncherError:
        raise
    except OSError as exc:
        raise LauncherError(f"Could not read plugin package files: {exc}") from exc
    return result


def _validate_author_manifest(manifest: Any) -> dict[str, Any]:
    if not isinstance(manifest, dict):
        raise LauncherError("plugin.json must contain a JSON object.")
    candidate = dict(manifest)
    candidate.setdefault("integrity", {"files": {}})
    try:
        validated = validate_plugin_json_manifest(candidate)
    except LauncherError:
        raise
    except (TypeError, ValueError, KeyError) as exc:
        raise LauncherError(f"plugin.json manifest validation failed: {exc}") from exc
    if not isinstance(validated, dict):
        raise LauncherError("Plugin manifest validator returned an invalid result.")
    return validated


def _lint_sources(files: dict[str, bytes]) -> list[str]:
    warnings: list[str] = []
    for name, content in sorted(files.items()):
        if Path(name).suffix.lower() not in {".js", ".mjs", ".cjs"}:
            continue
        try:
            source = content.decode("utf-8")
        except UnicodeError as exc:
            raise LauncherError(f"JavaScript source is not UTF-8: {name}") from exc
        tokens = _javascript_tokens(source)
        import_line = _unsupported_import_line(tokens)
        if import_line is not None:
            raise LauncherError(
                f"Unsupported static ESM/CommonJS import in {name}:{import_line}; "
                "bundle dependencies into a single file and remove imports."
            )
        identifiers = {value for kind, value, _line in tokens if kind == "identifier"}
        network = sorted(identifiers & _NETWORK_APIS)
        filesystem = sorted(identifiers & _FILESYSTEM_APIS)
        if network:
            warnings.append(
                f"{name}: direct network API(s) detected ({', '.join(network)}); "
                "prefer permission-scoped network APIs. This check is advisory; QuickJS is the security boundary."
            )
        if filesystem:
            warnings.append(
                f"{name}: direct filesystem API(s) detected ({', '.join(filesystem)}); "
                "prefer the plugin files API. This check is advisory; QuickJS is the security boundary."
            )
    return warnings


def _javascript_tokens(source: str) -> list[tuple[str, str, int]]:
    tokens: list[tuple[str, str, int]] = []
    index = 0
    line = 1
    while index < len(source):
        char = source[index]
        if char.isspace():
            if char == "\n":
                line += 1
            index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2)
            if newline < 0:
                break
            index = newline
            continue
        if source.startswith("/*", index):
            end = source.find("*/", index + 2)
            if end < 0:
                raise LauncherError("JavaScript source contains an unterminated block comment.")
            line += source.count("\n", index, end + 2)
            index = end + 2
            continue
        if char in {"'", '"'}:
            quote = char
            start_line = line
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    if index + 1 < len(source) and source[index + 1] == "\n":
                        line += 1
                    index += 2
                    continue
                if source[index] == quote:
                    index += 1
                    break
                if source[index] == "\n":
                    line += 1
                index += 1
            else:
                raise LauncherError("JavaScript source contains an unterminated string.")
            tokens.append(("string", "", start_line))
            continue
        if char == "`":
            start_line = line
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    if index + 1 < len(source) and source[index + 1] == "\n":
                        line += 1
                    index += 2
                    continue
                if source[index] == "`":
                    index += 1
                    break
                if source[index] == "\n":
                    line += 1
                index += 1
            else:
                raise LauncherError("JavaScript source contains an unterminated template string.")
            tokens.append(("template", "", start_line))
            continue
        if char == "/" and _regex_can_start(tokens):
            start_line = line
            index += 1
            in_character_class = False
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                    continue
                if source[index] == "[":
                    in_character_class = True
                elif source[index] == "]":
                    in_character_class = False
                elif source[index] == "/" and not in_character_class:
                    index += 1
                    while index < len(source) and source[index].isalpha():
                        index += 1
                    break
                elif source[index] == "\n":
                    break
                index += 1
            tokens.append(("regex", "", start_line))
            continue
        if char.isalpha() or char in {"_", "$"}:
            start = index
            index += 1
            while index < len(source) and (source[index].isalnum() or source[index] in {"_", "$"}):
                index += 1
            tokens.append(("identifier", source[start:index], line))
            continue
        tokens.append(("punctuation", char, line))
        index += 1
    return tokens


def _regex_can_start(tokens: list[tuple[str, str, int]]) -> bool:
    if not tokens:
        return True
    _kind, previous, _line = tokens[-1]
    return previous in {"=", "(", "[", "{", ",", ":", ";", "!", "?", "&", "|", "return", "case", "throw", "=>"}


def _unsupported_import_line(tokens: list[tuple[str, str, int]]) -> int | None:
    for index, (kind, value, line) in enumerate(tokens):
        if kind != "identifier":
            continue
        previous = tokens[index - 1][1] if index else ""
        following = tokens[index + 1][1] if index + 1 < len(tokens) else ""
        if value == "import" and previous != ".":
            if following == "(" or (
                following == "."
                and index + 2 < len(tokens)
                and tokens[index + 2][1] == "meta"
            ):
                continue
            return line
        if value == "require" and previous != "." and following == "(":
            if index + 2 < len(tokens) and tokens[index + 2][0] == "string":
                return line
        if value == "export":
            cursor = index + 1
            while cursor + 1 < len(tokens) and tokens[cursor][1] != ";":
                if tokens[cursor][0] == "identifier" and tokens[cursor][1] == "from":
                    if tokens[cursor + 1][0] == "string":
                        return line
                cursor += 1
    return None


def _read_archive(content: bytes) -> tuple[dict[str, Any], dict[str, Any], dict[str, bytes]]:
    try:
        with zipfile.ZipFile(io.BytesIO(content), "r") as archive:
            members = archive.infolist()
            if not members or len(members) > PLUGIN_MAX_FILES:
                raise LauncherError("Plugin archive has no files or contains too many entries.")
            seen: set[str] = set()
            file_bytes: dict[str, bytes] = {}
            total_size = 0
            for member in members:
                is_directory = member.is_dir()
                name = _normalize_archive_path(member.filename, is_directory=is_directory)
                if name in seen:
                    raise LauncherError("Plugin archive contains duplicate paths.")
                seen.add(name)
                mode = (member.external_attr >> 16) & 0o170000
                if mode == stat.S_IFLNK:
                    raise LauncherError("Plugin archive contains a symbolic link.")
                if is_directory:
                    if mode not in {0, stat.S_IFDIR}:
                        raise LauncherError("Plugin archive contains a non-directory entry.")
                    continue
                if mode not in {0, stat.S_IFREG}:
                    raise LauncherError("Plugin archive contains a non-regular file.")
                if member.flag_bits & 0x1:
                    raise LauncherError("Encrypted plugin archive entries are not allowed.")
                remaining = PLUGIN_MAX_UNPACKED_BYTES - total_size
                per_file_limit = PLUGIN_MAX_MANIFEST_BYTES if name == PLUGIN_MANIFEST else PLUGIN_MAX_UNPACKED_BYTES
                if member.file_size < 0 or member.file_size > min(remaining, per_file_limit):
                    raise LauncherError("Plugin archive exceeds the unpacked size limit.")
                with archive.open(member, "r") as source:
                    data = source.read(min(remaining, per_file_limit) + 1)
                if len(data) > min(remaining, per_file_limit) or len(data) != member.file_size:
                    raise LauncherError("Plugin archive expands beyond the declared size limit.")
                total_size += len(data)
                file_bytes[name] = data

            for name in file_bytes:
                parts = name.split("/")
                if any("/".join(parts[:index]) in file_bytes for index in range(1, len(parts))):
                    raise LauncherError("Plugin archive contains a file path used as a directory.")
            manifest_bytes = file_bytes.pop(PLUGIN_MANIFEST, None)
            if manifest_bytes is None:
                raise LauncherError("Plugin archive must contain a root plugin.json manifest.")
            manifest = _parse_manifest_bytes(manifest_bytes)
            validated = _validate_author_manifest(manifest)
            entrypoint = validated.get("entrypoint", manifest.get("entrypoint"))
            if not isinstance(entrypoint, str) or _normalize_archive_path(entrypoint) not in file_bytes:
                raise LauncherError("Plugin archive does not contain the manifest entrypoint.")
            return manifest, validated, file_bytes
    except LauncherError:
        raise
    except Exception as exc:
        raise LauncherError("Plugin archive is invalid or contains unreadable files.") from exc


def _parse_manifest_bytes(content: bytes) -> dict[str, Any]:
    if len(content) > PLUGIN_MAX_MANIFEST_BYTES:
        raise LauncherError("plugin.json exceeds the manifest size limit.")
    try:
        manifest = json.loads(content.decode("utf-8"), object_pairs_hook=_unique_json_object)
    except (UnicodeError, json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise LauncherError(f"plugin.json is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(manifest, dict):
        raise LauncherError("plugin.json must contain a JSON object.")
    return manifest


def _validate_integrity(manifest: dict[str, Any], file_bytes: dict[str, bytes]) -> None:
    expected = _integrity_value(file_bytes)["files"]
    integrity = manifest.get("integrity")
    if not isinstance(integrity, dict) or set(integrity) != {"files"}:
        raise LauncherError("Plugin manifest integrity metadata is missing or malformed.")
    actual = integrity.get("files")
    if not isinstance(actual, dict) or set(actual) != set(expected):
        raise LauncherError("Plugin integrity hashes must cover every package file except plugin.json.")
    for name, digest in expected.items():
        supplied = actual.get(name)
        if not isinstance(supplied, str) or not re.fullmatch(r"[0-9a-f]{64}", supplied):
            raise LauncherError(f"Plugin integrity hash is invalid for {name}.")
        if not hmac.compare_digest(supplied, digest):
            raise LauncherError(f"Plugin integrity check failed for {name}.")


def _integrity_value(file_bytes: dict[str, bytes]) -> dict[str, dict[str, str]]:
    return {
        "files": {
            name: hashlib.sha256(content).hexdigest()
            for name, content in sorted(file_bytes.items())
        }
    }


def _validate_signature(value: Any) -> None:
    if not isinstance(value, dict) or set(value) != _SIGNATURE_KEYS:
        raise LauncherError("Plugin signature metadata is missing or malformed.")
    if value.get("algorithm") != "ed25519":
        raise LauncherError("Plugin signature algorithm must be ed25519.")
    key_id = value.get("key_id")
    if not isinstance(key_id, str) or not key_id or len(key_id) > 128 or not key_id.isprintable():
        raise LauncherError("Plugin signature key_id is invalid.")
    for field, expected_length in (("public_key", 32), ("value", 64)):
        encoded = value.get(field)
        if not isinstance(encoded, str):
            raise LauncherError(f"Plugin signature {field} must be base64 text.")
        try:
            decoded = base64.b64decode(encoded, validate=True)
        except (ValueError, binascii.Error) as exc:
            raise LauncherError(f"Plugin signature {field} is not valid base64.") from exc
        if len(decoded) != expected_length:
            raise LauncherError(f"Plugin signature {field} has an invalid length.")


def _plugin_security():
    try:
        from .plugin_security import sign_manifest, verify_manifest
    except ModuleNotFoundError as exc:
        if exc.name == "cryptography":
            raise LauncherError("Plugin signatures require the cryptography package for Ed25519 support.") from exc
        raise LauncherError("The shared nivora_launcher.plugin_security module is not available.") from exc
    except ImportError as exc:
        raise LauncherError("The shared plugin signing and verification functions are not available.") from exc
    return sign_manifest, verify_manifest


def _verify_signature(verify_manifest, manifest: dict[str, Any], file_bytes: dict[str, bytes]) -> None:
    try:
        verified = verify_manifest(manifest, file_bytes)
    except Exception:
        raise LauncherError("Plugin signature verification failed.") from None
    if not isinstance(verified, dict) or verified.get("signed") is not True:
        error = verified.get("error") if isinstance(verified, dict) else None
        raise LauncherError(str(error or "Plugin signature verifier returned an invalid result."))
    if verified.get("valid") is not True:
        error = verified.get("error")
        if isinstance(error, str) and error:
            raise LauncherError(f"Plugin signature verification failed: {error}")
        raise LauncherError("Plugin signature verification failed.")


def _read_private_key(path: str | Path) -> bytes:
    key_path = Path(path).expanduser()
    if key_path.is_symlink() or not key_path.is_file():
        raise LauncherError("Private key must be a regular PEM file, not a symbolic link.")
    try:
        with key_path.open("rb") as handle:
            content = handle.read(1024 * 1024 + 1)
    except OSError as exc:
        raise LauncherError(f"Could not read private key: {exc}") from exc
    if not content or len(content) > 1024 * 1024:
        raise LauncherError("Private key file is empty or too large.")
    return content


def _key_output_paths(private_path: str | Path, public_path: str | Path) -> tuple[Path, Path]:
    paths = [Path(private_path).expanduser(), Path(public_path).expanduser()]
    if paths[0].resolve(strict=False) == paths[1].resolve(strict=False):
        raise LauncherError("Private and public key paths must be different.")
    normalized: list[Path] = []
    for path in paths:
        if path.is_symlink() or path.exists():
            raise LauncherError(f"Key output already exists or is a symbolic link: {path}")
        if not path.parent.is_dir():
            raise LauncherError(f"Key output directory does not exist: {path.parent}")
        normalized.append(path.parent.resolve(strict=True) / path.name)
    if normalized[0] == normalized[1]:
        raise LauncherError("Private and public key paths must be different.")
    return normalized[0], normalized[1]


def _write_new_file(path: Path, content: bytes, mode: int) -> None:
    descriptor: int | None = None
    created = False
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
        created = True
        with os.fdopen(descriptor, "wb") as handle:
            descriptor = None
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(path, mode)
    except OSError:
        if descriptor is not None:
            os.close(descriptor)
        if created:
            path.unlink(missing_ok=True)
        raise


def _plugin_id_from_directory(name: str) -> str:
    plugin_id = re.sub(r"[^a-z0-9._-]+", "-", name.lower()).strip("._-")
    if not plugin_id or not re.match(r"^[a-z]", plugin_id):
        plugin_id = f"plugin-{plugin_id}"
    if len(plugin_id) < 2:
        plugin_id = f"{plugin_id}-plugin"
    return plugin_id[:64]


def _normalize_archive_path(name: str, *, is_directory: bool = False) -> str:
    if not isinstance(name, str) or not name or "\x00" in name or "\\" in name:
        raise LauncherError("Plugin package contains an invalid path.")
    normalized = name[:-1] if is_directory and name.endswith("/") else name
    if normalized.startswith("/") or re.match(r"^[A-Za-z]:", normalized):
        raise LauncherError("Plugin package contains an absolute path.")
    parts = normalized.split("/")
    if any(part in {"", ".", ".."} or ":" in part for part in parts):
        raise LauncherError("Plugin package contains an unsafe path.")
    if any(any(ord(char) < 32 or ord(char) == 127 for char in part) for part in parts):
        raise LauncherError("Plugin package contains a path with control characters.")
    if len(normalized.encode("utf-8")) > 1024 or any(len(part.encode("utf-8")) > 255 for part in parts):
        raise LauncherError("Plugin package contains an excessively long path.")
    return normalized


def _unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _json_bytes(value: dict[str, Any]) -> bytes:
    try:
        return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    except (TypeError, ValueError, RecursionError) as exc:
        raise LauncherError("Plugin manifest contains values that cannot be encoded as JSON.") from exc


def _is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False
