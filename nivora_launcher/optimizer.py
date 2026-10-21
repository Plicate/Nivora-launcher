"""launcher-side optimization engine."""
from __future__ import annotations

import json
import os
import platform
import re
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

_AIKARS_FLAGS: list[str] = [
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=30",
    "-XX:G1MaxNewSizePercent=40",
    "-XX:G1HeapRegionSize=8M",
    "-XX:G1ReservePercent=20",
    "-XX:G1HeapWastePercent=5",
    "-XX:G1MixedGCCountTarget=4",
    "-XX:InitiatingHeapOccupancyPercent=15",
    "-XX:G1MixedGCLiveThresholdPercent=90",
    "-XX:G1RSetUpdatingPauseTimePercent=5",
    "-XX:SurvivorRatio=32",
    "-XX:+PerfDisableSharedMem",
    "-XX:MaxTenuringThreshold=1",
    "-Dusing.aikars.flags=https://mcflags.emc.gs",
    "-Daikars.new.flags=true",
]

# low-pause G1GC flags – best for FPS-focused sessions.
_FPS_FLAGS: list[str] = [
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=50",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+AlwaysPreTouch",
    "-XX:+DisableExplicitGC",
    "-Dfml.earlyprogresswindow=false",
]

# balanced G1GC flags – good default for most users.
_BALANCED_FLAGS: list[str] = [
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=100",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+AlwaysPreTouch",
    "-XX:+DisableExplicitGC",
]

# compatibility flags – no GC tuning, for old Java / edge cases.
_COMPAT_FLAGS: list[str] = [
    "-XX:+UseSerialGC",
    "-Dfml.earlyprogresswindow=false",
]

# streaming preset – prioritise steady frame delivery over peak FPS.
_STREAMING_FLAGS: list[str] = [
    "-XX:+UseG1GC",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=100",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+AlwaysPreTouch",
    "-XX:+DisableExplicitGC",
    "-XX:+PerfDisableSharedMem",
]

JVM_PRESETS: dict[str, dict[str, Any]] = {
    "fps": {
        "id": "fps",
        "name": "FPS (низкие паузы GC)",
        "description": "Минимальные паузы сборщика мусора. Лучший FPS для ванильных и лёгких сборок.",
        "flags": _FPS_FLAGS,
        "recommended_for": ["vanilla", "light"],
    },
    "balanced": {
        "id": "balanced",
        "name": "Balanced",
        "description": "Хороший баланс FPS и стабильности. Универсальный вариант для большинства сборок.",
        "flags": _BALANCED_FLAGS,
        "recommended_for": ["vanilla", "light", "medium"],
    },
    "heavy_modpack": {
        "id": "heavy_modpack",
        "name": "Heavy Modpack (Aikar's)",
        "description": "Флаги Aikar — проверенный стандарт для тяжёлых сборок с сотнями модов.",
        "flags": _AIKARS_FLAGS,
        "recommended_for": ["medium", "heavy"],
    },
    "compatibility": {
        "id": "compatibility",
        "name": "Compatibility",
        "description": "Только SerialGC, без эксперименталок. Для старых версий и проблемных окружений.",
        "flags": _COMPAT_FLAGS,
        "recommended_for": ["legacy"],
    },
    "streaming": {
        "id": "streaming",
        "name": "Streaming",
        "description": "Равномерный фреймтайм. Снижает дропы при стриминге/записи.",
        "flags": _STREAMING_FLAGS,
        "recommended_for": ["light", "medium"],
    },
    "none": {
        "id": "none",
        "name": "Без пресета",
        "description": "JVM-аргументы из профиля не изменяются.",
        "flags": [],
        "recommended_for": [],
    },
}

def system_ram_kib() -> int:
    """Total physical RAM in KiB (best-effort, 0 if unavailable)."""
    if os.name == "nt":
        try:
            import psutil
            return int(psutil.virtual_memory().total) // 1024
        except Exception:
            return 0
    try:
        pages = int(os.sysconf("SC_PHYS_PAGES"))
        page_size = int(os.sysconf("SC_PAGE_SIZE"))
        if pages > 0 and page_size > 0:
            return pages * page_size // 1024
    except Exception:
        pass
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1])
    except Exception:
        pass
    return 0

