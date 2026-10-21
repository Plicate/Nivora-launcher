from __future__ import annotations

import json
import os
import platform
import re
import shutil
import stat
import subprocess
import time
from pathlib import Path
from typing import Any

from .config import ConfigStore, _atomic_json_write
from .constants import JAVA_RUNTIME_MANIFEST_URL
from .errors import LauncherError
from .net import HttpClient
from .platform_support import is_reparse_point, set_private_fd
from .portable_fs import regular_file, safe_path

class JavaRuntimeManager:
    def __init__(self, config: ConfigStore, http: HttpClient | None = None) -> None:
        self.config = config
        self.http = http or HttpClient()

    def java_for(self, version_json: dict[str, Any], java_path: str | None = None) -> Path:
        required_major, component = _required_java(version_json)
        configured = (java_path if java_path is not None else self.config.settings.java_path).strip()
        if configured:
            path = validate_java_path(configured, allow_directory=True)
            actual = java_version(path)
            if required_major and actual and actual < required_major:
                raise LauncherError(
                    f"Настроенная Java {actual} слишком старая для этой версии; требуется Java {required_major}"
                )
            return path

        system = shutil.which("java")
        system_path: Path | None = None
        system_major: int | None = None
        if system:
            system_path = validate_java_path(system)
            system_major = java_version(system_path)
            if required_major is None or system_major == required_major:
                return system_path

        runtime_error: LauncherError | None = None
        if component:
            try:
                bundled = self._ensure_mojang_runtime(str(component))
                if bundled:
                    return bundled
            except LauncherError as exc:
                runtime_error = exc
        if system_path:
            if required_major and system_major and system_major < required_major:
                raise LauncherError(f"Java {system_major} слишком старая; требуется Java {required_major}")
            return system_path
        if runtime_error:
            raise runtime_error
        raise LauncherError("Java не найден. Установите jre-openjdk или разрешите лаунчеру скачать runtime Mojang.")

    def _ensure_mojang_runtime(self, component: str) -> Path | None:
        _safe_runtime_component(component, "runtime component")
        platform_key = _runtime_platform()
        all_manifest = self._runtime_manifest()
        platform_data = all_manifest.get(platform_key, {})
        candidates = platform_data.get(component, []) if isinstance(platform_data, dict) else []
        if not candidates:
            return None
        entry = candidates[0]
        if not isinstance(entry, dict):
            raise LauncherError(f"Java runtime manifest имеет некорректный entry для {component}")
        runtime_version = str(entry.get("version", {}).get("name", "unknown"))
        _safe_runtime_component(runtime_version, "runtime version")
        destination = self.config.paths.runtimes_dir / component / runtime_version
        _ensure_runtime_path(destination, self.config.paths.runtimes_dir)
        destination.mkdir(parents=True, exist_ok=True)
        executable_name = "java.exe" if os.name == "nt" else "java"
        java_path = destination / "bin" / executable_name
        marker = destination / ".trial-runtime-complete"
        if (
            not is_reparse_point(marker)
            and marker.exists()
            and java_path.exists()
            and _within(java_path.resolve(), destination.resolve())
            and (os.name == "nt" or os.access(java_path, os.X_OK))
        ):
            return validate_java_path(java_path)
        manifest_url = entry.get("manifest", {}).get("url")
        if not manifest_url:
            return None
        manifest = self.http.get_json(str(manifest_url))
        files = manifest.get("files", manifest)
        if not isinstance(files, dict):
            raise LauncherError(f"Неожиданный Java runtime manifest для {component}")
        processed_files = 0
        pending_links: list[tuple[Path, Path]] = []
        for relative, metadata in files.items():
            relative_path = Path(str(relative).replace("\\", "/"))
            if relative_path.is_absolute() or ".." in relative_path.parts or not relative_path.parts:
                raise LauncherError(f"Java runtime manifest содержит небезопасный путь: {relative}")
            target = safe_path(destination, tuple(relative_path.parts))
            if not isinstance(metadata, dict):
                continue
            file_type = metadata.get("type", "file")
            if file_type not in {"file", "directory", "link"}:
                raise LauncherError(f"Java runtime manifest содержит неизвестный тип файла: {relative}")
            if file_type == "directory":
                _ensure_runtime_path(target, destination)
                target.mkdir(parents=True, exist_ok=True)
                processed_files += 1
                continue
            if file_type == "link":
                # runtime manifests can contain links; only allow a relative target inside the runtime.
                link_target = Path(str(metadata.get("target", "")).replace("\\", "/"))
                resolved_target = (target.parent / link_target).resolve()
                if link_target.is_absolute() or not _within(resolved_target, destination.resolve()):
                    raise LauncherError(f"Небезопасная ссылка в Java runtime: {relative}")
                _ensure_runtime_path(target.parent, destination)
                target.parent.mkdir(parents=True, exist_ok=True)
                if is_reparse_point(target):
                    raise LauncherError(f"Ссылка в Java runtime является reparse point: {relative}")
                if target.exists():
                    target.unlink()
                if os.name == "nt":
                    pending_links.append((target, resolved_target))
                else:
                    target.symlink_to(link_target)
                    processed_files += 1
                continue
            download = metadata.get("downloads", {}).get("raw", metadata.get("downloads", {}))
            url = download.get("url") if isinstance(download, dict) else None
            if not url:
                raise LauncherError(f"Java runtime manifest не содержит URL: {relative}")
            _ensure_runtime_path(target.parent, destination)
            self.http.download(
                str(url),
                target,
                sha1=download.get("sha1"),
                size=download.get("size"),
            )
            if os.name != "nt" and (metadata.get("executable") or relative_path.name == "java"):
                target.chmod(target.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
            processed_files += 1
        if pending_links:
            remaining = pending_links
            while remaining:
                deferred: list[tuple[Path, Path]] = []
                progress = False
                for target, source in remaining:
                    if not source.is_file() or is_reparse_point(source):
                        deferred.append((target, source))
                        continue
                    _ensure_runtime_path(target.parent, destination)
                    regular_file(source)
                    temporary = target.with_name(f".{target.name}.{uuid.uuid4().hex}.part")
                    try:
                        shutil.copyfile(source, temporary)
                        safe_path(destination, tuple(target.relative_to(destination.resolve()).parts))
                        temporary.replace(target)
                    finally:
                        temporary.unlink(missing_ok=True)
                    processed_files += 1
                    progress = True
                if not progress:
                    unresolved = ", ".join(str(target.relative_to(destination.resolve())) for target, _ in deferred[:3])
                    raise LauncherError(f"Java runtime manifest содержит неразрешимые ссылки: {unresolved}")
                remaining = deferred
        if (
            java_path.exists()
            and _within(java_path.resolve(), destination.resolve())
            and processed_files == len(files)
        ):
            if is_reparse_point(marker) or not _runtime_path_without_symlink(marker.parent, destination):
                raise LauncherError("Файл состояния Java runtime является символьной ссылкой")
            fd = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o600)
            set_private_fd(fd)
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                handle.write("complete\n")
            return validate_java_path(java_path)
        return None

    def _runtime_manifest(self) -> dict[str, Any]:
        cache_path = self.config.paths.download_cache / "java-runtime-manifest.json"
        cached: dict[str, Any] | None = None
        try:
            if not cache_path.is_symlink() and cache_path.stat().st_size <= 8 * 1024 * 1024:
                value = json.loads(cache_path.read_text(encoding="utf-8"))
                if isinstance(value, dict) and isinstance(value.get("manifest"), dict):
                    cached = value
                    if time.time() - float(value.get("fetched_at", 0)) < 6 * 60 * 60:
                        return value["manifest"]
        except (OSError, ValueError, TypeError):
            cached = None
        try:
            manifest = self.http.get_json(JAVA_RUNTIME_MANIFEST_URL)
        except LauncherError:
            if cached:
                return cached["manifest"]
            raise
        if not isinstance(manifest, dict):
            raise LauncherError("Java runtime manifest имеет неожиданный формат")
        _atomic_json_write(cache_path, {"fetched_at": time.time(), "manifest": manifest}, private=True)
        return manifest

