from __future__ import annotations

import json
import re
import threading
import time
from pathlib import Path
from typing import Any, Callable

from .errors import LauncherError
from .installer import MinecraftInstaller
from .metrics import _process_sample, gpu_process_sample

MENU_MARKER = "Sound engine started"
MENU_WAIT_SECONDS = 150
SAMPLE_SECONDS = 15

_ACTIVE: dict[str, dict[str, Any]] = {}
_LOCK = threading.Lock()

def benchmark_state(instance_name: str) -> dict[str, Any]:
    with _LOCK:
        state = _ACTIVE.get(instance_name)
        if not state:
            return {"phase": "idle", "available": True}
        snapshot = dict(state)
        snapshot["available"] = False
        return snapshot

def start_benchmark(
    instance_name: str,
    instance_path: Path,
    minecraft_version: str,
    shader_file: str,
    launcher_factory: Callable[[], Any],
    install_result_factory: Callable[[], Any],
    session_append: Callable[[str, str], None],
    session_lines: Callable[[str], list[str]],
) -> None:
    with _LOCK:
        if instance_name in _ACTIVE:
            raise LauncherError("Бенчмарк шейдеров уже идёт для этого экземпляра")
        _ACTIVE[instance_name] = {"phase": "prepare", "progress": 0, "message": "Готовлю запуск…"}

    thread = threading.Thread(
        target=_run_benchmark,
        args=(instance_name, instance_path, minecraft_version, shader_file, launcher_factory, install_result_factory, session_append, session_lines),
        daemon=True,
        name=f"shader-benchmark-{instance_name}",
    )
    thread.start()

def _update(instance_name: str, **values: Any) -> None:
    with _LOCK:
        state = _ACTIVE.setdefault(instance_name, {})
        state.update(values)

def _finish(instance_name: str, error: str = "") -> None:
    with _LOCK:
        state = _ACTIVE.setdefault(instance_name, {})
        state["phase"] = "failed" if error else "done"
        state["progress"] = 100
        if error:
            state["error"] = error
        state["ended_at"] = time.time()

def _run_benchmark(
    instance_name: str,
    instance_path: Path,
    minecraft_version: str,
    shader_file: str,
    launcher_factory: Callable[[], Any],
    install_result_factory: Callable[[], Any],
    session_append: Callable[[str, str], None],
    session_lines: Callable[[str], list[str]],
) -> None:
    options_path = instance_path / "options.txt"
    original = _read_options(options_path)
    try:
        # фаза 1: без шейдера — убираем выбор шейдерпака из options.txt.
        _write_options(options_path, apply_shader_selection(original, None, minecraft_version))
        _update(instance_name, phase="baseline", progress=10, message="Запускаю игру без шейдера…")
        baseline = _measure_phase(
            instance_name, "baseline", launcher_factory, install_result_factory,
            session_append, session_lines,
        )
        # фаза 2: с шейдером.
        _write_options(options_path, apply_shader_selection(original, shader_file, minecraft_version))
        _update(instance_name, phase="shader", progress=55, message="Запускаю игру с шейдером…", baseline=baseline)
        with_shader = _measure_phase(
            instance_name, "shader", launcher_factory, install_result_factory,
            session_append, session_lines,
        )
        _update(
            instance_name,
            phase="done",
            progress=100,
            baseline=baseline,
            shader=with_shader,
            verdict=_compare(baseline, with_shader),
            message="Бенчмарк завершён",
        )
    except LauncherError as error:
        _finish(instance_name, str(error))
    except Exception as error:  # noqa: BLE001 — фоновый поток: любая ошибка уходит в статус
        _finish(instance_name, f"Непредвиденная ошибка бенчмарка: {error}")
    finally:
        _write_options(options_path, original)

