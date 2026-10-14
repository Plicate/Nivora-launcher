from __future__ import annotations

import json
import os
import platform
import shutil
import stat
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from .config import ConfigStore, _atomic_json_write
from .errors import LauncherError
from .metadata import safe_instance_name
from .platform_support import is_reparse_point


SECURITY_PROFILES = ("compat", "off", "manual", "strict")
SECURITY_PERMISSIONS = (
    "network:game",
    "files:instance",
    "desktop:display",
    "desktop:audio",
    "files:host-read",
    "files:host-write",
    "process:spawn",
    "chat:read",
    "input:capture",
    "native:load",
)
HIGH_RISK_PERMISSIONS = frozenset(
    {
        "network:game",
        "desktop:display",
        "desktop:audio",
        "files:host-read",
        "files:host-write",
        "process:spawn",
        "chat:read",
        "input:capture",
        "native:load",
    }
)
# These capabilities belong to the Java/mod process itself. The launcher can
# isolate the process, but cannot safely grant or revoke them inside one JVM.
UNENFORCED_CELL_PERMISSIONS = frozenset({"process:spawn", "chat:read", "input:capture", "native:load"})
CELL_DIRECTORY = ".trial-cell"
PROTECTED_SETTINGS_FORMAT = 1
SERVICE_JSON_FILES = (
    ".trial-launcher.json",
    ".trial-launcher-instance.json",
    ".trial-launcher-server.json",
    ".trial-launcher-live.json",
    ".trial-launcher-metrics.json",
)


def protected_settings_path(config: ConfigStore, instance_name: str) -> Path:
    safe_name = safe_instance_name(str(instance_name))
    path = config.paths.instance_security_dir / f"{safe_name}.json"
    if is_reparse_point(path):
        raise LauncherError("Защищённые настройки экземпляра являются символьной ссылкой")
    return path


def load_protected_settings(config: ConfigStore, instance_name: str, defaults: Mapping[str, Any]) -> dict[str, Any]:
    path = protected_settings_path(config, instance_name)
    if not path.exists():
        values = dict(defaults)
        save_protected_settings(config, instance_name, values)
        return values
    if not path.is_file() or path.stat().st_size > 64 * 1024:
        raise LauncherError("Защищённые настройки экземпляра повреждены")
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as exc:
        raise LauncherError(f"Не удалось прочитать защищённые настройки экземпляра: {exc}") from exc
    if not isinstance(raw, dict) or raw.get("format") != PROTECTED_SETTINGS_FORMAT:
        raise LauncherError("Защищённые настройки экземпляра имеют неподдерживаемый формат")
    values = dict(defaults)
    values.update({key: raw[key] for key in defaults if key in raw})
    return values


def save_protected_settings(config: ConfigStore, instance_name: str, values: Mapping[str, Any]) -> Path:
    path = protected_settings_path(config, instance_name)
    payload = {"format": PROTECTED_SETTINGS_FORMAT, **dict(values)}
    _atomic_json_write(path, payload, private=True)
    return path


def clear_protected_settings(config: ConfigStore, instance_name: str) -> None:
    path = protected_settings_path(config, instance_name)
    if path.exists() or is_reparse_point(path):
        path.unlink()


def ensure_service_json_files(instance_dir: Path) -> tuple[Path, ...]:
    created: list[Path] = []
    for name in SERVICE_JSON_FILES:
        path = instance_dir / name
        if is_reparse_point(path) or (path.exists() and not path.is_file()):
            raise LauncherError(f"Служебный путь экземпляра недоступен: {name}")
        if path.exists():
            continue
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                handle.write("{}\n")
        except FileExistsError:
            continue
        except OSError as exc:
            raise LauncherError(f"Не удалось создать служебный файл экземпляра: {name}") from exc
        created.append(path)
    return tuple(created)


@dataclass(frozen=True)
class InstanceCell:
    root: Path
    home: Path
    tmp: Path
    config: Path
    cache: Path
    logs: Path