def discover_java_paths() -> list[Path]:
    candidates: list[Path] = []
    java_home = os.environ.get("JAVA_HOME", "").strip()
    if java_home:
        candidates.append(Path(java_home).expanduser() / "bin" / ("java.exe" if os.name == "nt" else "java"))
    system_java = shutil.which("java.exe" if os.name == "nt" else "java")
    if system_java:
        candidates.append(Path(system_java))
    if os.name == "nt":
        roots = [
            Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Java",
            Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Eclipse Adoptium",
            Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local")) / "Programs" / "Eclipse Adoptium",
        ]
        binary_name = "java.exe"
    else:
        roots = [Path("/usr/lib/jvm"), Path("/opt")]
        binary_name = "java"
    for root in roots:
        if not root.is_dir():
            continue
        try:
            candidates.extend(path for path in root.glob(f"*/bin/{binary_name}") if path.is_file())
        except OSError:
            continue
    result: list[Path] = []
    seen: set[str] = set()
    for path in candidates:
        try:
            resolved = path.expanduser().resolve()
        except OSError:
            continue
        key = str(resolved)
        if key in seen or not resolved.is_file() or (os.name != "nt" and not os.access(resolved, os.X_OK)):
            continue
        try:
            resolved = validate_java_path(resolved)
        except LauncherError:
            continue
        seen.add(key)
        result.append(resolved)
    return result

