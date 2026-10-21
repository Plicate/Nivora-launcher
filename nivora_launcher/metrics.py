from __future__ import annotations

import json
import os
import re
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

from .config import _atomic_json_write
from .constants import MAX_INSTANCE_LIVE_STATE_JSON_BYTES
from .errors import LauncherError

METRICS_FILE = ".trial-launcher-metrics.json"
_MAX_MEASURED_FILES = 20_000
_MAX_SAMPLED_PROCESSES = 1024

# GPU-сэмплинг дорог (запуск nvidia-smi / чтение fdinfo) — кешируем на пару секунд.
_GPU_CACHE: dict[int, tuple[float, dict[str, Any]]] = {}
_GPU_CACHE_SECONDS = 2.0
_GPU_CACHE_LOCK = threading.Lock()

_SESSION_EVENT_MARKERS: tuple[tuple[str, str], ...] = (
    (r"Backend library: LWJGL", "LWJGL инициализирован"),
    (r"Backend library: OpenGL", "OpenGL-контекст создан"),
    (r"Setting user:", "Профиль игрока применён"),
    (r"Reloading ResourceManager", "Загрузка ресурсов"),
    (r"OpenAL initialized", "Звук (OpenAL) готов"),
    (r"Sound engine started", "Звуковой движок запущен"),
    (r"Created: \d+x\d+x\d+ [\w:/.-]+atlas", "Атлас текстур создан"),
    (r"Loading \d+ mods?", "Загрузка модов"),
    (r"Loaded \d+ mods?", "Моды загружены"),
    (r"Forge Mod Loader", "Forge загружается"),
    (r"Preparing start region", "Мир: подготовка стартового региона"),
    (r"Starting integrated server", "Запуск встроенного сервера (мир открыт)"),
    (r"Joining worker", "Подключение к серверу"),
    (r"Stopping!", "Игра останавливается"),
)
_LOG_TIME_PREFIX = re.compile(r"\[?(\d{2}):(\d{2}):(\d{2})[.,]?(\d{1,3})?\]?")

