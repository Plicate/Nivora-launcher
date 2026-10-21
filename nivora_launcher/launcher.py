from __future__ import annotations

import codecs
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Mapping

from .accounts import Account, AccountStore, OfflineDebugAccount
from .config import ConfigStore, java_memory_value
from .errors import LauncherError
from .installer import InstallResult, MinecraftInstaller
from .instance_files import instance_log_hash
from .instances import InstanceStore
from .metadata import VersionRepository
from .operations import CancellationToken, OperationCancelled
from .platform_support import mojang_os_name, open_path, process_start_options, set_private_fd, set_private_path, terminate_process_tree
from .process_guard import InstanceProcessLock
from .runtime import JavaRuntimeManager, java_bitness, java_probe, java_version, safe_process_environment
from .security import ensure_instance_cell, prepare_process
from .optimizer import JVM_PRESETS, analyze_instance, record_launch_end, record_launch_start

_AUTH_TOKEN_PLACEHOLDER = "__TRIAL_LAUNCHER_AUTH_TOKEN__"
_JAVA_ARGUMENT_FILE_TARGET = "/instance/.trial-cell/tmp/.trial-launcher-java-args"
_WAYLAND_GLFW_URL = "https://github.com/jdkeke142/glfw-wayland-minecraft/releases/download/v1.1.0/libglfw.so"
_WAYLAND_GLFW_SHA256 = "1fa075ea4f595bd6917a7f304f2fe5d583ec82d8f1ac0f0350bd38b32a20a46e"
_WAYLAND_GLFW_SIZE = 662_440

class _JavaArgumentFile:
    def __init__(self, *, fd: int | None = None, path: Path | None = None) -> None:
        self.fd = fd
        self.path = path
        self.argument = f"@/proc/self/fd/{fd}" if fd is not None else f"@{path}"

    @property
    def pass_fds(self) -> tuple[int, ...]:
        return (self.fd,) if self.fd is not None else ()

    def close(self) -> None:
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None
        if self.path is not None:
            try:
                self.path.unlink()
            except FileNotFoundError:
                pass
            except OSError:
                pass

def _java_argument(value: str) -> str:
    if "\x00" in value or "\n" in value or "\r" in value:
        raise LauncherError("Аргумент Java содержит недопустимый перевод строки")
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'

def _is_wayland_session(environment: Mapping[str, str], version_id: str) -> bool:
    return (
        str(environment.get("XDG_SESSION_TYPE", "")).strip().lower() == "wayland"
        and bool(environment.get("WAYLAND_DISPLAY"))
        and not _is_legacy_version(version_id)
    )

def _native_wayland_command(command: list[str], main_class: str, library: Path) -> list[str]:
    try:
        main_index = command.index(main_class, 1)
    except ValueError as exc:
        raise LauncherError("Не найден mainClass для native Wayland GLFW") from exc
    return [
        *command[:main_index],
        f"-Dorg.lwjgl.glfw.libname={library}",
        "-Dsodium.checks.issue2561=false",
        *command[main_index:],
    ]

def _native_wayland_environment(environment: dict[str, str]) -> None:
    for key in (
        "__GLX_VENDOR_LIBRARY_NAME",
        "__VK_LAYER_NV_optimus",
        "MESA_LOADER_DRIVER_OVERRIDE",
        "GALLIUM_DRIVER",
        "LIBGL_ALWAYS_SOFTWARE",
        "LIBGL_DRI3_DISABLE",
        "LIBGL_ALWAYS_INDIRECT",
        "MESA_GL_VERSION_OVERRIDE",
        "MESA_GLSL_VERSION_OVERRIDE",
    ):
        environment.pop(key, None)
    if environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE") != "nvidia":
        environment.pop("__NV_PRIME_RENDER_OFFLOAD", None)
    environment["GLFW_FORCE_WAYLAND"] = "1"
    environment["TRIAL_LAUNCHER_GRAPHICS_BACKEND"] = "native-wayland-glfw-v1.1.0"

def _looks_like_non_nvidia_renderer(line: str) -> bool:
    lowered = line.lower()
    if "llvmpipe" in lowered or "softpipe" in lowered:
        return True
    vendor = re.search(r"\bopengl\s+vendor\s*:\s*([^\r\n]+)", line, re.IGNORECASE)
    return bool(vendor and "nvidia" not in vendor.group(1).lower())

def _windows_short_path(path: str) -> str:
    """8.3-имя существующего пути (ASCII); пустая строка, если недоступно."""
    if os.name != "nt":
        return ""
    try:
        import ctypes

        get_short = ctypes.windll.kernel32.GetShortPathNameW  # type: ignore[attr-defined]
        size = get_short(path, None, 0)
        if not size:
            return ""
        buffer = ctypes.create_unicode_buffer(size)
        return buffer.value if get_short(path, buffer, size) else ""
    except (AttributeError, OSError, ValueError):
        return ""

def _encodable(value: str, encoding: str) -> bool:
    try:
        value.encode(encoding, "strict")
        return True
    except (UnicodeEncodeError, LookupError):
        return False