def _runtime_platform() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()
    if system == "windows":
        if machine in {"x86_64", "amd64"}:
            return "windows-x64"
        if machine in {"arm64", "aarch64"}:
            return "windows-arm64"
        raise LauncherError(f"Для Windows поддерживается x86_64 или ARM64, обнаружено: {machine}")
    if system == "darwin":
        if machine in {"arm64", "aarch64"}:
            return "macos-arm64"
        if machine in {"x86_64", "amd64"}:
            return "macos"
        raise LauncherError(f"Для macOS поддерживается x86_64 или ARM64, обнаружено: {machine}")
    if system == "linux":
        if machine in {"x86_64", "amd64"}:
            return "linux"
        if machine in {"arm64", "aarch64"}:
            return "linux-arm64"
        raise LauncherError(f"Для Linux поддерживается x86_64 или ARM64, обнаружено: {machine}")
    raise LauncherError(f"Nivora Launcher не поддерживает платформу: {platform.system()}")

def _legacy_version_id(version_id: str) -> bool:
    match = re.search(r"1\.(\d+)(?:[.\-]|$)", version_id)
    return bool(match and int(match.group(1)) < 13)

def _required_java(version_json: dict[str, Any]) -> tuple[int | None, str | None]:
    """какую Java требует версия: major и runtime-компонент Mojang."""
    block = version_json.get("javaVersion")
    block = block if isinstance(block, dict) else {}
    major = block.get("majorVersion")
    major = int(major) if major else None
    component = str(block.get("component") or "") or None
    if major is None and _legacy_version_id(str(version_json.get("id") or "")):
        return 8, component or "jre-legacy"
    return major, component

def _within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False

def _safe_runtime_component(value: str, label: str) -> None:
    if not value or not re.fullmatch(r"[A-Za-z0-9._+-]+", value):
        raise LauncherError(f"Небезопасный {label}: {value}")

def _runtime_path_without_symlink(path: Path, root: Path) -> bool:
    current = path
    while True:
        if is_reparse_point(current):
            return False
        if current == root:
            return True
        if current == current.parent or not _within(current, root):
            return False
        current = current.parent

def _ensure_runtime_path(path: Path, root: Path) -> None:
    if not _runtime_path_without_symlink(path, root):
        raise LauncherError(f"Небезопасный путь Java runtime: {path}")