def count_mods_in_instance(instance_dir: Path) -> int:
    """Count mod .jar files in instance/mods (non-recursive, max 4096)."""
    mods_dir = instance_dir / "mods"
    if not mods_dir.is_dir():
        return 0
    count = 0
    try:
        for entry in mods_dir.iterdir():
            if count >= 4096:
                break
            if entry.suffix.lower() in {".jar", ".litemod"} and entry.is_file():
                count += 1
    except OSError:
        pass
    return count

def classify_instance_weight(mod_count: int, loader: str) -> str:
    """Return one of: vanilla | light | medium | heavy | legacy."""
    if loader in {"", "vanilla"}:
        return "vanilla"
    if mod_count <= 0:
        return "vanilla"
    if mod_count < 20:
        return "light"
    if mod_count < 80:
        return "medium"
    return "heavy"

_MEMORY_TABLE: list[tuple[str, int, int]] = [
    # weight, min_sys_ram_gib, rec_mib
    ("vanilla", 0, 1536),
    ("vanilla", 8, 2048),
    ("light", 0, 2048),
    ("light", 8, 2048),
    ("light", 16, 3072),
    ("medium", 0, 3072),
    ("medium", 8, 4096),
    ("medium", 16, 6144),
    ("heavy", 0, 4096),
    ("heavy", 8, 6144),
    ("heavy", 16, 8192),
    ("heavy", 32, 10240),
    ("legacy", 0, 1024),
    ("legacy", 4, 1536),
]