def _windows_ansi_argument(value: str, encoding: str, short_path: Callable[[str], str] = _windows_short_path) -> str:
    """привести аргумент к системной ANSI-кодировке Windows."""
    if _encodable(value, encoding):
        return value
    pieces: list[str] = []
    for piece in value.split(";"):
        if _encodable(piece, encoding):
            pieces.append(piece)
            continue
        prefix, path = "", piece
        if piece.startswith("-") and "=" in piece:
            prefix, path = piece.split("=", 1)
            prefix += "="
        shortened = short_path(path) if path else ""
        if not shortened or not _encodable(shortened, encoding):
            raise LauncherError(
                "Путь содержит символы, которые Java на Windows не может прочитать в системной кодировке "
                f"({encoding}): {path}. Перенесите папку игры в путь из латинских букв "
                "(Настройки → Папка игры) или включите в Windows «Бета: UTF-8 для всех языков»."
            )
        pieces.append(prefix + shortened)
    return ";".join(pieces)

def _windows_ansi_command(command: list[str], encoding: str = "", short_path: Callable[[str], str] = _windows_short_path) -> list[str]:
    encoding = encoding or _windows_ansi_encoding()
    return [_windows_ansi_argument(value, encoding, short_path) for value in command]

def _windows_ansi_encoding() -> str:
    # «mbcs» — текущая ANSI-кодовая страница Windows; вне Windows кодека нет.
    try:
        codecs.lookup("mbcs")
        return "mbcs"
    except LookupError:
        return "utf-8"

def _argument_file_encoding() -> str:
    # java читает @argfile в той же кодировке, что и argv: на Windows — ANSI.
    return _windows_ansi_encoding() if os.name == "nt" else "utf-8"

def _create_java_argument_file(
    arguments: list[str],
    instance_dir: Path,
    *,
    private_directory: Path | None = None,
) -> _JavaArgumentFile:
    content = "\n".join(_java_argument(value) for value in arguments) + "\n"
    memfd_create = getattr(os, "memfd_create", None)
    fd: int | None = None
    if memfd_create is not None:
        try:
            fd = memfd_create("nivora-launcher-java-args", 0)
            data = content.encode("utf-8")
            while data:
                written = os.write(fd, data)
                if written <= 0:
                    raise OSError("не удалось записать Java argument file")
                data = data[written:]
            os.lseek(fd, 0, os.SEEK_SET)
            return _JavaArgumentFile(fd=fd)
        except OSError:
            if fd is not None:
                try:
                    os.close(fd)
                except OSError:
                    pass

    if os.name == "nt":
        if private_directory is None:
            raise LauncherError("Для Windows Java argument file нужен приватный каталог данных приложения")
        arguments_dir = private_directory
        arguments_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    else:
        arguments_dir = ensure_instance_cell(instance_dir).tmp
    fd, path = tempfile.mkstemp(prefix=".java-args-", dir=arguments_dir)
    try:
        set_private_fd(fd)
        with os.fdopen(fd, "w", encoding=_argument_file_encoding(), errors="strict") as handle:
            handle.write(content)
    except UnicodeEncodeError as exc:
        try:
            os.close(fd)
        except OSError:
            pass
        Path(path).unlink(missing_ok=True)
        raise LauncherError("Аргументы Java содержат символы вне системной кодировки Windows") from exc
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        Path(path).unlink(missing_ok=True)
        raise
    if os.name == "nt":
        set_private_path(Path(path))
    return _JavaArgumentFile(path=Path(path))

def _create_launcher_java_argument_file(arguments: list[str], instance_dir: Path, data_home: Path) -> _JavaArgumentFile:
    if os.name == "nt":
        return _create_java_argument_file(arguments, instance_dir, private_directory=data_home)
    return _create_java_argument_file(arguments, instance_dir)

def _should_use_argument_file(has_token: bool, java_major: int | None) -> bool:
    """передавать ли аргументы Java через @argfile."""
    if java_major is not None and java_major < 9:
        return False
    return has_token or os.name == "nt"

def _materialize_auth_token(command: list[str], access_token: str) -> list[str]:
    if not access_token:
        return command
    return [value.replace(_AUTH_TOKEN_PLACEHOLDER, access_token) for value in command]

