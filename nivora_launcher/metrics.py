from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import Any

from .config import _atomic_json_write
from .constants import MAX_INSTANCE_LIVE_STATE_JSON_BYTES
from .errors import LauncherError


METRICS_FILE = ".trial-launcher-metrics.json"
_MAX_MEASURED_FILES = 20_000
_MAX_SAMPLED_PROCESSES = 1024


def directory_size(root: Path, *, exclude_names: set[str] | None = None) -> int:
    excluded = exclude_names or {".trial-launcher-snapshots", "logs", "cache"}
    total = 0
    count = 0
    try:
        for path in root.rglob("*"):
            if count >= _MAX_MEASURED_FILES:
                break
            if any(part in excluded for part in path.parts):
                continue
            if path.is_file() and not path.is_symlink():
                try:
                    total += path.stat().st_size
                    count += 1
                except OSError:
                    continue
    except OSError:
        return total
    return total


def _process_sample(pid: int, *, proc_root: Path = Path("/proc")) -> tuple[float, int] | None:
    if pid <= 0:
        return None
    if os.name == "nt":
        return _windows_process_sample(pid)
    if os.name != "posix":
        return None
    pending = [pid]
    seen: set[int] = set()
    root_found = False
    cpu_seconds_total = 0.0
    ram_bytes_total = 0
    ticks = float(os.sysconf("SC_CLK_TCK"))
    while pending and len(seen) < _MAX_SAMPLED_PROCESSES:
        current = pending.pop()
        if current <= 0 or current in seen:
            continue
        seen.add(current)
        proc = proc_root / str(current)
        sample = _single_process_sample(proc, ticks)
        if current == pid:
            if sample is None:
                return None
            root_found = True
        if sample is not None:
            cpu_seconds_total += sample[0]
            ram_bytes_total += sample[1]
        try:
            children_path = proc / "task" / str(current) / "children"
            pending.extend(int(value) for value in children_path.read_text(encoding="ascii").split())
        except (OSError, ValueError):
            continue
    if not root_found:
        return None
    return cpu_seconds_total, ram_bytes_total


def _windows_process_sample(pid: int) -> tuple[float, int] | None:
    try:
        import psutil

        root = psutil.Process(pid)
        processes = [root, *root.children(recursive=True)]
        cpu_seconds = 0.0
        ram_bytes = 0
        found_root = False
        for process in processes:
            try:
                times = process.cpu_times()
                memory = process.memory_info()
            except psutil.Error:
                if process.pid == pid:
                    return None
                continue
            if process.pid == pid:
                found_root = True
            cpu_seconds += float(times.user) + float(times.system)
            ram_bytes += int(memory.rss)
        return (cpu_seconds, ram_bytes) if found_root else None
    except (ImportError, OSError, ValueError):
        return None


def _single_process_sample(proc: Path, ticks: float) -> tuple[float, int] | None:
    try:
        stat_line = (proc / "stat").read_text(encoding="utf-8")
        closing = stat_line.rfind(")")
        fields = stat_line[closing + 2 :].split()
        if len(fields) < 13:
            return None
        cpu_seconds = (float(fields[11]) + float(fields[12])) / max(1.0, ticks)
        status = (proc / "status").read_text(encoding="utf-8", errors="replace")
        rss_kib = 0
        for line in status.splitlines():
            if line.startswith("VmRSS:"):
                try:
                    rss_kib = int(line.split()[1])
                except (IndexError, ValueError):
                    rss_kib = 0
                break
        return cpu_seconds, max(0, rss_kib) * 1024
    except (OSError, ValueError, IndexError):
        return None


def read_metrics(root: Path) -> dict[str, Any]:
    path = root / METRICS_FILE
    if path.is_symlink() or not path.exists():
        return {}
    try:
        if path.stat().st_size > 512 * 1024:
            return {}
        value = json.loads(path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError, TypeError, json.JSONDecodeError):
        return {}