@dataclass(frozen=True)
class SecurityPolicy:
    profile: str
    permissions: tuple[str, ...]
    sandbox_requested: bool
    network_access: bool
    host_file_read: bool
    host_file_write: bool
    process_spawn: bool
    native_load: bool
    display_access: bool
    audio_access: bool

    @property
    def high_risk_permissions(self) -> tuple[str, ...]:
        return tuple(permission for permission in self.permissions if permission in HIGH_RISK_PERMISSIONS)


def normalize_security_profile(value: Any) -> str:
    profile = str(value or "strict").strip().lower()
    if profile not in SECURITY_PROFILES:
        raise LauncherError(f"Неизвестный security profile: {profile}")
    return profile


def normalize_security_permissions(
    value: Any,
    *,
    default: tuple[str, ...] = ("files:instance", "desktop:display", "desktop:audio"),
) -> list[str]:
    raw = value if isinstance(value, (list, tuple, set)) else default
    result: list[str] = []
    for item in raw:
        permission = str(item).strip().lower()
        if permission not in SECURITY_PERMISSIONS or permission in result:
            continue
        result.append(permission)
    return result


def security_policy(settings: Mapping[str, Any]) -> SecurityPolicy:
    profile = normalize_security_profile(settings.get("security_profile", "strict"))
    permissions = tuple(normalize_security_permissions(settings.get("security_permissions")))
    if profile in {"manual", "strict"}:
        unsupported = sorted(UNENFORCED_CELL_PERMISSIONS.intersection(permissions))
        if unsupported:
            names = ", ".join(unsupported)
            raise LauncherError(f"Security profile {profile} не может безопасно выдать разрешения: {names}. Используйте off для доверенной сборки.")
    if profile in {"compat", "off"}:
        return SecurityPolicy(
            profile=profile,
            permissions=permissions,
            sandbox_requested=False,
            network_access=True,
            host_file_read=True,
            host_file_write=True,
            process_spawn=True,
            native_load=True,
            display_access=True,
            audio_access=True,
        )
    return SecurityPolicy(
        profile=profile,
        permissions=permissions,
        sandbox_requested=True,
        network_access="network:game" in permissions,
        host_file_read="files:host-read" in permissions,
        host_file_write="files:host-write" in permissions,
        process_spawn="process:spawn" in permissions,
        native_load="native:load" in permissions,
        display_access="desktop:display" in permissions,
        audio_access="desktop:audio" in permissions,
    )


def ensure_instance_cell(instance_dir: Path) -> InstanceCell:
    instance_dir = instance_dir.expanduser()
    if is_reparse_point(instance_dir) or not instance_dir.is_dir():
        raise LauncherError("Каталог экземпляра недоступен для создания security cell")
    instance_dir = instance_dir.resolve()
    root = instance_dir / CELL_DIRECTORY
    if is_reparse_point(root):
        raise LauncherError("Security cell является ссылкой или junction")
    directories = {
        "home": root / "home",
        "tmp": root / "tmp",
        "config": root / "config",
        "cache": root / "cache",
        "logs": root / "logs",
    }
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    for path in directories.values():
        if is_reparse_point(path):
            raise LauncherError(f"Security cell содержит ссылку или junction: {path.name}")
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        try:
            path.chmod(0o700)
        except OSError:
            pass
    try:
        root.chmod(0o700)
    except OSError:
        pass
    return InstanceCell(root=root, **directories)