def recommend_memory_mib(
    *,
    weight: str = "vanilla",
    system_ram_kib: int = 0,
    mod_count: int = 0,
) -> int:
    """Return recommended Minecraft heap size in MiB."""
    sys_gib = system_ram_kib // (1024 * 1024)
    best = 2048  # safe fallback
    for w, min_gib, mib in _MEMORY_TABLE:
        if w == weight and sys_gib >= min_gib:
            best = mib
    # per-mod tweak: add 12 MiB per mod beyond 80 mods (heavy packs)
    if mod_count > 80:
        extra = ((mod_count - 80) // 20) * 256
        best = min(best + extra, 16384)
    # never exceed 60 % of system RAM
    if system_ram_kib > 0:
        ceiling = (system_ram_kib * 6) // (10 * 1024)  # KiB → MiB at 60 %
        best = min(best, max(1024, ceiling))
    return best

@dataclass
class GpuInfo:
    vendor: str = "unknown"   # nvidia | amd | intel | unknown
    driver: str = ""
    renderer: str = ""
    vram_mib: int = 0
    supports_vulkan: bool = False
    # true when system has multiple GPU vendors (hybrid/Optimus).
    is_hybrid: bool = False

def detect_gpu() -> GpuInfo:
    """Best-effort GPU detection without X11/GL context."""
    info = GpuInfo()
    if os.name == "nt":
        return _detect_gpu_windows(info)
    return _detect_gpu_linux(info)

def _detect_gpu_linux(info: GpuInfo) -> GpuInfo:
    """Detect GPU on Linux via /sys/class/drm + nvidia-smi."""
    drm = Path("/sys/class/drm")
    vendors: set[str] = set()
    _VID_MAP = {"0x10de": "nvidia", "0x1002": "amd", "0x8086": "intel"}
    try:
        for card in sorted(drm.iterdir()):
            try:
                vid = (card / "device" / "vendor").read_text(encoding="utf-8").strip().lower()
                v = _VID_MAP.get(vid)
                if v:
                    vendors.add(v)
            except OSError:
                pass
    except OSError:
        pass

    # prefer discrete GPU: nvidia > amd > intel
    for v in ("nvidia", "amd", "intel"):
        if v in vendors:
            info.vendor = v
            break

    # hybrid: both a discrete GPU vendor and Intel iGPU present
    info.is_hybrid = ("intel" in vendors and len(vendors) >= 2) or len(vendors) >= 2

    # nvidia-smi → VRAM + driver version (works on all platforms with NVIDIA drivers)
    if info.vendor == "nvidia" and shutil.which("nvidia-smi"):
        try:
            out = subprocess.check_output(
                ["nvidia-smi", "--query-gpu=memory.total,driver_version",
                 "--format=csv,noheader,nounits"],
                timeout=3, stderr=subprocess.DEVNULL, text=True,
            )
            parts = out.strip().split(",")
            if len(parts) >= 1:
                info.vram_mib = int(parts[0].strip())
            if len(parts) >= 2:
                info.driver = parts[1].strip()
        except Exception:
            pass

    # glxinfo renderer (only present when X11/GLX is running)
    if shutil.which("glxinfo"):
        try:
            out = subprocess.check_output(
                ["glxinfo", "-B"], timeout=4, stderr=subprocess.DEVNULL, text=True
            )
            for line in out.splitlines():
                if "opengl renderer" in line.lower():
                    info.renderer = line.split(":", 1)[-1].strip()
                    break
        except Exception:
            pass

    # vulkan: vulkaninfo binary (package vulkan-tools on most distros)
    info.supports_vulkan = bool(shutil.which("vulkaninfo"))
    return info

def _detect_gpu_windows(info: GpuInfo) -> GpuInfo:
    """Detect GPU on Windows via PowerShell CIM (primary) or wmic (fallback)."""
    adapters: list[dict] = _win_gpu_adapters_powershell()
    if not adapters:
        adapters = _win_gpu_adapters_wmic()
    if not adapters:
        return info

    # classify each adapter
    _WIN_VENDORS = [("nvidia",), ("amd", "radeon"), ("intel",)]
    classified: list[tuple[str, dict]] = []
    for adapter in adapters:
        name_l = adapter.get("name", "").lower()
        vendor = "unknown"
        for vgroup in _WIN_VENDORS:
            if any(k in name_l for k in vgroup):
                vendor = vgroup[0]
                break
        classified.append((vendor, adapter))

    # pick primary (discrete first)
    for preferred in ("nvidia", "amd", "intel"):
        for vendor, adapter in classified:
            if vendor == preferred:
                info.vendor = vendor
                info.renderer = adapter.get("name", "")
                info.driver = adapter.get("driver", "")
                info.vram_mib = adapter.get("vram_mib", 0)
                break
        if info.vendor != "unknown":
            break

    # hybrid: discrete GPU (non-Intel) present alongside Intel iGPU
    has_intel = any(v == "intel" for v, _ in classified)
    has_discrete = any(v in {"nvidia", "amd"} for v, _ in classified)
    info.is_hybrid = has_intel and has_discrete

    # vulkan: vulkan-1.dll in System32
    info.supports_vulkan = _win_has_vulkan()
    return info

def _win_gpu_adapters_powershell() -> list[dict]:
    """Query video controllers via PowerShell Get-CimInstance (Windows 7+)."""
    try:
        out = subprocess.check_output(
            [
                "powershell", "-NoProfile", "-NonInteractive", "-Command",
                "Get-CimInstance Win32_VideoController "
                "| Select-Object Name,AdapterRAM,DriverVersion "
                "| ConvertTo-Json -Compress",
            ],
            timeout=6, stderr=subprocess.DEVNULL, text=True, creationflags=0x08000000,
        )
        import json as _json
        raw = _json.loads(out.strip() or "[]")
        if isinstance(raw, dict):
            raw = [raw]  # single adapter returned as object, not array
        result: list[dict] = []
        for item in raw if isinstance(raw, list) else []:
            if not isinstance(item, dict):
                continue
            vram_bytes = item.get("AdapterRAM") or 0
            result.append({
                "name": str(item.get("Name") or ""),
                "driver": str(item.get("DriverVersion") or ""),
                "vram_mib": int(vram_bytes) // (1024 * 1024) if vram_bytes else 0,
            })
        return result
    except Exception:
        return []

def _win_gpu_adapters_wmic() -> list[dict]:
    """Fallback: query video controllers via wmic (deprecated in Win11, but still works)."""
    try:
        out = subprocess.check_output(
            ["wmic", "path", "win32_VideoController",
             "get", "Name,AdapterRAM,DriverVersion", "/format:csv"],
            timeout=5, stderr=subprocess.DEVNULL, text=True,
        )
        result: list[dict] = []
        for line in out.strip().splitlines()[1:]:
            parts = [p.strip() for p in line.split(",")]
            if len(parts) < 3:
                continue
            name = parts[2] if len(parts) > 2 else ""
            if not name:
                continue
            try:
                vram_mib = int(parts[1]) // (1024 * 1024) if parts[1] else 0
            except ValueError:
                vram_mib = 0
            result.append({
                "name": name,
                "driver": parts[3] if len(parts) > 3 else "",
                "vram_mib": vram_mib,
            })
        return result
    except Exception:
        return []

def _win_has_vulkan() -> bool:
    """Check for Vulkan runtime on Windows (vulkan-1.dll in System32)."""
    try:
        import ctypes
        sys32 = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
        if (sys32 / "vulkan-1.dll").exists():
            return True
    except Exception:
        pass
    return bool(shutil.which("vulkaninfo") or shutil.which("vulkaninfo.exe"))

def gpu_env_hints(gpu: GpuInfo, current_mode: str = "auto") -> dict[str, str]:
    """return platform-specific environment-variable hints based on GPU vendor."""
    if os.name == "nt":
        # windows: driver handles discrete GPU selection transparently.
        return {}
    # linux / other POSIX: X11/Vulkan hints for hybrid GPU setups.
    hints: dict[str, str] = {}
    if gpu.vendor == "nvidia" and current_mode in {"", "auto", "nvidia"} and gpu.is_hybrid:
        hints["__NV_PRIME_RENDER_OFFLOAD"] = "1"
        hints["__GLX_VENDOR_LIBRARY_NAME"] = "nvidia"
        hints["__VK_LAYER_NV_optimus"] = "NVIDIA_only"
    elif gpu.vendor == "amd" and gpu.is_hybrid:
        # on hybrid AMD systems, prefer Mesa RADV over amdvlk if present.
        if shutil.which("vulkaninfo"):
            hints["AMD_VULKAN_ICD"] = "RADV"
    return hints

@dataclass
class OptimizerMod:
    name: str
    mod_id: str
    description: str
    url: str
    loaders: list[str]  # fabric | quilt | forge | neoforge
    min_mc_version: str  # e.g. "1.16"
    category: str  # fps | memory | loading | network
    required_side: str = "client"  # client | server | both

_OPTIMIZER_MODS: list[OptimizerMod] = [
    OptimizerMod(
        name="Sodium",
        mod_id="sodium",
        description="Мощная замена рендерера Minecraft. Огромный прирост FPS.",
        url="https://modrinth.com/mod/sodium",
        loaders=["fabric", "quilt"],
        min_mc_version="1.16",
        category="fps",
    ),
    OptimizerMod(
        name="Lithium",
        mod_id="lithium",
        description="Оптимизации игровой логики без изменения поведения.",
        url="https://modrinth.com/mod/lithium",
        loaders=["fabric", "quilt"],
        min_mc_version="1.16",
        category="fps",
        required_side="both",
    ),
    OptimizerMod(
        name="FerriteCore",
        mod_id="ferrite-core",
        description="Снижает потребление памяти блок-состояниями.",
        url="https://modrinth.com/mod/ferrite-core",
        loaders=["fabric", "quilt", "forge", "neoforge"],
        min_mc_version="1.16",
        category="memory",
        required_side="both",
    ),
    OptimizerMod(
        name="Phosphor",
        mod_id="phosphor",
        description="Оптимизирует движок освещения (для старых версий).",
        url="https://modrinth.com/mod/phosphor",
        loaders=["fabric"],
        min_mc_version="1.15",
        category="fps",
        required_side="both",
    ),
    OptimizerMod(
        name="Starlight",
        mod_id="starlight",
        description="Полная переработка движка освещения (1.17+).",
        url="https://modrinth.com/mod/starlight",
        loaders=["fabric", "quilt"],
        min_mc_version="1.17",
        category="fps",
        required_side="both",
    ),
    OptimizerMod(
        name="LazyDFU",
        mod_id="lazydfu",
        description="Ленивая инициализация DataFixerUpper — ускоряет загрузку.",
        url="https://modrinth.com/mod/lazydfu",
        loaders=["fabric", "quilt"],
        min_mc_version="1.16",
        category="loading",
    ),
    OptimizerMod(
        name="ModernFix",
        mod_id="modernfix",
        description="Снижает использование памяти и время загрузки. Поддерживает Forge/Fabric/NeoForge.",
        url="https://modrinth.com/mod/modernfix",
        loaders=["fabric", "quilt", "forge", "neoforge"],
        min_mc_version="1.16",
        category="loading",
        required_side="both",
    ),
    OptimizerMod(
        name="Indium",
        mod_id="indium",
        description="Поддержка Fabric Rendering API для Sodium.",
        url="https://modrinth.com/mod/indium",
        loaders=["fabric", "quilt"],
        min_mc_version="1.16",
        category="fps",
    ),
    OptimizerMod(
        name="ImmediatelyFast",
        mod_id="immediatelyfast",
        description="Ускоряет immediate mode рендеринг (entity, GUI, HUD).",
        url="https://modrinth.com/mod/immediatelyfast",
        loaders=["fabric", "quilt"],
        min_mc_version="1.18",
        category="fps",
    ),
    OptimizerMod(
        name="Entity Culling",
        mod_id="entityculling",
        description="Не рендерит сущности за стенами. Прирост FPS на городах/фермах.",
        url="https://modrinth.com/mod/entityculling",
        loaders=["fabric", "quilt", "forge", "neoforge"],
        min_mc_version="1.16",
        category="fps",
    ),
    OptimizerMod(
        name="Clumps",
        mod_id="clumps",
        description="Группирует шарики опыта → меньше сущностей → лучше TPS.",
        url="https://modrinth.com/mod/clumps",
        loaders=["fabric", "quilt", "forge", "neoforge"],
        min_mc_version="1.16",
        category="fps",
        required_side="both",
    ),
    OptimizerMod(
        name="Krypton",
        mod_id="krypton",
        description="Оптимизирует сетевой стек Minecraft.",
        url="https://modrinth.com/mod/krypton",
        loaders=["fabric", "quilt"],
        min_mc_version="1.16",
        category="network",
        required_side="both",
    ),
]

def _mc_minor_version(version_id: str) -> int:
    """Extract minor version number from a Minecraft version string."""
    m = re.search(r"1\.(\d+)", version_id)
    return int(m.group(1)) if m else 0

def recommend_optimizer_mods(
    *,
    loader: str,
    mc_version: str,
    weight: str = "vanilla",
    installed_mod_ids: set[str] | None = None,
) -> list[dict[str, Any]]:
    """Return a list of recommended optimizer mod dicts for the given setup."""
    minor = _mc_minor_version(mc_version)
    installed = installed_mod_ids or set()
    results: list[dict[str, Any]] = []
    for mod in _OPTIMIZER_MODS:
        mod_minor = _mc_minor_version(mod.min_mc_version)
        if minor < mod_minor:
            continue
        if loader and mod.loaders and loader.lower() not in mod.loaders:
            continue
        already = mod.mod_id in installed
        results.append({
            "name": mod.name,
            "mod_id": mod.mod_id,
            "description": mod.description,
            "url": mod.url,
            "category": mod.category,
            "required_side": mod.required_side,
            "already_installed": already,
        })
    # sort: not-installed first, then by category priority
    cat_priority = {"fps": 0, "memory": 1, "loading": 2, "network": 3}
    results.sort(key=lambda m: (m["already_installed"], cat_priority.get(m["category"], 9)))
    return results

LAUNCH_METRICS_FILE = ".trial-launcher-launch-history.json"
_MAX_HISTORY_ENTRIES = 20

def _launch_history_path(instance_dir: Path) -> Path:
    return instance_dir / LAUNCH_METRICS_FILE

def record_launch_start(instance_dir: Path, launch_id: str, *, loader: str, preset: str, memory: str, mc_version: str) -> None:
    """Write a launch-start entry to the instance's launch history."""
    path = _launch_history_path(instance_dir)
    history = _load_launch_history(path)
    history.append({
        "launch_id": launch_id,
        "started_at": time.time(),
        "loader": loader,
        "preset": preset,
        "memory": memory,
        "mc_version": mc_version,
        "exit_code": None,
        "elapsed_seconds": None,
        "time_to_window_seconds": None,
    })
    _save_launch_history(path, history)

def record_launch_end(
    instance_dir: Path,
    launch_id: str,
    *,
    exit_code: int,
    elapsed_seconds: float,
    time_to_window_seconds: float | None = None,
) -> None:
    """Update the launch history entry with outcome."""
    path = _launch_history_path(instance_dir)
    history = _load_launch_history(path)
    for entry in reversed(history):
        if entry.get("launch_id") == launch_id:
            entry["exit_code"] = exit_code
            entry["elapsed_seconds"] = round(elapsed_seconds, 2)
            if time_to_window_seconds is not None:
                entry["time_to_window_seconds"] = round(time_to_window_seconds, 2)
            break
    _save_launch_history(path, history)

def read_launch_history(instance_dir: Path) -> list[dict[str, Any]]:
    return _load_launch_history(_launch_history_path(instance_dir))

def _load_launch_history(path: Path) -> list[dict[str, Any]]:
    try:
        if path.is_file() and not path.is_symlink() and path.stat().st_size <= 256 * 1024:
            data = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(data, list):
                return [e for e in data if isinstance(e, dict)]
    except (OSError, ValueError):
        pass
    return []

def _save_launch_history(path: Path, history: list[dict[str, Any]]) -> None:
    try:
        trimmed = history[-_MAX_HISTORY_ENTRIES:]
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(trimmed, indent=2), encoding="utf-8")
        tmp.replace(path)
    except OSError:
        pass

