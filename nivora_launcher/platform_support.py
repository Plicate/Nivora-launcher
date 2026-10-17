from __future__ import annotations

import os
import shutil
import signal
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any


def set_private_fd(fd: int, mode: int = 0o600) -> None:
    """Apply POSIX mode bits where supported; Windows uses the profile ACL."""
    fchmod = getattr(os, "fchmod", None)
    if fchmod is not None:
        fchmod(fd, mode)


def set_private_path(path: Path) -> None:
    """Windows: ACL файла только для текущего пользователя (аналог 0600).

    POSIX-режим на Windows не действует, а каталог данных наследует ACL
    профиля. Ошибка не прерывает запуск: остаётся ACL профиля пользователя.
    """
    if sys.platform != "win32":
        return
    from .windows_sandbox import WindowsSandboxError, restrict_to_current_user

    try:
        restrict_to_current_user(path)
    except (WindowsSandboxError, OSError, subprocess.SubprocessError, AttributeError, ValueError):
        # AttributeError: в урезанных окружениях (Wine, старые сборки Windows)
        # нет части Win32 API — запуск игры из-за ACL срываться не должен.
        pass


def is_reparse_point(path: Path) -> bool:
    try:
        value = path.lstat()
    except FileNotFoundError:
        return False
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return path.is_symlink() or bool(getattr(value, "st_file_attributes", 0) & reparse_flag)


def process_exists(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        import psutil

        return psutil.pid_exists(pid)
    except ImportError:
        pass
    if os.name == "nt":
        try:
            import ctypes

            kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
            open_process = kernel32.OpenProcess
            open_process.argtypes = (ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32)
            open_process.restype = ctypes.c_void_p
            close_handle = kernel32.CloseHandle
            close_handle.argtypes = (ctypes.c_void_p,)
            close_handle.restype = ctypes.c_int
            handle = open_process(0x1000, 0, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
            if handle:
                close_handle(handle)
                return True
            return ctypes.get_last_error() == 5  # Access denied still means the PID exists.
        except (AttributeError, OSError):
            return False
    try:
        os.kill(pid, 0)
        return True
    except PermissionError:
        return True
    except (OSError, ProcessLookupError):
        return False


def process_start_options() -> dict[str, Any]:
    if os.name == "nt":
        flags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0) | getattr(subprocess, "CREATE_NO_WINDOW", 0)
        return {"creationflags": flags}
    return {"start_new_session": True}


def terminate_process_tree(process: Any, *, force: bool = False) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        taskkill = shutil.which("taskkill.exe") or shutil.which("taskkill")
        if taskkill:
            try:
                subprocess.run(
                    [taskkill, "/PID", str(process.pid), "/T", "/F"],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=5,
                    check=False,
                )
                return
            except (OSError, subprocess.SubprocessError):
                pass
        try:
            process.kill() if force else process.terminate()
        except OSError:
            pass
        return

    try:
        os.killpg(process.pid, signal.SIGKILL if force else signal.SIGTERM)
    except (OSError, ProcessLookupError, AttributeError):
        try:
            process.kill() if force else process.terminate()
        except OSError:
            pass


def open_path(path: Path) -> None:
    if os.name == "nt":
        os.startfile(str(path))  # type: ignore[attr-defined]
        return
    if sys_platform() == "darwin":
        subprocess.Popen(["open", str(path)])
        return
    opener = shutil.which("xdg-open")
    if not opener:
        raise RuntimeError("Не найден xdg-open. Установите пакет xdg-utils.")
    subprocess.Popen([opener, str(path)])


def sys_platform() -> str:
    import sys

    return sys.platform