def _measure_phase(
    instance_name: str,
    phase_label: str,
    launcher_factory: Callable[[], Any],
    install_result_factory: Callable[[], Any],
    session_append: Callable[[str, str], None],
    session_lines: Callable[[str], list[str]],
) -> dict[str, Any]:
    launcher = launcher_factory()
    install = install_result_factory()
    output = lambda line: session_append(instance_name, line)  # noqa: E731 — короткая обёртка

    def target() -> None:
        try:
            launcher.run(install, _benchmark_account(launcher), output=output, settings={})
        except Exception:
            pass

    worker = threading.Thread(target=target, daemon=True, name=f"shader-benchmark-{instance_name}-{phase_label}")
    worker.start()

    started = time.monotonic()
    menu_seen = False
    while time.monotonic() - started < MENU_WAIT_SECONDS:
        if not worker.is_alive():
            raise LauncherError(f"Игра закрылась раньше главного меню ({phase_label})")
        joined = "\n".join(session_lines(instance_name))
        if MENU_MARKER in joined or "Reloading ResourceManager" in joined and "Created: " in joined:
            menu_seen = True
            break
        time.sleep(1.0)
    if not menu_seen:
        _stop_game(launcher)
        raise LauncherError(f"Главное меню не появилось за {MENU_WAIT_SECONDS} с ({phase_label})")

    _update(instance_name, message=f"Замеряю нагрузку ({phase_label})…")
    samples = 0
    cpu_total = 0.0
    gpu_total = 0.0
    gpu_mem_peak = 0
    ram_peak = 0
    previous_cpu: float | None = None
    previous_at: float | None = None
    measure_start = time.monotonic()
    while time.monotonic() - measure_start < SAMPLE_SECONDS:
        process = getattr(launcher, "process", None)
        pid = process.pid if process else 0
        sample = _process_sample(pid) if pid else None
        if sample:
            cpu_seconds, ram_bytes = sample
            now = time.monotonic()
            if previous_cpu is not None and previous_at is not None:
                elapsed = max(0.05, now - previous_at)
                cpu_total += max(0.0, (cpu_seconds - previous_cpu) / elapsed * 100.0)
                samples += 1
            previous_cpu = cpu_seconds
            previous_at = now
            ram_peak = max(ram_peak, ram_bytes)
        gpu = gpu_process_sample(pid if sample else 0)
        if gpu.get("util_percent") is not None:
            gpu_total += float(gpu["util_percent"])
        if gpu.get("memory_bytes"):
            gpu_mem_peak = max(gpu_mem_peak, int(gpu["memory_bytes"]))
        time.sleep(1.0)

    _stop_game(launcher)
    worker.join(timeout=10)
    if samples:
        return {
            "cpu_percent": round(cpu_total / samples, 1),
            "ram_mb": round(ram_peak / (1024 * 1024), 1),
            "gpu_percent": round(gpu_total / SAMPLE_SECONDS, 1) if gpu_total else None,
            "gpu_memory_mb": round(gpu_mem_peak / (1024 * 1024), 1) if gpu_mem_peak else None,
            "samples": samples,
        }
    return {"cpu_percent": None, "ram_mb": 0.0, "gpu_percent": None, "gpu_memory_mb": None, "samples": 0}

def _benchmark_account(launcher: Any) -> Any:
    account = launcher.accounts.default()
    if account is None:
        account = launcher.offline.get()
    return account

def _stop_game(launcher: Any) -> None:
    process = getattr(launcher, "process", None)
    if process is None or process.poll() is not None:
        return
    try:
        process.terminate()
        process.wait(timeout=8)
    except Exception:
        try:
            process.kill()
        except Exception:
            pass

def _compare(baseline: dict[str, Any], with_shader: dict[str, Any]) -> dict[str, Any]:
    def load(sample: dict[str, Any]) -> float:
        for key in ("gpu_percent", "cpu_percent"):
            value = sample.get(key)
            if value is not None:
                return float(value)
        return 0.0

    before = load(baseline)
    after = load(with_shader)
    delta = max(0.0, after - before)
    if after >= 95.0:
        label, tone = "Видеокарта упирается в потолок — шейдер слишком тяжёлый", "danger"
    elif delta >= 40.0:
        label, tone = "Тяжёлый шейдер: нагрузка на карту выросла сильно", "warning"
    elif delta >= 15.0:
        label, tone = "Заметная нагрузка — играбельно на средних настройках", "warning"
    else:
        label, tone = "Лёгкий для этой системы", "success"
    return {
        "label": label,
        "tone": tone,
        "gpu_delta_percent": round(delta, 1),
        "metric": "gpu_percent" if baseline.get("gpu_percent") is not None else "cpu_percent",
    }

def _read_options(path: Path) -> str:
    try:
        if path.is_symlink() or not path.is_file():
            return ""
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""

def _write_options(path: Path, content: str) -> None:
    try:
        if content:
            path.write_text(content, encoding="utf-8")
        elif path.exists():
            path.unlink()
    except OSError as error:
        raise LauncherError(f"Не удалось вернуть options.txt: {error}") from error

def _shader_options_line(minecraft_version: str, shader_file: str) -> str:
    # 1.21.4+ хранит выбор шейдера JSON-строкой, старые — простым значением.
    match = re.match(r"(\d+)\.(\d+)(?:\.(\d+))?", minecraft_version)
    modern = False
    if match:
        major, minor = int(match.group(1)), int(match.group(2))
        modern = (major, minor) >= (1, 21) and (minor > 21 or minecraft_version.strip() in {"1.21.4", "1.21.5", "1.21.6", "1.21.7", "1.21.8", "1.21.9", "1.21.10", "1.21.11"})
    if modern:
        payload = json.dumps({"id": "file", "file": shader_file}, ensure_ascii=False)
        return f"shaderPack:{payload}"
    return f"shaderPack:{shader_file}"

def apply_shader_selection(options_content: str, shader_file: str | None, minecraft_version: str) -> str:
    """Убирает или выставляет активный шейдерпак в options.txt."""
    lines = [line for line in options_content.splitlines() if not line.startswith("shaderPack:")]
    if shader_file:
        lines.append(_shader_options_line(minecraft_version, shader_file))
    return "\n".join(lines) + ("\n" if lines else "")