def _java_version_output(java_path: Path) -> str:
    """Вывод `java -version`; пустая строка, если бинарник не ответил."""
    executable = java_path
    if os.name == "nt" and java_path.name.lower() == "javaw.exe":
        console_java = java_path.with_name("java.exe")
        if console_java.is_file():
            executable = console_java
    try:
        result = subprocess.run(
            [str(executable), "-version"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return (result.stderr or "") + (result.stdout or "")

def java_probe(java_path: Path) -> tuple[int | None, int | None]:
    """(major, разрядность) одним запуском `java -version`."""
    output = _java_version_output(java_path)
    if not output:
        return None, None
    major: int | None = None
    match = re.search(r'version\s+"([^"]+)', output)
    if match:
        value = match.group(1)
        if value.startswith("1."):
            value = value.split(".", 2)[1]
        try:
            major = int(value.split(".", 1)[0])
        except ValueError:
            major = None
    bitness: int | None = None
    if "64-Bit" in output or "64-bit" in output:
        bitness = 64
    elif "VM" in output or "version" in output:
        bitness = 32
    return major, bitness

def java_version(java_path: Path) -> int | None:
    major, _ = java_probe(java_path)
    return major

def java_bitness(java_path: Path) -> int | None:
    """разрядность Java (32/64) по выводу `java -version`."""
    _, bitness = java_probe(java_path)
    return bitness

def validate_java_path(value: str | Path, *, allow_directory: bool = False) -> Path:
    """validate a Java executable without running it."""
    candidate = Path(value).expanduser()
    java_name = "java.exe" if os.name == "nt" else "java"
    if allow_directory and candidate.is_dir():
        candidate = candidate / "bin" / java_name
    try:
        resolved = candidate.resolve(strict=True)
    except OSError as exc:
        raise LauncherError(f"Java executable не найден: {candidate}") from exc
    allowed_names = {"java.exe", "javaw.exe"} if os.name == "nt" else {"java", "javaw"}
    if (
        resolved.name.lower() not in allowed_names
        or resolved.parent.name.lower() != "bin"
        or not resolved.is_file()
        or (os.name != "nt" and not os.access(resolved, os.X_OK))
    ):
        raise LauncherError("Java должна быть исполняемым файлом java/javaw внутри каталога JDK/JRE/bin")
    if os.name == "nt":
        return resolved
    try:
        root = resolved.parent.parent
        executable_stat = resolved.stat()
        bin_stat = resolved.parent.stat()
        root_stat = root.stat()
    except OSError as exc:
        raise LauncherError("Не удалось проверить права Java executable") from exc
    allowed_owners = {os.geteuid(), 0}
    if executable_stat.st_uid not in allowed_owners or bin_stat.st_uid not in allowed_owners or root_stat.st_uid not in allowed_owners:
        raise LauncherError("Java executable и каталог JDK/JRE должны принадлежать текущему пользователю или root")
    if any(mode & (stat.S_IWGRP | stat.S_IWOTH) for mode in (executable_stat.st_mode, bin_stat.st_mode, root_stat.st_mode)):
        raise LauncherError("Java executable и каталог JDK/JRE не должны быть доступны для записи группе или всем")
    return resolved

def safe_process_environment(*, compatibility: bool = False) -> dict[str, str]:
    if compatibility:
        return {
            key: value
            for key, value in os.environ.items()
            if not any(marker in key.upper() for marker in ("PASSWORD", "SECRET", "TOKEN", "PRIVATE_KEY"))
        }
    if os.name == "nt":
        passthrough = {
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "TEMP",
            "TMP",
            "PATHEXT",
            "COMSPEC",
            "PROCESSOR_ARCHITECTURE",
            "NUMBER_OF_PROCESSORS",
            "JAVA_HOME",
            "JDK_HOME",
        }
        return {key: value for key, value in os.environ.items() if key.upper() in passthrough}
    passthrough = {
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "TMPDIR",
        "TMP",
        "TEMP",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_RUNTIME_DIR",
        "PULSE_SERVER",
        "PIPEWIRE_REMOTE",
        "PIPEWIRE_LATENCY",
        "DESKTOP_SESSION",
        "GDK_BACKEND",
        "QT_QPA_PLATFORM",
        "QT_SCALE_FACTOR",
        "LD_LIBRARY_PATH",
        "JAVA_HOME",
        "JDK_HOME",
        "LIBGL_ALWAYS_SOFTWARE",
        "LIBGL_ALWAYS_INDIRECT",
        "MESA_LOADER_DRIVER_OVERRIDE",
        "GALLIUM_DRIVER",
        "__GLX_VENDOR_LIBRARY_NAME",
        "__NV_PRIME_RENDER_OFFLOAD",
        "__VK_LAYER_NV_optimus",
    }
    return {key: value for key, value in os.environ.items() if key in passthrough or key.startswith("LC_")}