class SessionEvents:
    """извлекает из лога игры события жизненного цикла и активность модов."""

    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []
        self.mod_stats: dict[str, dict[str, Any]] = {}
        self._seen_markers: set[str] = set()
        self._last_line: str | None = None

    def feed(self, lines: list[str]) -> None:
        if not lines:
            return
        if self._last_line is not None:
            try:
                # хвост лога ограничен (последние N строк): считаем новыми
                # все строки после последнего знакомого совпадения.
                position = len(lines) - 1 - lines[::-1].index(self._last_line)
            except ValueError:
                position = -1
            fresh = lines[position + 1 :]
        else:
            fresh = list(lines)
        self._last_line = lines[-1]
        for line in fresh:
            self._consume(line)

    def snapshot(self) -> dict[str, Any]:
        heaviest = sorted(
            self.mod_stats.values(),
            key=lambda item: (item["errors"] * 8 + item["warnings"] * 3 + item["lines"] // 4),
            reverse=True,
        )
        return {"events": list(self.events), "mods": heaviest}

    def _consume(self, line: str) -> None:
        now = time.time()
        for pattern, title in _SESSION_EVENT_MARKERS:
            if pattern in self._seen_markers:
                continue
            if re.search(pattern, line):
                self._seen_markers.add(pattern)
                self.events.append({"at": now, "title": title})
                break
        mod_id = _mod_id_from_line(line)
        if mod_id:
            entry = self.mod_stats.setdefault(mod_id, {"mod": mod_id, "lines": 0, "warnings": 0, "errors": 0, "first_seen": now})
            entry["lines"] += 1
            if re.search(r"/ERROR\]|\(FATAL\)|SEVERE", line):
                entry["errors"] += 1
            elif re.search(r"/WARN\]", line):
                entry["warnings"] += 1

def _mod_id_from_line(line: str) -> str | None:
    # fabric: «[main/INFO] (modid) сообщение», Forge/NeoForge: «[main/INFO] [modid/] ...».
    match = re.search(r"\] \[([a-z0-9_-]{2,32})/\]", line) or re.search(r"\] \(([a-z0-9_-]{2,32})\)", line)
    if match and match.group(1) not in {"main", "worker", "render", "server", "system", "minecraft"}:
        return match.group(1)
    return None

def _run_nvidia_pmon(pid: int) -> dict[str, Any] | None:
    try:
        result = subprocess.run(
            ["nvidia-smi", "pmon", "-c", "1", "-s", "um"],
            capture_output=True,
            text=True,
            timeout=3,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    for line in result.stdout.splitlines():
        if line.lstrip().startswith("#"):
            continue
        sample = _parse_pmon_row(line, pid)
        if sample is not None:
            return sample
    return None

def _parse_pmon_row(line: str, pid: int) -> dict[str, Any] | None:
    fields = line.split()
    if len(fields) < 6 or not fields[1].isdigit() or int(fields[1]) != pid:
        return None
    return {
        "util_percent": _optional_float(fields[3]),
        "memory_bytes": _megabyte_string_to_bytes(fields[4]),
        "source": "nvidia",
    }

def _optional_float(value: str) -> float | None:
    try:
        return float(value)
    except ValueError:
        return None

def _megabyte_string_to_bytes(value: str) -> int | None:
    try:
        return int(float(value) * 1024 * 1024)
    except ValueError:
        return None

def _drm_fdinfo_sample(pid: int) -> dict[str, Any] | None:
    # mesa-стек (amdgpu/i915/panfrost...) пишет наработку движков DRM в
    if os.name != "posix":
        return None
    gfx_ns = 0
    vram = 0
    try:
        fd_dir = Path(f"/proc/{pid}/fd")
        for descriptor in list(fd_dir.iterdir())[:256]:
            try:
                target = os.readlink(descriptor)
            except OSError:
                continue
            if "/dev/dri/" not in target:
                continue
            try:
                content = (Path(f"/proc/{pid}/fdinfo/{descriptor.name}")).read_text(encoding="ascii", errors="replace")
            except OSError:
                continue
            for info_line in content.splitlines():
                if info_line.startswith("drm-engine-gfx:"):
                    try:
                        gfx_ns += int(info_line.split(":")[1].split()[0])
                    except (IndexError, ValueError):
                        pass
                elif info_line.startswith("drm-memory-vram:") or info_line.startswith("drm-memory-gtt:"):
                    try:
                        vram += int(info_line.split(":")[1].split()[0])
                    except (IndexError, ValueError):
                        pass
    except OSError:
        return None
    if not gfx_ns and not vram:
        return None
    return {"gfx_ns": gfx_ns, "vram_bytes": vram}

def gpu_process_sample(pid: int) -> dict[str, Any]:
    """Нагрузка GPU конкретным процессом игры. Недоступно — все поля null."""
    if pid <= 0:
        return {"util_percent": None, "memory_bytes": None, "source": "none"}
    now = time.monotonic()
    with _GPU_CACHE_LOCK:
        cached = _GPU_CACHE.get(pid)
        if cached and now - cached[0] < _GPU_CACHE_SECONDS:
            return cached[1]
    sample: dict[str, Any] | None = _run_nvidia_pmon(pid)
    if sample is None:
        drm = _drm_fdinfo_sample(pid)
        if drm:
            previous = getattr(gpu_process_sample, "_drm_previous", {}).get(pid)
            util = None
            if previous and now > previous[0]:
                elapsed = now - previous[0]
                delta_ns = max(0, drm["gfx_ns"] - previous[1])
                util = round(min(100.0, delta_ns / (elapsed * 1e9) * 100.0), 1)
            history = getattr(gpu_process_sample, "_drm_previous", {})
            history[pid] = (now, drm["gfx_ns"])
            gpu_process_sample._drm_previous = history  # type: ignore[attr-defined]
            sample = {"util_percent": util, "memory_bytes": drm["vram_bytes"] or None, "source": "drm"}
    if sample is None:
        sample = {"util_percent": None, "memory_bytes": None, "source": "none"}
    with _GPU_CACHE_LOCK:
        _GPU_CACHE[pid] = (now, sample)
    return sample

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
    # сколько замеров храним для средних значений (при опросе раз в секунду
    # это ~10 минут истории).
    MAX_HISTORY = 600

    def __init__(self, root: Path, pid: int, *, started_at: float | None = None) -> None:
        self.root = root
        self.pid = pid
        self.started_at = started_at or time.time()
        self.started_monotonic = time.monotonic() - max(0.0, time.time() - self.started_at)
        self.previous_cpu: float | None = None
        self.previous_at: float | None = None
        self.peak_ram = 0
        self.peak_cpu = 0.0
        self.peak_gpu: float | None = 0.0
        self.storage_bytes = 0
        self.storage_sampled_at = 0.0
        self.cpu_history: list[float] = []
        self.ram_history: list[int] = []
        self.gpu_history: list[float] = []
        self.session_events = SessionEvents()

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
        gpu = gpu_process_sample(self.pid if process else 0)
        self.peak_ram = max(self.peak_ram, ram_bytes)
        self.peak_cpu = max(self.peak_cpu, cpu_percent)
        if gpu.get("util_percent") is not None:
            self.peak_gpu = max(self.peak_gpu or 0.0, float(gpu["util_percent"]))
        if now - self.storage_sampled_at >= 5 or not self.storage_sampled_at:
            self.storage_bytes = directory_size(self.root)
            self.storage_sampled_at = now
        self.cpu_history = (self.cpu_history + [cpu_percent])[-self.MAX_HISTORY :]
        self.ram_history = (self.ram_history + [ram_bytes])[-self.MAX_HISTORY :]
        self.gpu_history = (self.gpu_history + [float(gpu.get("util_percent") or 0.0)])[-self.MAX_HISTORY :]
        return {
            "timestamp": now,
            "pid": self.pid,
            "running": bool(running and process is not None),
            "uptime_seconds": max(0, int(monotonic_now - self.started_monotonic)),
            "peak_ram_bytes": self.peak_ram,
            "peak_cpu_percent": round(self.peak_cpu, 2),
            "peak_gpu_percent": round(self.peak_gpu, 2) if self.peak_gpu is not None else None,
            "avg_cpu_percent": _average(self.cpu_history),
            "avg_ram_bytes": int(_average(self.ram_history)),
            "avg_gpu_percent": _average(self.gpu_history),
            "ram_bytes": ram_bytes,
            "cpu_percent": round(cpu_percent, 2),
            "gpu": gpu,
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
                "session": self.session_events.snapshot(),
            }
        )
        write_last_snapshot(self.root, snapshot)
        return snapshot

def _average(values: list[float]) -> float | None:
    if not values:
        return None
    return round(sum(values) / len(values), 2)

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