@dataclass
class OptimizationReport:
    """Full optimization analysis for an instance."""
    # system
    system_ram_kib: int = 0
    cpu_count: int = 1
    os_name: str = ""
    gpu: GpuInfo = field(default_factory=GpuInfo)
    # instance
    loader: str = ""
    mc_version: str = ""
    mod_count: int = 0
    weight: str = "vanilla"
    # recommendations
    recommended_memory_mib: int = 2048
    recommended_preset_id: str = "balanced"
    recommended_mods: list[dict[str, Any]] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    current_memory_mib: int = 0
    current_preset_id: str = "none"

    def to_dict(self) -> dict[str, Any]:
        return {
            "system": {
                "ram_kib": self.system_ram_kib,
                "ram_gib": round(self.system_ram_kib / (1024 * 1024), 1),
                "cpu_count": self.cpu_count,
                "os": self.os_name,
                "gpu_vendor": self.gpu.vendor,
                "gpu_renderer": self.gpu.renderer,
                "gpu_driver": self.gpu.driver,
                "gpu_vram_mib": self.gpu.vram_mib,
                "gpu_is_hybrid": self.gpu.is_hybrid,
            },
            "instance": {
                "loader": self.loader,
                "mc_version": self.mc_version,
                "mod_count": self.mod_count,
                "weight": self.weight,
            },
            "recommendations": {
                "memory_mib": self.recommended_memory_mib,
                "preset_id": self.recommended_preset_id,
                "preset_name": JVM_PRESETS.get(self.recommended_preset_id, {}).get("name", ""),
                "preset_flags": JVM_PRESETS.get(self.recommended_preset_id, {}).get("flags", []),
                "optimizer_mods": self.recommended_mods,
                "warnings": self.warnings,
            },
            "current": {
                "memory_mib": self.current_memory_mib,
                "preset_id": self.current_preset_id,
            },
        }

