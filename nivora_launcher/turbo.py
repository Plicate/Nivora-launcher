"""пакетное SHA-256 через внешний бинарник nivora-turbo (cpp-turbo/)."""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

_TURBO_TIMEOUT_SECONDS = 600
# меньше этого размера пачка не стоит запуска процесса.
_MIN_BATCH_FILES = 4
_BINARY_NAMES = ("nivora-turbo", "nivora-turbo.exe")

_cached_path: str | None = None
_cached_resolved = False

def turbo_binary() -> str | None:
    global _cached_path, _cached_resolved
    if _cached_resolved:
        return _cached_path
    _cached_resolved = True

    env_value = os.environ.get("NIVORA_TURBO_BIN", "").strip()
    if env_value and os.path.isfile(env_value) and os.access(env_value, os.X_OK):
        _cached_path = env_value
        return _cached_path

    # рядом с бэкендом: packaged-сборка кладёт helper в тот же каталог.
    siblings = []
    executable = Path(sys.executable)
    siblings.append(executable.parent)
    # python -m nivora_launcher: реальный entry point лежит в argv[0]
    if sys.argv:
        entry = Path(sys.argv[0]).resolve()
        if entry.parent != executable.parent:
            siblings.append(entry.parent)
    for directory in siblings:
        for name in _BINARY_NAMES:
            candidate = directory / name
            if candidate.is_file() and os.access(candidate, os.X_OK):
                _cached_path = str(candidate)
                return _cached_path

    found = shutil.which("nivora-turbo")
    _cached_path = found
    return _cached_path

def sha256_batch(paths: list[Path]) -> dict[Path, tuple[str, int]] | None:
    """хеширует список файлов одним запуском nivora-turbo."""
    binary = turbo_binary()
    if binary is None or len(paths) < _MIN_BATCH_FILES:
        return None
    unique = list(dict.fromkeys(paths))
    try:
        result = subprocess.run(
            [binary, "sha256", *(str(path) for path in unique)],
            capture_output=True,
            text=True,
            timeout=_TURBO_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    digests: dict[Path, tuple[str, int]] = {}
    for line in result.stdout.splitlines():
        parts = line.split("\t", 2)
        if len(parts) != 3:
            continue
        digest, size, raw_path = parts
        if digest == "-" or not digest:
            return None
        try:
            digests[Path(raw_path)] = (digest.lower(), int(size))
        except ValueError:
            return None
    if len(digests) != len(unique):
        return None
    return digests