def prepare_process(
    command: list[str],
    instance_dir: Path,
    environment: Mapping[str, str],
    settings: Mapping[str, Any],
    *,
    argument_file_fd: int | None = None,
    argument_file_target: str | None = None,
) -> tuple[list[str], Path, dict[str, str], str | None]:
    policy = security_policy(settings)
    if policy.profile in {"compat", "off"}:
        return command, instance_dir, dict(environment), None
    if policy.profile == "strict" and platform.system().lower() != "linux":
        raise LauncherError("strict profile сейчас поддерживается только на Linux через bubblewrap")

    cell = ensure_instance_cell(instance_dir)
    service_files = ensure_service_json_files(instance_dir)
    process_environment = dict(environment)
    process_environment.update(
        {
            "HOME": str(cell.home),
            "TMPDIR": str(cell.tmp),
            "TMP": str(cell.tmp),
            "TEMP": str(cell.tmp),
            "XDG_CONFIG_HOME": str(cell.config),
            "XDG_CACHE_HOME": str(cell.cache),
            "TRIAL_LAUNCHER_SECURITY_PROFILE": policy.profile,
            "TRIAL_LAUNCHER_CELL_ROOT": str(cell.root),
        }
    )
    if policy.profile == "manual" and platform.system().lower() != "linux":
        return command, instance_dir, process_environment, (
            f"manual profile на {platform.system()} не предоставляет OS-level sandbox; разрешения network, file и desktop не обеспечиваются."
        )
    if policy.profile == "manual" and not shutil.which("bwrap"):
        return command, instance_dir, process_environment, "bwrap не найден: manual profile не может ограничить network, file или desktop permissions."
    if policy.profile == "manual" and policy.host_file_write:
        return command, instance_dir, process_environment, "manual profile отключает OS-level sandbox; разрешения network, file и desktop не обеспечиваются."

    bubblewrap = shutil.which("bwrap")
    if not bubblewrap:
        raise LauncherError("strict profile требует установленный bubblewrap (bwrap)")
    if policy.host_file_write:
        raise LauncherError("strict profile не выдаёт модам запись в файлы компьютера; используйте off для доверенной сборки")

    mapped_command = [_map_instance_argument(value, instance_dir) for value in command]
    graphics_mode = str(process_environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE", "")).strip().lower()
    sys_mount = (
        ("--ro-bind", "/sys", "/sys")
        if policy.display_access and graphics_mode in {"auto", "mesa", "nvidia"}
        else ("--tmpfs", "/sys")
    )
    if argument_file_fd is not None and argument_file_target:
        if not argument_file_target.startswith("/instance/.trial-cell/"):
            raise LauncherError("Java argument file должен находиться внутри security cell")
        relative_target = argument_file_target.removeprefix("/instance/")
        target_path = instance_dir / relative_target
        if ".." in Path(relative_target).parts or target_path.exists() or target_path.is_symlink():
            raise LauncherError("Путь Java argument file занят или небезопасен")
        argument_reference = f"@/proc/self/fd/{argument_file_fd}"
        mapped_command = [
            f"@{argument_file_target}" if value == argument_reference else value
            for value in mapped_command
        ]
    java_root, mapped_java = _sandbox_java_path(command[0])
    sandbox = [
        bubblewrap,
        "--die-with-parent",
        "--new-session",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--ro-bind",
        "/usr",
        "/usr",
        "--tmpfs",
        "/etc",
        "--ro-bind",
        "/bin",
        "/bin",
        "--ro-bind",
        "/sbin",
        "/sbin",
        "--ro-bind",
        "/lib",
        "/lib",
        "--ro-bind",
        "/lib64",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        *sys_mount,
        "--tmpfs",
        "/run",
        "--tmpfs",
        "/home",
        "--tmpfs",
        "/root",
        "--tmpfs",
        "/mnt",
        "--tmpfs",
        "/media",
        "--tmpfs",
        "/opt",
        "--tmpfs",
        "/var",
        "--tmpfs",
        "/tmp",
        "--bind" if "files:instance" in policy.permissions else "--ro-bind",
        str(instance_dir),
        "/instance",
        "--chdir",
        "/instance",
        "--cap-drop",
        "ALL",
    ]
    _append_minimal_etc_bindings(sandbox, cell)
    # Конфигурация fontconfig — каталог, а не одиночные файлы: без него Java
    # в песочнице сыплет «Cannot load default config file» и шрифты ломаются.
    fonts_config = Path("/etc/fonts")
    if fonts_config.is_dir() and not fonts_config.is_symlink():
        sandbox.extend(("--ro-bind", str(fonts_config), str(fonts_config)))
    for service_file in service_files:
        sandbox.extend(("--ro-bind", str(service_file), f"/instance/{service_file.name}"))
    if argument_file_fd is not None and argument_file_target:
        sandbox.extend(("--ro-bind-data", str(argument_file_fd), argument_file_target))
    _append_device_bindings(
        sandbox,
        display_access=policy.display_access,
        audio_access=policy.audio_access,
    )
    if java_root and mapped_java:
        sandbox.extend(("--ro-bind", str(java_root), "/trial-java"))
        mapped_command[0] = mapped_java
    if not policy.network_access:
        sandbox.append("--unshare-net")
    if policy.host_file_read:
        # Host reads are an explicit escape hatch. Keep the rest of the host
        # unmounted instead of making it visible through a bind of '/'.
        for visible_path in ("/home", "/root", "/mnt", "/media"):
            if Path(visible_path).exists():
                sandbox.extend(("--ro-bind", visible_path, visible_path))
    process_environment["HOME"] = "/instance/.trial-cell/home"
    process_environment["TMPDIR"] = "/instance/.trial-cell/tmp"
    process_environment["TMP"] = "/instance/.trial-cell/tmp"
    process_environment["TEMP"] = "/instance/.trial-cell/tmp"
    process_environment["XDG_CONFIG_HOME"] = "/instance/.trial-cell/config"
    process_environment["XDG_CACHE_HOME"] = "/instance/.trial-cell/cache"
    process_environment["TRIAL_LAUNCHER_CELL_ROOT"] = "/instance/.trial-cell"
    process_environment.pop("DBUS_SESSION_BUS_ADDRESS", None)
    warning = None
    display = process_environment.get("DISPLAY", "").strip()
    wayland_display = process_environment.get("WAYLAND_DISPLAY", "").strip()
    runtime_dir = process_environment.get("XDG_RUNTIME_DIR", "").strip()
    if policy.display_access:
        x11_socket_dir = Path("/tmp/.X11-unix")
        if display and _local_x11_display(display) and x11_socket_dir.is_dir() and not x11_socket_dir.is_symlink():
            sandbox.extend(("--ro-bind", str(x11_socket_dir), str(x11_socket_dir)))
            if _prepare_xauthority(process_environment, cell):
                sandbox.extend(("--ro-bind", str(cell.root / "xauthority"), "/instance/.trial-cell/xauthority"))
            else:
                process_environment.pop("XAUTHORITY", None)
        else:
            if display:
                warning = "strict sandbox отключил X11: нужен локальный X11 socket и корректный Xauthority."
            process_environment.pop("XAUTHORITY", None)
        if wayland_display and runtime_dir:
            _append_runtime_socket(sandbox, Path(runtime_dir) / wayland_display)
        # Темы курсоров из домашнего каталога: HOME в песочнице — tmpfs, и без
        # этого wayland-клиент не может собрать курсоры («Failed to create
        # standard cursor»), мышка в меню игры пропадает.
        for host_icons, cell_icons in (
            (Path.home() / ".icons", Path("/instance/.trial-cell/home/.icons")),
            (Path.home() / ".local" / "share" / "icons", Path("/instance/.trial-cell/home/.local/share/icons")),
        ):
            if host_icons.is_dir() and not host_icons.is_symlink():
                sandbox.extend(("--ro-bind", str(host_icons), str(cell_icons)))
    else:
        process_environment.pop("DISPLAY", None)
        process_environment.pop("WAYLAND_DISPLAY", None)
        process_environment.pop("XAUTHORITY", None)
        process_environment.pop("GDK_BACKEND", None)
        process_environment.pop("QT_QPA_PLATFORM", None)
        if display or wayland_display:
            warning = "strict sandbox отключил графический интерфейс: отсутствует desktop:display."
    if policy.audio_access:
        for socket_name in ("pipewire-0", "pulse/native"):
            if runtime_dir:
                _append_runtime_socket(sandbox, Path(runtime_dir) / socket_name)
    else:
        process_environment.pop("PULSE_SERVER", None)
        process_environment.pop("PIPEWIRE_REMOTE", None)
        process_environment.pop("PIPEWIRE_LATENCY", None)
        if not (policy.display_access or policy.audio_access):
            process_environment.pop("XDG_RUNTIME_DIR", None)
    # The wrapper changes cwd after it starts; the parent must still use the host path.
    if "files:host-read" in policy.permissions:
        warning = (warning + " " if warning else "") + "strict sandbox разрешает чтение host-файлов только в пределах явно подключённых read-only путей."
    if not policy.network_access:
        warning = (warning + " " if warning else "") + f"{policy.profile} sandbox отключил сеть; включите network:game для сетевой игры."
    return [*sandbox, *mapped_command], instance_dir, process_environment, warning


def sandbox_status() -> dict[str, Any]:
    bubblewrap = shutil.which("bwrap")
    probe_error = ""
    strict_available = False
    if platform.system().lower() == "linux" and bubblewrap:
        true_binary = shutil.which("true") or "/usr/bin/true"
        try:
            result = subprocess.run(
                [
                    bubblewrap,
                    "--die-with-parent",
                    "--new-session",
                    "--unshare-user",
                    "--unshare-pid",
                    "--ro-bind",
                    "/usr",
                    "/usr",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--tmpfs",
                    "/tmp",
                    "--",
                    true_binary,
                ],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
                timeout=3,
                check=False,
            )
            strict_available = result.returncode == 0
            if not strict_available:
                probe_error = (result.stderr or "bubblewrap probe failed").strip()[:256]
        except (OSError, subprocess.SubprocessError) as exc:
            probe_error = str(exc)[:256]
    return {
        "platform": platform.system().lower(),
        "bubblewrap": bool(bubblewrap),
        "bubblewrap_path": bubblewrap or "",
        "strict_available": strict_available,
        "strict_probe_error": probe_error,
        "profiles": list(SECURITY_PROFILES),
        "permissions": list(SECURITY_PERMISSIONS),
    }


def _map_instance_path(value: str, instance_dir: Path) -> str:
    source = str(instance_dir)
    if value == source:
        return "/instance"
    prefix = source.rstrip("/") + "/"
    if value.startswith(prefix):
        return "/instance/" + value[len(prefix) :]
    return value


def _map_instance_argument(value: str, instance_dir: Path) -> str:
    """Map instance paths in plain args, classpaths, and -D path options."""
    if "=" in value:
        prefix, suffix = value.split("=", 1)
        mapped_suffix = _map_instance_path_list(suffix, instance_dir)
        if mapped_suffix != suffix:
            return f"{prefix}={mapped_suffix}"
    return _map_instance_path_list(value, instance_dir)


def _map_instance_path_list(value: str, instance_dir: Path) -> str:
    parts = value.split(os.pathsep)
    mapped = [_map_instance_path(part, instance_dir) for part in parts]
    return os.pathsep.join(mapped)


def _sandbox_java_path(value: str) -> tuple[Path | None, str | None]:
    candidate = Path(value).expanduser()
    if not candidate.is_absolute() or not candidate.exists():
        return None, None
    try:
        resolved = candidate.resolve()
        root = resolved.parent
        if resolved.parent.name.lower() == "bin":
            runtime_root = resolved.parent.parent
            try:
                Path.home().resolve().relative_to(runtime_root)
                runtime_root_contains_home = True
            except ValueError:
                runtime_root_contains_home = False
            # Дом JVM узнаём по любому из типичных маркеров. Раньше требовались
            # сразу release и lib — дистрибутивные JDK без файла release
            # распознавались как «просто каталог bin», и в песочнице Java
            # не находила свой conf/security/java.security.
            markers = (
                (runtime_root / "release").is_file(),
                (runtime_root / "lib").is_dir(),
                (runtime_root / "conf").is_dir(),
            )
            if any(markers) and not runtime_root_contains_home:
                root = runtime_root
        if root == Path("/") or not root.is_dir():
            return None, None
        return root, "/trial-java/" + str(resolved.relative_to(root))
    except (OSError, ValueError):
        return None, None


def _append_minimal_etc_bindings(sandbox: list[str], cell: InstanceCell) -> None:
    staged_root = cell.root / "system-etc"
    if is_reparse_point(staged_root):
        raise LauncherError("Sandbox system configuration staging path is a link")
    staged_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not staged_root.is_dir():
        raise LauncherError("Sandbox system configuration staging path is unavailable")
    created_directories: set[Path] = set()

    # Copy only runtime configuration needed by libc and Java; never expose the host /etc tree.
    for relative in (
        "group",
        "hosts",
        "ld.so.cache",
        "localtime",
        "nsswitch.conf",
        "os-release",
        "passwd",
        "resolv.conf",
        "ssl/cert.pem",
        "ssl/certs/ca-certificates.crt",
        "pki/tls/cert.pem",
        "pki/tls/certs/ca-bundle.crt",
    ):
        target = staged_root / relative
        if _stage_system_file(Path("/etc") / relative, target, staged_root, max_bytes=8 * 1024 * 1024):
            destination_parent = Path("/etc")
            for part in Path(relative).parts[:-1]:
                destination_parent /= part
                if destination_parent not in created_directories:
                    sandbox.extend(("--dir", str(destination_parent)))
                    created_directories.add(destination_parent)
            sandbox.extend(("--ro-bind", str(target), f"/etc/{relative}"))


def _stage_system_file(source: Path, target: Path, staged_root: Path, *, max_bytes: int) -> bool:
    try:
        source_fd = os.open(source, os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NONBLOCK", 0))
        with os.fdopen(source_fd, "rb") as handle:
            metadata = os.fstat(handle.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > max_bytes:
                return False
            content = handle.read(max_bytes + 1)
        if len(content) > max_bytes:
            return False

        relative_target = target.relative_to(staged_root)
        current = staged_root
        for part in relative_target.parts[:-1]:
            current /= part
            if is_reparse_point(current):
                return False
            current.mkdir(mode=0o700, exist_ok=True)
        if is_reparse_point(target):
            return False
        if target.exists():
            target.unlink()
        target_fd = os.open(
            target,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
            0o600,
        )
        with os.fdopen(target_fd, "wb") as handle:
            handle.write(content)
        target.chmod(0o400)
        return True
    except OSError:
        return False


def _append_device_bindings(
    sandbox: list[str], *, display_access: bool, audio_access: bool
) -> None:
    raw_paths: list[Path] = []
    if display_access:
        raw_paths.append(Path("/dev/dri"))
    if audio_access:
        raw_paths.append(Path("/dev/snd"))
    for raw_path in raw_paths:
        if raw_path.is_dir() and not raw_path.is_symlink():
            sandbox.extend(("--dev-bind", str(raw_path), str(raw_path)))
    if display_access:
        for raw_path in sorted(Path("/dev").glob("nvidia*")):
            if raw_path.is_symlink() or not (raw_path.is_dir() or raw_path.is_char_device()):
                continue
            sandbox.extend(("--dev-bind", str(raw_path), str(raw_path)))


def _local_x11_display(value: str) -> bool:
    display = value.removeprefix("unix/")
    if not display.startswith(":"):
        return False
    number = display[1:].split(".", 1)[0]
    return number.isdigit()


def _prepare_xauthority(environment: dict[str, str], cell: InstanceCell) -> bool:
    raw = environment.get("XAUTHORITY", "").strip()
    if not raw:
        return False
    source = Path(raw).expanduser()
    target = cell.root / "xauthority"
    if source.is_symlink() or not source.is_file() or target.is_symlink():
        return False
    try:
        shutil.copyfile(source, target)
        target.chmod(0o600)
    except OSError:
        return False
    environment["XAUTHORITY"] = "/instance/.trial-cell/xauthority"
    return True


def _append_runtime_socket(sandbox: list[str], socket_path: Path) -> None:
    if socket_path.is_symlink() or not socket_path.is_socket():
        return
    runtime_root = Path("/run")
    try:
        relative = socket_path.resolve().relative_to(runtime_root)
    except ValueError:
        return
    target = runtime_root / relative
    parents: list[Path] = []
    current = target.parent
    while current != runtime_root and runtime_root in current.parents:
        parents.append(current)
        current = current.parent
    for parent in reversed(parents):
        sandbox.extend(("--dir", str(parent)))
    sandbox.extend(("--ro-bind", str(socket_path), str(target)))
