from __future__ import annotations

import errno
import hashlib
import os
import re
from pathlib import Path

from .errors import LauncherError

try:
    import fcntl
except ImportError:  # pragma: no cover - the desktop target is POSIX
    fcntl = None

try:
    import msvcrt
except ImportError:  # pragma: no cover - the desktop target is Windows
    msvcrt = None

class InstanceProcessLock:
    """Keep one client or server process per instance across launcher workers."""

    def __init__(self, data_home: Path, instance_name: str) -> None:
        safe_name = re.sub(r"[^A-Za-z0-9._-]+", "_", instance_name).strip("._-") or "instance"
        name_hash = hashlib.sha256(instance_name.encode("utf-8")).hexdigest()[:16]
        self.path = data_home / ".locks" / f"{safe_name[:64]}-{name_hash}.lock"
        self._handle: int | None = None
        self._lock_backend: str | None = None

    def acquire(self) -> None:
        if self._handle is not None:
            return
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
        try:
            handle = os.open(self.path, flags, 0o600)
        except OSError as exc:
            raise LauncherError(f"Не удалось открыть блокировку экземпляра: {exc}") from exc
        if fcntl is not None:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as exc:
                os.close(handle)
                if exc.errno in {errno.EACCES, errno.EAGAIN}:
                    raise LauncherError("Экземпляр уже запущен") from exc
                raise LauncherError(f"Не удалось заблокировать экземпляр: {exc}") from exc
            self._lock_backend = "fcntl"
        elif msvcrt is not None:
            try:
                if os.fstat(handle).st_size == 0:
                    os.write(handle, b"\0")
                os.lseek(handle, 0, os.SEEK_SET)
                msvcrt.locking(handle, msvcrt.LK_NBLCK, 1)
                self._lock_backend = "msvcrt"
            except OSError as exc:
                os.close(handle)
                self._lock_backend = None
                if exc.errno in {errno.EACCES, errno.EAGAIN, errno.EDEADLK, 13, 36}:
                    raise LauncherError("Экземпляр уже запущен") from exc
                raise LauncherError(f"Не удалось заблокировать экземпляр: {exc}") from exc
        self._handle = handle

    def release(self) -> None:
        handle, self._handle = self._handle, None
        if handle is None:
            return
        if self._lock_backend == "fcntl" and fcntl is not None:
            try:
                fcntl.flock(handle, fcntl.LOCK_UN)
            except OSError:
                pass
        elif self._lock_backend == "msvcrt" and msvcrt is not None:
            try:
                os.lseek(handle, 0, os.SEEK_SET)
                msvcrt.locking(handle, msvcrt.LK_UNLCK, 1)
            except OSError:
                pass
        self._lock_backend = None
        try:
            os.close(handle)
        except OSError:
            pass