class GameLauncher:
    def __init__(
        self,
        config: ConfigStore,
        installer: MinecraftInstaller,
        accounts: AccountStore,
        repository: VersionRepository,
        cancellation: CancellationToken | None = None,
    ) -> None:
        self.config = config
        self.installer = installer
        self.accounts = accounts
        self.repository = repository
        self.runtime = JavaRuntimeManager(config, installer.http)
        self.offline = OfflineDebugAccount(config, accounts)
        self.cancellation = cancellation
        self.process: subprocess.Popen[str] | None = None

    def _resolved_graphics_mode(self, settings: Mapping[str, Any] | None) -> str:
        requested = str((settings or {}).get("graphics_mode", "") or "").strip().lower()
        configured = str(getattr(self.config.settings, "graphics_mode", "auto") or "auto").strip().lower()
        return configured if requested in {"", "auto"} else requested

    def _nvidia_gpu_available(self) -> bool:
        nvidia_smi = shutil.which("nvidia-smi")
        if not nvidia_smi:
            return False
        try:
            result = subprocess.run(
                [nvidia_smi, "--query-gpu=name", "--format=csv,noheader"],
                capture_output=True,
                text=True,
                timeout=3,
                check=False,
                env=safe_process_environment(),
            )
        except (OSError, subprocess.SubprocessError):
            return False
        return result.returncode == 0 and bool(result.stdout.strip())

    def _wayland_glfw_library(self, instance_dir: Path) -> Path:
        machine = os.uname().machine.lower() if hasattr(os, "uname") else ""
        if machine not in {"x86_64", "amd64"}:
            raise LauncherError("Native Wayland GLFW fallback доступен только для Linux x86_64")
        library_dir = ensure_instance_cell(instance_dir).cache / "wayland-glfw-v1.1.0"
        if library_dir.is_symlink() or (library_dir.exists() and not library_dir.is_dir()):
            raise LauncherError("Каталог native Wayland GLFW небезопасен")
        library_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        library = library_dir / "libglfw.so"
        if library.is_symlink() or (library.exists() and not library.is_file()):
            raise LauncherError("Файл native Wayland GLFW небезопасен")
        self.installer.http.download(
            _WAYLAND_GLFW_URL,
            library,
            sha256=_WAYLAND_GLFW_SHA256,
            size=_WAYLAND_GLFW_SIZE,
        )
        return library

    def build_command(
        self,
        install: InstallResult,
        account: Account,
        settings: Mapping[str, Any] | None = None,
    ) -> list[str]:
        launch_settings = settings or {}
        java_override = str(launch_settings.get("java_path", "")).strip()
        java = self.runtime.java_for(install.version_json, java_override) if java_override else self.runtime.java_for(install.version_json)
        game_assets = install.assets_root
        asset_index = install.version_json.get("assetIndex")
        if isinstance(asset_index, dict) and asset_index.get("map_to_resources"):
            game_assets = install.instance_dir / "resources"
        placeholders = {
            "auth_player_name": account.username,
            "version_name": install.version_id,
            "game_directory": str(install.instance_dir),
            "assets_root": str(install.assets_root),
            "game_assets": str(game_assets),
            "assets_index_name": install.asset_index_name,
            "auth_uuid": account.uuid,
            "auth_access_token": _AUTH_TOKEN_PLACEHOLDER if account.kind in ("microsoft", "ely") and account.access_token else account.access_token,
            "auth_session": _AUTH_TOKEN_PLACEHOLDER if account.kind in ("microsoft", "ely") and account.access_token else account.access_token,
            "auth_profile_id": account.uuid,
            "user_properties": "{}",
            "user_type": {"microsoft": "msa", "ely": "mojang"}.get(account.kind, "legacy"),
            "version_type": str(install.version_json.get("type", "release")),
            "natives_directory": str(install.natives_dir),
            "launcher_name": "Nivora Launcher",
            "launcher_version": self.config.settings.launcher_version,
            "library_directory": str(install.instance_dir / "libraries"),
            "classpath_separator": os.pathsep,
            "classpath": os.pathsep.join(str(path) for path in [*install.libraries, install.client_jar]),
            "auth_xuid": account.xuid,
            "clientid": self.config.settings.client_id,
            "resolution_width": str(launch_settings.get("window_width", 1280)),
            "resolution_height": str(launch_settings.get("window_height", 720)),
            "path": str(install.logging_file or ""),
        }
        features = {"has_custom_resolution": True}
        arguments = install.version_json.get("arguments", {})
        jvm_items: list[Any] = []
        if isinstance(arguments, dict):
            if isinstance(arguments.get("default-user-jvm"), list):
                jvm_items.extend(arguments["default-user-jvm"])
            if isinstance(arguments.get("jvm"), list):
                jvm_items.extend(arguments["jvm"])
        game_items: list[Any] = arguments.get("game", []) if isinstance(arguments, dict) else []
        if not jvm_items and install.version_json.get("minecraftArguments"):
            game_items = shlex.split(str(install.version_json["minecraftArguments"]))
        jvm_args = _expand_arguments(jvm_items, placeholders, features)
        game_args = _expand_arguments(game_items, placeholders, features)
        profile_jvm_args = _safe_extra_arguments(launch_settings.get("jvm_arguments"))
        profile_game_args = _safe_extra_arguments(launch_settings.get("game_arguments"))
        if profile_jvm_args:
            jvm_args = [*profile_jvm_args, *jvm_args]
        if profile_game_args:
            game_args = [*game_args, *profile_game_args]
        # inject JVM preset flags (strip existing GC/UnlockExperimental args first)
        preset_id = str(launch_settings.get("jvm_preset", "none") or "none")
        preset_flags = list(JVM_PRESETS.get(preset_id, {}).get("flags", []))
        if preset_flags:
            _GC_PREFIXES = ("-XX:", "-Dusing.aikars", "-Daikars", "-Dfml.earlyprogresswindow")
            jvm_args = [a for a in jvm_args if not any(a.startswith(p) for p in _GC_PREFIXES)]
            jvm_args = [*preset_flags, *jvm_args]
        if _is_legacy_version(install.version_id):
            compatibility_args = [
                # современные флаги (пресеты, аргументы профиля) не должны убивать Java 8
                "-XX:+IgnoreUnrecognizedVMOptions",
                "-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true",
                "-Dorg.lwjgl.opengl.Display.useXRandR=false",
            ]
            jvm_args = [*compatibility_args, *jvm_args]
        if bool(launch_settings.get("fullscreen", False)) and "--fullscreen" not in game_args:
            game_args.append("--fullscreen")
        quick_world = str(launch_settings.get("quick_play_world", "")).strip()
        quick_server = str(launch_settings.get("quick_play_server", "")).strip()
        if quick_world:
            game_args.extend(["--quickPlaySingleplayer", quick_world])
        elif quick_server:
            game_args.extend(["--quickPlayMultiplayer", quick_server])
        graphics_mode = self._resolved_graphics_mode(launch_settings)
        if "-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true" not in jvm_args:
            # keep LWJGL 2 software rendering available after an explicit-driver fallback.
            jvm_args.insert(0, "-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true")
        if graphics_mode in {"", "auto", "software"} and _is_legacy_version(install.version_id):
            for argument in ("-Dorg.lwjgl.opengl.Display.useXRandR=false",):
                if argument not in jvm_args:
                    jvm_args.insert(0, argument)
        jvm_args = _ensure_native_library_path(jvm_args, install.natives_dir)
        memory = str(launch_settings.get("memory", "")) or self.config.settings.memory
        jvm_args = _apply_memory_setting(jvm_args, memory)
        classpath = placeholders["classpath"]
        if "-cp" not in jvm_args and "-classpath" not in jvm_args:
            jvm_args.extend(["-cp", classpath])
        main_class = install.version_json.get("mainClass")
        if not main_class:
            raise LauncherError(f"У версии {install.version_id} отсутствует mainClass")
        return [str(java), *jvm_args, str(main_class), *game_args]

    def run(
        self,
        install: InstallResult,
        account: Account,
        *,
        dry_run: bool = False,
        output: Callable[[str], None] | None = None,
        settings: Mapping[str, Any] | None = None,
    ) -> int:
        emit = output or print
        if account.kind == "offline" and self.offline.remaining(account) <= 0:
            raise LauncherError("Ограничение автономного аккаунта исчерпано. Используйте Microsoft-авторизацию.")
        launch_settings = settings or {}
        command = _materialize_auth_token(self.build_command(install, account, launch_settings), account.access_token)
        command = _adapt_command_for_old_java(command, emit)
        if os.name == "nt":
            command = _windows_ansi_command(command)
        original_command = command
        if dry_run:
            emit(shlex.join(_redact_command(command, {account.access_token, _AUTH_TOKEN_PLACEHOLDER})))
            return 0
        instance_name = install.instance_dir.name
        log_path = self.config.paths.logs_dir / (
            f"{_log_component(instance_name)}-{instance_log_hash(instance_name)}-"
            f"{_log_component(install.version_id)}-{time.time_ns()}-{uuid.uuid4().hex[:10]}.log"
        )
        log_path.parent.mkdir(parents=True, exist_ok=True)
        live_state_path = install.instance_dir / ".trial-launcher-live.json"
        if live_state_path.is_symlink() or log_path.is_symlink():
            raise LauncherError("Файлы состояния запуска являются символьными ссылками")
        _ensure_private_file(log_path)
        _launch_id = str(uuid.uuid4())
        _launch_preset = str(launch_settings.get("jvm_preset", "none") or "none")
        _launch_memory = str(launch_settings.get("memory", "")) or getattr(self.config.settings, "memory", "auto")
        try:
            record_launch_start(
                install.instance_dir,
                _launch_id,
                loader=str(install.version_id),
                preset=_launch_preset,
                memory=_launch_memory,
                mc_version=str(install.version_id),
            )
        except Exception:
            pass
        _launch_wall_start = time.monotonic()
        started = _launch_wall_start
        environment = self._launch_environment(launch_settings, install.version_id)
        def publish(line: str) -> None:
            emit(line)

        def event(line: str) -> None:
            flags = os.O_WRONLY | os.O_APPEND | getattr(os, "O_NOFOLLOW", 0)
            try:
                fd = os.open(log_path, flags)
                with os.fdopen(fd, "a", encoding="utf-8") as log:
                    log.write(line + "\n")
                    log.flush()
            except OSError:
                pass
            publish(line)

        event(f"Лог запуска: {log_path}")
        event(f"Запуск {install.version_id} для {account.username}")
        version_json = getattr(install, "version_json", {})
        is_wayland = _is_wayland_session(environment, install.version_id)
        has_custom_glfw = any(argument.startswith("-Dorg.lwjgl.glfw.libname=") for argument in command[1:])
        native_wayland_first = (
            environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE") == "nvidia"
            and is_wayland
            and not has_custom_glfw
        )
        native_wayland_unavailable = False
        java_metadata = version_json.get("javaVersion", {}) if isinstance(version_json, dict) else {}
        required_java_major = java_metadata.get("majorVersion") if isinstance(java_metadata, dict) else None
        actual_java_major = java_version(Path(command[0])) if required_java_major else None
        if actual_java_major:
            java_line = f"Java: {actual_java_major}"
            if required_java_major:
                java_line += f"; Minecraft metadata: {required_java_major}"
                try:
                    java_mismatch = actual_java_major != int(required_java_major)
                except (TypeError, ValueError):
                    java_mismatch = False
                if java_mismatch:
                    configured_java = bool(
                        launch_settings.get("java_path") or getattr(self.config.settings, "java_path", "")
                    )
                    source = "выбранная вручную" if configured_java else "системная"
                    event(
                        f"Предупреждение: {source} Java {actual_java_major} отличается от Java "
                        f"{required_java_major} из метаданных версии; совместимость не гарантирована."
                    )
            event(java_line)
        has_token = account.kind in ("microsoft", "ely") and bool(account.access_token)
        argument_file_java = actual_java_major
        if argument_file_java is None and (has_token or os.name == "nt"):
            argument_file_java = java_version(Path(command[0]))
        use_argument_file = _should_use_argument_file(has_token, argument_file_java)
        if has_token and not use_argument_file:
            event("Java 8 не поддерживает файлы аргументов (@argfile): параметры передаются напрямую.")
        captured: list[str] = []
        return_code: int | None = None
        wrong_nvidia_renderer = False

        def record(line: str, log: Any) -> None:
            captured.append(line)
            if len(captured) > 500:
                del captured[:-500]
            log.write(line + "\n")
            log.flush()
            publish(line)

        java_argument_file: _JavaArgumentFile | None = None
        java_argument_files: list[_JavaArgumentFile] = []
        launch_command = command
        process_command = launch_command
        process_cwd = install.instance_dir
        process_lock = InstanceProcessLock(self.config.paths.data_home, install.instance_dir.name)
        try:
            process_lock.acquire()
        except BaseException:
            if java_argument_file:
                java_argument_file.close()
            raise
        _write_live_state(
            live_state_path,
            running=True,
            exit_code=None,
            pid=0,
            started_at=time.time(),
            log_id=log_path.name,
        )

        def run_process(
            process_environment: dict[str, str],
            append_log: bool = False,
            command_override: list[str] | None = None,
            cwd_override: Path | None = None,
            argument_file_override: _JavaArgumentFile | None = None,
        ) -> int:
            nonlocal wrong_nvidia_renderer
            active_command = command_override or process_command
            active_cwd = cwd_override or process_cwd
            active_argument_file = argument_file_override or java_argument_file
            log_fd_flags = os.O_WRONLY | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
            log_fd_flags |= os.O_APPEND if append_log else os.O_TRUNC
            log_fd = os.open(log_path, log_fd_flags, 0o600)
            with os.fdopen(log_fd, "a" if append_log else "w", encoding="utf-8") as log:
                popen_options: dict[str, Any] = {
                    "cwd": active_cwd,
                    "stdout": subprocess.PIPE,
                    "stderr": subprocess.STDOUT,
                    "text": True,
                    "bufsize": 1,
                    "env": process_environment,
                }
                if active_argument_file and active_argument_file.pass_fds and os.name != "nt":
                    popen_options["pass_fds"] = active_argument_file.pass_fds
                popen_options.update(process_start_options())
                process = subprocess.Popen(active_command, **popen_options)
                self.process = process
                _write_live_state(
                    live_state_path,
                    running=True,
                    exit_code=None,
                    pid=int(getattr(process, "pid", 0) or 0),
                    started_at=time.time(),
                    log_id=log_path.name,
                )
                if process.stdout is None:
                    raise LauncherError("Java не открыла поток вывода")
                monitor_stop = threading.Event()

                def monitor_cancellation() -> None:
                    while not monitor_stop.wait(0.1):
                        if self.cancellation and self.cancellation.cancelled():
                            self.stop()
                            return

                monitor = threading.Thread(target=monitor_cancellation, daemon=True)
                monitor.start()
                attempt_output: list[str] = []
                try:
                    for line in process.stdout:
                        value = line.rstrip("\n")
                        record(value, log)
                        attempt_output.append(value)
                        if len(attempt_output) > 500:
                            del attempt_output[:-500]
                        if (
                            process_environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE") == "nvidia"
                            and _looks_like_non_nvidia_renderer(value)
                        ):
                            wrong_nvidia_renderer = True
                            record("Игра сообщила OpenGL renderer не NVIDIA при выбранном режиме NVIDIA; останавливаю запуск без software fallback.", log)
                            self.stop()
                            break
                        if _looks_like_glx_context_error(attempt_output):
                            record("Обнаружена ошибка GLX; закрываю зависшее окно игры для повторного запуска.", log)
                            self.stop()
                            break
                    return_code = process.wait()
                finally:
                    monitor_stop.set()
                    monitor.join(timeout=1)
                if self.cancellation:
                    self.cancellation.check()
                return return_code

        try:
            if native_wayland_first:
                try:
                    library = self._wayland_glfw_library(install.instance_dir)
                    command = _native_wayland_command(
                        command,
                        str(version_json.get("mainClass", "")),
                        library,
                    )
                    java_argument_file = (
                        _create_launcher_java_argument_file(
                            command[1:],
                            install.instance_dir,
                            self.config.paths.data_home,
                        )
                        if use_argument_file
                        else None
                    )
                    if java_argument_file:
                        java_argument_files.append(java_argument_file)
                        launch_command = [command[0], java_argument_file.argument]
                    else:
                        launch_command = command
                    process_command = launch_command
                    _native_wayland_environment(environment)
                    event("NVIDIA + Wayland: использую native Wayland GLFW с первой попытки, без предварительного GLX.")
                except (LauncherError, OSError) as exc:
                    native_wayland_unavailable = True
                    native_wayland_first = False
                    command = original_command
                    launch_command = command
                    process_command = launch_command
                    event(f"Native Wayland GLFW недоступен; пробую обычный запуск через GLX: {exc}")
            event(
                "Графика: "
                f"mode={environment.get('TRIAL_LAUNCHER_GRAPHICS_MODE', getattr(self.config.settings, 'graphics_mode', 'auto'))}, "
                f"GLX={environment.get('__GLX_VENDOR_LIBRARY_NAME', 'system')}, "
                f"PRIME={'on' if environment.get('__NV_PRIME_RENDER_OFFLOAD') == '1' else 'off'}, "
                f"GLFW={environment.get('TRIAL_LAUNCHER_GRAPHICS_BACKEND', 'bundled')}, "
                f"session={environment.get('XDG_SESSION_TYPE', 'unknown')}, "
                f"legacy_lwjgl={'yes' if _is_legacy_version(install.version_id) else 'no'}"
            )
            if not java_argument_file and use_argument_file:
                java_argument_file = _create_launcher_java_argument_file(
                    command[1:],
                    install.instance_dir,
                    self.config.paths.data_home,
                )
                java_argument_files.append(java_argument_file)
                launch_command = [command[0], java_argument_file.argument]
                process_command = launch_command
            process_command, process_cwd, environment, security_warning = prepare_process(
                launch_command,
                install.instance_dir,
                environment,
                launch_settings,
                argument_file_fd=java_argument_file.fd if java_argument_file else None,
                argument_file_target=_JAVA_ARGUMENT_FILE_TARGET if java_argument_file else None,
            )
            if security_warning:
                event(f"Безопасность: {security_warning}")
            return_code = run_process(environment, append_log=True)
            requested_mode = self._resolved_graphics_mode(launch_settings)
            nvidia_mode = environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE") == "nvidia"
            if return_code != 0 and requested_mode != "software" and _looks_like_glx_context_error(captured):
                if nvidia_mode:
                    event("NVIDIA не создала OpenGL-контекст; не переключаю запуск на llvmpipe.")
                else:
                    if is_wayland and not has_custom_glfw and not native_wayland_unavailable:
                        native_argument_file: _JavaArgumentFile | None = None
                        try:
                            library = self._wayland_glfw_library(install.instance_dir)
                            native_command = _native_wayland_command(
                                command,
                                str(version_json.get("mainClass", "")),
                                library,
                            )
                            if java_argument_file:
                                native_argument_file = _create_launcher_java_argument_file(
                                    native_command[1:],
                                    install.instance_dir,
                                    self.config.paths.data_home,
                                )
                                java_argument_files.append(native_argument_file)
                            native_launch_command = (
                                [native_command[0], native_argument_file.argument]
                                if native_argument_file
                                else native_command
                            )
                            native_environment = self._launch_environment(launch_settings, install.version_id)
                            _native_wayland_environment(native_environment)
                            (
                                native_process_command,
                                native_process_cwd,
                                native_environment,
                                native_warning,
                            ) = prepare_process(
                                native_launch_command,
                                install.instance_dir,
                                native_environment,
                                launch_settings,
                                argument_file_fd=native_argument_file.fd if native_argument_file else None,
                                argument_file_target=_JAVA_ARGUMENT_FILE_TARGET if native_argument_file else None,
                            )
                            event("GLX не создал контекст; пробую native Wayland GLFW v1.1.0 с проверкой SHA-256.")
                            if native_warning:
                                event(f"Безопасность: {native_warning}")
                            return_code = run_process(
                                native_environment,
                                append_log=True,
                                command_override=native_process_command,
                                cwd_override=native_process_cwd,
                                argument_file_override=native_argument_file,
                            )
                        except (LauncherError, OSError) as exc:
                            event(f"Native Wayland GLFW retry недоступен: {exc}")
                        finally:
                            if native_argument_file:
                                native_argument_file.close()
                    if return_code != 0:
                        retry_environment = self._launch_environment(
                            {**launch_settings, "graphics_mode": "software"},
                            install.version_id,
                        )
                        event(
                            "Графический запуск не удался; повторяю запуск в явно отмеченном режиме "
                            f"{retry_environment.get('TRIAL_LAUNCHER_GRAPHICS_MODE', 'software')}/llvmpipe."
                        )
                        process_command, process_cwd, retry_environment, retry_warning = prepare_process(
                            launch_command,
                            install.instance_dir,
                            retry_environment,
                            launch_settings,
                            argument_file_fd=java_argument_file.fd if java_argument_file else None,
                            argument_file_target=_JAVA_ARGUMENT_FILE_TARGET if java_argument_file else None,
                        )
                        if retry_warning:
                            event(f"Безопасность: {retry_warning}")
                        return_code = run_process(retry_environment, append_log=True)
            if wrong_nvidia_renderer:
                event("Запуск остановлен: NVIDIA была выбрана, но игра сообщила OpenGL renderer не NVIDIA.")
            elif return_code != 0 and requested_mode != "software" and is_wayland and not _looks_like_glx_context_error(captured):
                if native_wayland_first:
                    event("Native Wayland NVIDIA запуск завершился без подтверждённого NVIDIA renderer.")
                else:
                    event(
                        "Игра завершилась с ошибкой, но в выводе нет распознанного GLX-сбоя; "
                        "native Wayland retry не запускался. Нужен конец лога запуска."
                    )
            return return_code
        except OSError as exc:
            raise LauncherError(f"Не удалось запустить Java: {exc}") from exc
        finally:
            self.process = None
            _write_live_state(
                live_state_path,
                running=False,
                exit_code=return_code,
                pid=0,
                started_at=0,
                log_id=log_path.name,
            )
            process_lock.release()
            for argument_file in java_argument_files:
                argument_file.close()
            try:
                _elapsed = time.monotonic() - _launch_wall_start
                record_launch_end(install.instance_dir, _launch_id, exit_code=return_code, elapsed_seconds=_elapsed)
            except Exception:
                pass
            try:
                InstanceStore(self.config).add_played_seconds(install.instance_dir, max(0, int(time.monotonic() - started)))
            except LauncherError:
                pass
            if account.kind == "offline":
                remaining = self.offline.finish(account, started)
                publish(f"Осталось debug-времени: {remaining // 60} мин {remaining % 60} сек")
        return return_code

    def _launch_environment(
        self,
        settings: Mapping[str, Any] | None = None,
        version_id: str | None = None,
    ) -> dict[str, str]:
        launch_settings = settings or {}
        security_profile = str(launch_settings.get("security_profile", "strict") or "strict").strip().lower()
        environment = safe_process_environment(compatibility=security_profile in {"compat", "off"})
        mode = self._resolved_graphics_mode(launch_settings)
        if mode == "auto" and self._nvidia_gpu_available():
            mode = "nvidia"
        if security_profile not in {"compat", "off"}:
            for key in (
                "__GLX_VENDOR_LIBRARY_NAME",
                "MESA_LOADER_DRIVER_OVERRIDE",
                "GALLIUM_DRIVER",
                "LIBGL_ALWAYS_SOFTWARE",
                "LIBGL_DRI3_DISABLE",
                "LIBGL_ALWAYS_INDIRECT",
                "MESA_GL_VERSION_OVERRIDE",
                "MESA_GLSL_VERSION_OVERRIDE",
                "__NV_PRIME_RENDER_OFFLOAD",
                "__VK_LAYER_NV_optimus",
            ):
                environment.pop(key, None)
        if mode in {"mesa", "software"}:
            environment.pop("__NV_PRIME_RENDER_OFFLOAD", None)
            environment.pop("__VK_LAYER_NV_optimus", None)
        if mode == "nvidia":
            environment["__NV_PRIME_RENDER_OFFLOAD"] = "1"
            environment["__GLX_VENDOR_LIBRARY_NAME"] = "nvidia"
        elif mode == "mesa":
            environment["__GLX_VENDOR_LIBRARY_NAME"] = "mesa"
        elif mode == "software":
            # keep GLVND from selecting a broken proprietary GLX provider when
            # the explicit fallback is meant to use Mesa's llvmpipe.
            environment["__GLX_VENDOR_LIBRARY_NAME"] = "mesa"
            environment["LIBGL_ALWAYS_SOFTWARE"] = "1"
            environment["GALLIUM_DRIVER"] = "llvmpipe"
            environment.pop("MESA_LOADER_DRIVER_OVERRIDE", None)
            environment["LIBGL_DRI3_DISABLE"] = "1"
            environment["LIBGL_ALWAYS_INDIRECT"] = "0"
            # llvmpipe не тянет дефолтный профиль контекста LWJGL — держим совместимый
            if _is_legacy_version(version_id or ""):
                environment["MESA_GL_VERSION_OVERRIDE"] = "2.1"
                environment["MESA_GLSL_VERSION_OVERRIDE"] = "120"
            else:
                environment["MESA_GL_VERSION_OVERRIDE"] = "3.3"
                environment["MESA_GLSL_VERSION_OVERRIDE"] = "330"
        environment["TRIAL_LAUNCHER_GRAPHICS_MODE"] = mode or "auto"
        return environment

    def stop(self) -> None:
        process = self.process
        if not process or process.poll() is not None:
            return
        terminate_process_tree(process)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            terminate_process_tree(process, force=True)

    def open_folder(self, path: Path | None = None) -> None:
        root = self.config.game_dir.resolve()
        target = (path or root).expanduser().resolve()
        if not _path_within(target, root):
            raise LauncherError("Можно открыть только папку игры или её содержимое")
        target.mkdir(parents=True, exist_ok=True)
        try:
            open_path(target)
        except RuntimeError as exc:
            raise LauncherError(str(exc)) from exc
        except OSError as exc:
            raise LauncherError(f"Не удалось открыть папку: {exc}") from exc

