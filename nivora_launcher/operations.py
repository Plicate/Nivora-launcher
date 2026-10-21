from __future__ import annotations

import tempfile
import time
import os
from pathlib import Path

from .errors import LauncherError

class OperationCancelled(LauncherError):
    """Raised when the user cancels an in-flight GUI operation."""

    def __init__(self) -> None:
        super().__init__("Операция остановлена пользователем")

def marker_path(operation_id: str) -> Path:
    safe_id = "".join(char for char in operation_id if char.isalnum() or char in "-_")
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", "")) if os.environ.get("XDG_RUNTIME_DIR") else Path(tempfile.gettempdir()) / "trial-launcher-runtime"
    return runtime / f"trial-launcher-{safe_id}.cancel"

def _legacy_marker_path(operation_id: str) -> Path:
    safe_id = "".join(char for char in operation_id if char.isalnum() or char in "-_")
    return Path(tempfile.gettempdir()) / f"trial-launcher-{safe_id}.cancel"

class CancellationToken:
    def __init__(self, operation_id: str = "") -> None:
        self.operation_id = operation_id.strip()
        self.path = marker_path(self.operation_id) if self.operation_id else None

    def cancelled(self) -> bool:
        return bool(self.path and (self.path.exists() or _legacy_marker_path(self.operation_id).exists()))

    def check(self) -> None:
        if self.cancelled():
            raise OperationCancelled()

    def wait(self, seconds: float) -> None:
        deadline = time.monotonic() + max(0.0, seconds)
        while True:
            self.check()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            time.sleep(min(0.1, remaining))

    def cleanup(self) -> None:
        if not self.path:
            return
        try:
            self.path.unlink()
        except FileNotFoundError:
            pass
        try:
            _legacy_marker_path(self.operation_id).unlink()
        except FileNotFoundError:
            pass