def write_last_snapshot(root: Path, snapshot: dict[str, Any]) -> None:
    if root.is_symlink() or not root.is_dir():
        raise LauncherError("Каталог метрик экземпляра недоступен")
    _atomic_json_write(root / METRICS_FILE, {"last": snapshot}, private=True)


class ProcessMetricsTracker:
    def __init__(self, root: Path, pid: int, *, started_at: float | None = None) -> None:
        self.root = root
        self.pid = pid
        self.started_at = started_at or time.time()
        self.started_monotonic = time.monotonic() - max(0.0, time.time() - self.started_at)
        self.previous_cpu: float | None = None
        self.previous_at: float | None = None
        self.peak_ram = 0
        self.peak_cpu = 0.0
        self.storage_bytes = 0
        self.storage_sampled_at = 0.0

    def sample(self, *, running: bool = True, players: int = 0, max_players: int = 0, tps: float = 0.0) -> dict[str, Any]:
        now = time.time()
        monotonic_now = time.monotonic()
        process = _process_sample(self.pid)
        cpu_percent = 0.0
        ram_bytes = 0
        if process:
            cpu_seconds, ram_bytes = process
            if self.previous_cpu is not None and self.previous_at is not None:
                elapsed = max(0.05, monotonic_now - self.previous_at)
                cpu_percent = max(0.0, min(999.0, (cpu_seconds - self.previous_cpu) / elapsed * 100.0))
            self.previous_cpu = cpu_seconds
            self.previous_at = monotonic_now
        self.peak_ram = max(self.peak_ram, ram_bytes)
        self.peak_cpu = max(self.peak_cpu, cpu_percent)
        if now - self.storage_sampled_at >= 5 or not self.storage_sampled_at:
            self.storage_bytes = directory_size(self.root)
            self.storage_sampled_at = now
        return {
            "timestamp": now,
            "pid": self.pid,
            "running": bool(running and process is not None),
            "uptime_seconds": max(0, int(monotonic_now - self.started_monotonic)),
            "peak_ram_bytes": self.peak_ram,
            "peak_cpu_percent": round(self.peak_cpu, 2),
            "ram_bytes": ram_bytes,
            "cpu_percent": round(cpu_percent, 2),
            "storage_bytes": self.storage_bytes,
            "players": max(0, int(players)),
            "max_players": max(0, int(max_players)),
            "tps": round(max(0.0, float(tps)), 2),
        }

    def finish(self, *, exit_code: int | None = None) -> dict[str, Any]:
        snapshot = self.sample(running=False)
        snapshot.update(
            {
                "ended_at": time.time(),
                "exit_code": exit_code,
                "peak_ram_bytes": self.peak_ram,
                "peak_cpu_percent": round(self.peak_cpu, 2),
            }
        )
        write_last_snapshot(self.root, snapshot)
        return snapshot


def metrics_for_instance(root: Path, *, pid: int = 0, started_at: float = 0.0) -> dict[str, Any]:
    if pid <= 0:
        live_state = root / ".trial-launcher-live.json"
        try:
            if (
                live_state.is_symlink()
                or not live_state.is_file()
                or live_state.stat().st_size > MAX_INSTANCE_LIVE_STATE_JSON_BYTES
            ):
                raise OSError("instance live state is unavailable")
            raw = json.loads(live_state.read_text(encoding="utf-8"))
            if not isinstance(raw, dict):
                raise ValueError("instance live state is not an object")
            pid = int(raw.get("pid", 0)) if raw.get("running") else 0
            started_at = float(raw.get("started_at", 0))
        except (OSError, ValueError, TypeError, RecursionError):
            pid = 0
    if pid > 0:
        tracker = ProcessMetricsTracker(root, pid, started_at=started_at or None)
        return tracker.sample(running=_process_sample(pid) is not None)
    return {
        "timestamp": time.time(),
        "pid": 0,
        "running": False,
        "uptime_seconds": 0,
        "ram_bytes": 0,
        "cpu_percent": 0.0,
        "storage_bytes": directory_size(root),
        "players": 0,
        "max_players": 0,
        "tps": 0.0,
    }