def _is_legacy_version(version_id: str) -> bool:
    match = re.search(r"1\.(\d+)(?:[.\-]|$)", version_id)
    return bool(match and int(match.group(1)) < 13)

def _looks_like_glx_context_error(lines: list[str]) -> bool:
    text = "\n".join(lines).lower()
    return any(
        marker in text
        for marker in (
            "glxbadfbconfig",
            "65543",
            "glx: failed to create context",
            "failed to create context",
            "no opengl context found",
        )
    )

def _safe_extra_arguments(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    result: list[str] = []
    for item in value[:32]:
        argument = str(item)
        if not argument or "\x00" in argument or "\n" in argument or "\r" in argument:
            continue
        result.append(argument[:512])
    return result

def _log_component(value: str) -> str:
    normalized = re.sub(r"[^A-Za-z0-9._-]+", "_", str(value)).strip("._-")
    return normalized[:80] or "launcher"

def _write_live_state(
    path: Path,
    *,
    running: bool,
    exit_code: int | None,
    pid: int,
    started_at: float = 0,
    log_id: str = "",
) -> None:
    try:
        if path.is_symlink():
            return
        flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(
                json.dumps(
                    {
                        "running": running,
                        "pid": pid,
                        "exit_code": exit_code,
                        "started_at": started_at,
                        "log_id": log_id,
                    }
                )
            )
    except OSError:
        pass

def _ensure_private_file(path: Path) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    try:
        set_private_fd(fd)
    finally:
        os.close(fd)

def _path_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False

def _expand_arguments(items: list[Any], placeholders: dict[str, str], features: dict[str, bool]) -> list[str]:
    expanded: list[str] = []
    for item in items:
        if isinstance(item, str):
            expanded.append(_replace_placeholders(item, placeholders))
            continue
        if not isinstance(item, dict) or not _argument_allowed(item.get("rules"), features):
            continue
        value = item.get("value", "")
        values = value if isinstance(value, list) else [value]
        expanded.extend(_replace_placeholders(str(item_value), placeholders) for item_value in values)
    return expanded

def _argument_allowed(rules: Any, features: dict[str, bool]) -> bool:
    if not rules:
        return True
    allowed = False
    for rule in rules:
        if not isinstance(rule, dict):
            continue
        operating_system = rule.get("os", {})
        if isinstance(operating_system, dict):
            current_os = mojang_os_name()
            if operating_system.get("name") and operating_system["name"] != current_os:
                continue
            architecture = platform.machine().lower()
            if architecture in {"x86_64", "amd64"}:
                architecture = "x86_64"
            elif architecture in {"aarch64", "arm64"}:
                architecture = "arm64"
            expected_arch = str(operating_system.get("arch", "")).lower()
            expected_values = {architecture}
            if architecture == "x86_64":
                expected_values.update({"amd64", "64"})
            if expected_arch and expected_arch not in expected_values:
                continue
        conditions = rule.get("features", {})
        if isinstance(conditions, dict) and any(features.get(name, False) != bool(expected) for name, expected in conditions.items()):
            continue
        allowed = rule.get("action") == "allow"
    return allowed

def _replace_placeholders(value: str, placeholders: dict[str, str]) -> str:
    for key, replacement in placeholders.items():
        value = value.replace("${" + key + "}", replacement)
    return value

def _redact_command(command: list[str], secrets: set[str]) -> list[str]:
    return [
        next((item.replace(secret, "<redacted>") for secret in secrets if secret and secret in item), item)
        for item in command
    ]

_JVM_SIZE = re.compile(r"^(\d+(?:\.\d+)?)([kKmMgG]?)$")

def _jvm_xmx_kib(value: str) -> int | None:
    """Размер из -Xmx в терминах JVM: 2G, 512m, 4096k."""
    match = _JVM_SIZE.match(value)
    if not match:
        return None
    number = float(match.group(1))
    unit = (match.group(2) or "m").lower()
    factor = {"k": 1, "m": 1024, "g": 1024 * 1024}[unit]
    return int(number * factor)

def _adapt_command_for_old_java(command: list[str], emit: Callable[[str], None] | None = None) -> list[str]:
    """правка команды под старые JVM, один запуск `java -version` на обе."""
    if not command:
        return command
    try:
        major, bitness = java_probe(Path(command[0]))
    except (OSError, ValueError):
        return command
    capped = command
    if bitness == 32:
        capped = _cap_heap_for_32bit_java(command, emit, bitness=bitness)
    if major is not None and major < 9 and "-XX:+IgnoreUnrecognizedVMOptions" not in capped:
        capped = ["-XX:+IgnoreUnrecognizedVMOptions", *capped]
    return capped

def _cap_heap_for_32bit_java(command: list[str], emit: Callable[[str], None] | None = None, *, bitness: int | None = None) -> list[str]:
    """32-битная Java не резервирует кучу больше ~1,5 ГБ — JVM умирает с"""
    if not command:
        return command
    if bitness is None:
        bitness = java_bitness(Path(command[0]))
    if bitness != 32:
        return command
    capped: list[str] = []
    adjusted = False
    for item in command:
        if item.startswith("-Xmx") and len(item) > 4:
            kib = _jvm_xmx_kib(item[4:])
            if kib is not None and kib > 1024 * 1024:
                capped.append("-Xmx1024M")
                adjusted = True
                continue
        capped.append(item)
    if adjusted and emit is not None:
        emit("Java 32-бит: память экземпляра уменьшена до 1024M — потолок этой сборки")
    return capped

def _apply_memory_setting(args: list[str], memory: str) -> list[str]:
    result = [value for value in args if not value.startswith("-Xmx")]
    if memory.strip():
        value = f"-Xmx{java_memory_value(memory)}"
        try:
            index = next(index for index, item in enumerate(result) if item in {"-cp", "-classpath"})
        except StopIteration:
            result.append(value)
        else:
            result.insert(index, value)
    return result

def _ensure_native_library_path(args: list[str], natives_dir: Path) -> list[str]:
    if any(value.startswith("-Djava.library.path=") for value in args):
        return args
    native_argument = f"-Djava.library.path={natives_dir}"
    try:
        index = next(index for index, item in enumerate(args) if item in {"-cp", "-classpath"})
    except StopIteration:
        args.append(native_argument)
    else:
        args.insert(index, native_argument)
    return args
