from __future__ import annotations

import os
import re
import subprocess
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .errors import LauncherError

_FEATURE_WEIGHTS: dict[str, int] = {
    "Тени (shadow pass)": 16,
    "POM / параллакс": 12,
    "Волюметрический свет": 10,
    "Отражения (SSR/raytracing)": 10,
    "Волюметрические облака": 9,
    "TAA / временное сглаживание": 6,
    "Bloom": 5,
    "Глубина резкости (DOF)": 6,
    "Motion blur": 3,
    "Динамическое небо": 3,
    "Искажение теней": 2,
}

_FEATURE_PATTERNS: dict[str, str] = {
    "POM / параллакс": r"parallax|relief[_ ]?mapping|pom[_ ]?enabled|PARALLAX",
    "Волюметрический свет": r"volumetric|lightshaft|god[_ ]?ray|VL_ENABLED|volumetrics",
    "Отражения (SSR/raytracing)": r"raytrac|ssr|screen[_ ]space[_ ]reflect|REFLECTION",
    "Волюметрические облака": r"volumetric[_ ]cloud|cloud[_ ]raymarch|cumulus",
    "TAA / временное сглаживание": r"\btaa\b|temporal[_ ]?(aa|filter|accum)|TAA_ENABLED",
    "Bloom": r"\bbloom\b",
    "Глубина резкости (DOF)": r"\bdof\b|depth[_ ]of[_ ]field|focal[_ ]",
    "Motion blur": r"motion[_ ]?blur",
    "Динамическое небо": r"\bskybox\b|atmospher|scatter",
    "Искажение теней": r"distort",
}

_PASS_PATTERNS = (
    (r"^final\.(fsh|vsh)$", "final", 8),
    (r"^composite\d*\.(fsh|vsh)$", "composite", 6),
    (r"^deferred\d*\.(fsh|vsh)$", "deferred", 6),
    (r"^prepare\d*\.(fsh|vsh)$", "prepare", 4),
    (r"^shadow(fst|comp)?\.(fsh|vsh)$", "shadow", 14),
    (r"^gbuffers_[\w]+\.(fsh|vsh)$", "gbuffers", 3),
    (r"^\w+\.(gsh|csh)$", "compute", 7),
)

# известные паки по уровням нагрузки — для рекомендаций «что тянет карта».
_RECOMMENDED_PACKS: dict[str, list[dict[str, str]]] = {
    "software": [
        {"name": "Без шейдеров", "detail": "Программный рендер не тянет шейдеры в принципе"},
    ],
    "entry": [
        {"name": "Sildur Vibrant Lite", "detail": "Легкие эффекты, для встроенной графики"},
        {"name": "Sildur Enhanced Default", "detail": "Мягкие тени, почти без потери FPS"},
        {"name": "Vanilla Plus", "detail": "Улучшенная ваниль без тяжёлых пассов"},
    ],
    "mid": [
        {"name": "BSL", "detail": "Сбалансированная классика, гибкие настройки"},
        {"name": "Complementary Reimagined (Low)", "detail": "Картинка уровня консолей"},
        {"name": "Chocapic v4", "detail": "Средняя нагрузка, хороший вид"},
    ],
    "high": [
        {"name": "Complementary Reimagined", "detail": "Эталонная картинка без компромиссов"},
        {"name": "BSL (High + тени 2048)", "detail": "Полные тени и волюметрики"},
        {"name": "Rethinking Voxels", "detail": "Воксельные тени, тяжело для CPU тоже"},
    ],
    "enthusiast": [
        {"name": "Complementary Unbound", "detail": "Максимум эффектов"},
        {"name": "SEUS PTGI", "detail": "Трассировка пути без RTX"},
        {"name": "Molly's Path Traced", "detail": "Требует топовую карту"},
    ],
}

@dataclass
class ShaderAnalysis:
    name: str
    format: str = "iris"
    files: int = 0
    programs: list[str] = field(default_factory=list)
    features: list[str] = field(default_factory=list)
    shadow_resolution: int = 0
    size_bytes: int = 0
    complexity: int = 0
    estimated_vram_mb: int = 0
    notes: list[str] = field(default_factory=list)

    def verdict_against(self, tier: str) -> dict[str, Any]:
        thresholds = {
            "software": 0,
            "entry": 22,
            "mid": 45,
            "high": 70,
            "enthusiast": 101,
        }
        limit = thresholds.get(tier, 45)
        if self.complexity <= max(0, limit - 20):
            label, tone = "Потянет с запасом", "success"
        elif self.complexity <= limit:
            label, tone = "Потянет", "success"
        elif self.complexity <= limit + 18:
            label, tone = "На пределе — уменьшите дальность теней", "warning"
        else:
            label, tone = "Не потянет", "danger"
        return {
            "label": label,
            "tone": tone,
            "required_tier": next(
                (name for name, bound in (("entry", 22), ("mid", 45), ("high", 70), ("enthusiast", 101)) if self.complexity <= bound),
                "enthusiast",
            ),
        }

    def as_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "format": self.format,
            "files": self.files,
            "programs": self.programs,
            "features": self.features,
            "shadow_resolution": self.shadow_resolution,
            "size_bytes": self.size_bytes,
            "complexity": self.complexity,
            "estimated_vram_mb": self.estimated_vram_mb,
            "notes": self.notes,
        }