def analyze_instance(
    *,
    instance_dir: Path | None = None,
    loader: str = "",
    mc_version: str = "",
    current_memory: str = "auto",
    current_preset: str = "none",
    installed_mod_ids: set[str] | None = None,
) -> OptimizationReport:
    """Produce a full OptimizationReport for the given instance."""
    from .config import parse_memory_kib, recommended_memory_kib, system_memory_kib

    ram = system_ram_kib()
    mod_count = count_mods_in_instance(instance_dir) if instance_dir else 0
    weight = classify_instance_weight(mod_count, loader)
    if mc_version and _mc_minor_version(mc_version) < 13:
        weight = "legacy"

    rec_mib = recommend_memory_mib(weight=weight, system_ram_kib=ram, mod_count=mod_count)

    # recommended preset
    if weight == "legacy":
        preset_id = "compatibility"
    elif weight in {"heavy"}:
        preset_id = "heavy_modpack"
    elif weight in {"medium"}:
        preset_id = "balanced"
    else:
        preset_id = "fps"

    # recommended mods
    mods = recommend_optimizer_mods(
        loader=loader,
        mc_version=mc_version or "1.21",
        weight=weight,
        installed_mod_ids=installed_mod_ids,
    )

    gpu = detect_gpu()

    # parse current memory
    try:
        cur_mib = parse_memory_kib(current_memory) // 1024
    except Exception:
        cur_mib = recommended_memory_kib() // 1024

    warnings: list[str] = []
    # warn if current memory is very low
    if cur_mib < 1024:
        warnings.append("Выделено менее 1 ГиБ — игра может крашиться.")
    elif cur_mib < rec_mib - 512:
        warnings.append(
            f"Выделено {cur_mib} МиБ, рекомендуется {rec_mib} МиБ для данной сборки."
        )
    # warn if memory is way too high
    if ram > 0 and cur_mib > (ram // 1024) * 75 // 100:
        warnings.append("Выделено >75 % RAM — может замедлить систему.")
    # warn on hybrid GPU setups
    if gpu.is_hybrid and gpu.vendor == "nvidia":
        if os.name == "nt":
            warnings.append(
                "Обнаружен гибридный GPU (NVIDIA + Intel). "
                "Убедитесь, что в настройках NVIDIA выбрана 'Максимальная производительность' для java.exe."
            )
        else:
            warnings.append(
                "Обнаружен гибридный GPU (NVIDIA + iGPU). "
                "Рекомендуется режим графики: nvidia."
            )

    return OptimizationReport(
        system_ram_kib=ram,
        cpu_count=os.cpu_count() or 1,
        os_name=platform.system(),
        gpu=gpu,
        loader=loader,
        mc_version=mc_version,
        mod_count=mod_count,
        weight=weight,
        recommended_memory_mib=rec_mib,
        recommended_preset_id=preset_id,
        recommended_mods=mods,
        warnings=warnings,
        current_memory_mib=cur_mib,
        current_preset_id=current_preset,
    )

def prune_stale_metadata_cache(cache_dir: Path, *, max_age_hours: int = 48) -> int:
    """Delete metadata cache files older than max_age_hours. Returns count deleted."""
    cutoff = time.time() - max_age_hours * 3600
    deleted = 0
    try:
        for path in cache_dir.iterdir():
            if path.suffix in {".json", ".cache"} and path.is_file() and not path.is_symlink():
                try:
                    if path.stat().st_mtime < cutoff:
                        path.unlink()
                        deleted += 1
                except OSError:
                    pass
    except OSError:
        pass
    return deleted

def cache_stats(cache_dir: Path) -> dict[str, Any]:
    """Return basic stats for the download cache directory."""
    total_bytes = 0
    file_count = 0
    try:
        for path in cache_dir.rglob("*"):
            if path.is_file() and not path.is_symlink():
                try:
                    total_bytes += path.stat().st_size
                    file_count += 1
                except OSError:
                    pass
    except OSError:
        pass
    return {
        "total_bytes": total_bytes,
        "total_mib": round(total_bytes / (1024 * 1024), 1),
        "file_count": file_count,
    }