def analyze_shaderpack(source: Path) -> dict[str, Any]:
    """Разбирает шейдерпак: zip-архив или уже распакованный каталог."""
    if source.is_symlink():
        raise LauncherError("Шейдерпак является символьной ссылкой")
    if source.is_file():
        if source.stat().st_size > 512 * 1024 * 1024:
            raise LauncherError("Архив шейдерпака слишком большой")
        if not zipfile.is_zipfile(source):
            raise LauncherError("Это не архив шейдерпака")
        with zipfile.ZipFile(source) as archive:
            entries = [(item.filename.replace("\\", "/"), item.file_size) for item in archive.infolist() if not item.is_dir()]
            analysis = _analyze_entries(source.stem, entries)
            _scan_content(archive.read, entries, analysis)
    elif source.is_dir():
        entries = [
            (str(path.relative_to(source)).replace("\\", "/"), path.stat().st_size)
            for path in source.rglob("*")
            if path.is_file() and not path.is_symlink()
        ]
        analysis = _analyze_entries(source.name, entries)
        _scan_content(lambda name: (source / name).read_bytes(), entries, analysis)
    else:
        raise LauncherError(f"Шейдерпак не найден: {source}")
    if not analysis.programs:
        raise LauncherError("В шейдерпаке нет программ шейдеров (папка shaders/ с .fsh/.vsh)")
    analysis.size_bytes = sum(size for _name, size in entries)
    _finalize_complexity(analysis)
    return analysis.as_dict()

def _analyze_entries(name: str, entries) -> ShaderAnalysis:
    analysis = ShaderAnalysis(name=name)
    for entry_name, size in entries:
        normalized = entry_name.replace("\\", "/")
        if ".." in normalized.split("/"):
            analysis.notes.append(f"Подозрительный путь в архиве: {normalized}")
            continue
        if normalized.startswith("shaders/") or "/shaders/" in normalized:
            analysis.files += 1
            base = Path(normalized).name
            for pattern, kind, _weight in _PASS_PATTERNS:
                if re.match(pattern, base, re.IGNORECASE):
                    if kind not in analysis.programs:
                        analysis.programs.append(kind)
                    break
            if base == "shaders.properties":
                analysis.format = "iris-properties"
    if "shadow" in analysis.programs:
        analysis.features.append("Тени (shadow pass)")
    return analysis

def _scan_content(read_entry, entries, analysis: ShaderAnalysis) -> None:
    for name, _size in entries:
        if not name.lower().endswith((".fsh", ".vsh", ".glsl", ".gsh", ".csh", ".properties")):
            continue
        try:
            content = read_entry(name)
            if len(content) > 4 * 1024 * 1024:
                continue
            text = content.decode("utf-8", errors="replace")
        except (OSError, zipfile.BadZipFile):
            continue
        if name.endswith("shaders.properties"):
            match = re.search(r"^\s*shadowMapResolution\s*=\s*(\d+)", text, re.MULTILINE)
            if match:
                analysis.shadow_resolution = int(match.group(1))
            if re.search(r"^\s*supportedVersions", text, re.MULTILINE):
                analysis.notes.append("Пак декларирует поддерживаемые версии Minecraft")
            continue
        for feature, pattern in _FEATURE_PATTERNS.items():
            if feature in analysis.features:
                continue
            if re.search(pattern, text):
                analysis.features.append(feature)
        if not analysis.shadow_resolution:
            match = re.search(r"shadowMapResolution\s*=\s*(\d+)", text)
            if match:
                analysis.shadow_resolution = int(match.group(1))
        if re.search(r"iris\.", text) or "iris_features" in text:
            analysis.format = "iris"
        if re.search(r"#if\s+MC_VERSION\s*>=\s*\d+", text):
            analysis.notes.append("Пак адаптируется под версию Minecraft")

def _finalize_complexity(analysis: ShaderAnalysis) -> None:
    score = 0
    for feature in analysis.features:
        score += _FEATURE_WEIGHTS.get(feature, 2)
    weights = {kind: weight for _pattern, kind, weight in _PASS_PATTERNS}
    score += sum(weights.get(kind, 3) for kind in analysis.programs)
    shadow = analysis.shadow_resolution or (2048 if "Тени (shadow pass)" in analysis.features else 0)
    if shadow:
        score += min(18, shadow // 256)
        analysis.estimated_vram_mb = 256 + (shadow * shadow * 4 * 3) // (1024 * 1024) + 128 * max(0, len(analysis.programs) - 4)
    else:
        analysis.estimated_vram_mb = 256
    analysis.complexity = max(1, min(100, score))

def gpu_profile() -> dict[str, Any]:
    """Карточка GPU: имя, видеопамять, уровень производительности."""
    profile = _nvidia_profile()
    if profile is None:
        profile = _amdgpu_profile()
    if profile is None:
        profile = _intel_profile()
    if profile is None:
        return {
            "name": "Видеокарта не определена",
            "vram_mb": 0,
            "tier": "mid",
            "tier_label": "не определён",
            "source": "none",
            "recommendations": _RECOMMENDED_PACKS["mid"],
        }
    profile["tier_label"] = {
        "software": "программный рендер",
        "entry": "начальный",
        "mid": "средний",
        "high": "высокий",
        "enthusiast": "энтузиастский",
    }[profile["tier"]]
    profile.setdefault("source", "sysfs")
    profile["recommendations"] = _RECOMMENDED_PACKS.get(profile["tier"], _RECOMMENDED_PACKS["mid"])
    return profile

def _nvidia_profile() -> dict[str, Any] | None:
    try:
        result = subprocess.run(
            ["nvidia-smi", "--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=3,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0 or not result.stdout.strip():
        return None
    fields = result.stdout.splitlines()[0].split(",")
    if len(fields) < 2:
        return None
    name = fields[0].strip()
    vram_mb = int(re.sub(r"\D", "", fields[1]) or 0)
    return {"name": name, "vram_mb": vram_mb, "tier": _tier_from_name_and_vram(name, vram_mb), "source": "nvidia-smi"}

def _amdgpu_profile() -> dict[str, Any] | None:
    name = ""
    vram_mb = 0
    for card in sorted(Path("/sys/class/drm").glob("card[0-9]")):
        device = card / "device"
        try:
            raw = (device / "product_name").read_text(encoding="ascii", errors="replace").strip()
        except OSError:
            raw = ""
        if not raw:
            try:
                vendor = (device / "vendor").read_text(encoding="ascii").strip()
                identifier = (card / "device").read_text(encoding="ascii").strip()
            except OSError:
                continue
            if vendor != "0x1002":
                continue
            raw = f"AMD GPU {identifier}"
        try:
            info = (device / "mem_info_vram_total").read_text(encoding="ascii")
            vram_mb = int(info) // (1024 * 1024)
        except (OSError, ValueError):
            vram_mb = 0
        name = raw
        break
    if not name:
        return None
    return {"name": name, "vram_mb": vram_mb, "tier": _tier_from_name_and_vram(name, vram_mb), "source": "amdgpu"}

def _intel_profile() -> dict[str, Any] | None:
    for card in sorted(Path("/sys/class/drm").glob("card[0-9]")):
        try:
            vendor = (card / "device" / "vendor").read_text(encoding="ascii").strip()
        except OSError:
            continue
        if vendor == "0x8086":
            return {"name": "Intel integrated graphics", "vram_mb": 0, "tier": "entry", "source": "i915"}
    return None

def _tier_from_name_and_vram(name: str, vram_mb: int) -> str:
    lowered = name.lower()
    if "llvmpipe" in lowered or "software" in lowered:
        return "software"
    enthusiast = ("rtx 40" in lowered or "rtx 50" in lowered or "rx 7900" in lowered or "rx 90" in lowered or "arc a7" in lowered or vram_mb >= 16384)
    high = ("rtx 30" in lowered or "rtx 20" in lowered or "rx 68" in lowered or "rx 67" in lowered or "rx 78" in lowered or "gtx 1080" in lowered or vram_mb >= 8192)
    mid = ("gtx 16" in lowered or "gtx 10" in lowered or "rx 5" in lowered or "rx 64" in lowered or vram_mb >= 4096)
    if enthusiast:
        return "enthusiast"
    if high:
        return "high"
    if mid:
        return "mid"
    return "entry"
