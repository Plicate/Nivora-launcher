"""decompilation-based security audit for Java plugins and mods (JAR files)."""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import secrets
import shutil
import stat
import time
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Sequence

from .errors import LauncherError

AUDIT_MAX_JAR_BYTES = 192 * 1024 * 1024
AUDIT_MAX_ENTRIES = 16384
AUDIT_MAX_MEMBER_BYTES = 96 * 1024 * 1024
AUDIT_MAX_CLASS_BYTES = 24 * 1024 * 1024
AUDIT_MAX_UNPACKED_BYTES = 512 * 1024 * 1024
AUDIT_MAX_NESTED_JAR_BYTES = 48 * 1024 * 1024
AUDIT_MAX_NESTED_TOTAL_BYTES = 192 * 1024 * 1024
AUDIT_MAX_NESTED_JARS = 96
AUDIT_MAX_NESTED_DEPTH = 2
AUDIT_MAX_STRINGS_PER_CLASS = 4096
AUDIT_MAX_REFS_PER_CLASS = 8192
AUDIT_MAX_SOURCE_LINE = 2048
AUDIT_MAX_STRING_CONSTANT = 1024
AUDIT_MAX_MANIFEST_BYTES = 256 * 1024
SESSION_TTL_SECONDS = 30 * 60
SESSION_RESULT_TTL_SECONDS = 6 * 60 * 60
SESSION_MAX_ACTIVE = 4
SESSION_ID_RE = re.compile(r"[a-f0-9]{32}\Z", re.ASCII)
_MAX_EVIDENCE_PER_FINDING = 10
_ESTIMATE_SAMPLE_LINES = 1500
_ESTIMATE_MIN_SECONDS = 1
_ESTIMATE_MAX_SECONDS = 1800

SEVERITY_ORDER = ("critical", "warning", "notice", "info")
SEVERITY_SCORE = {"critical": 25, "warning": 8, "notice": 2, "info": 0}
SEVERITY_LABELS = {
    "critical": "Опасно",
    "warning": "Подозрительно",
    "notice": "Замечание",
    "info": "Информация",
}
VERDICT_LABELS = {
    "dangerous": "Обнаружены опасные сигналы",
    "suspicious": "Есть подозрительные сигналы",
    "review": "Есть замечания",
    "clean": "Угроз не обнаружено",
}

class AuditError(LauncherError):
    """Raised for user-facing audit failures."""

_CLASS_MAGIC = b"\xca\xfe\xba\xbe"

# constant pool tags
_CP_UTF8 = 1
_CP_INTEGER = 3
_CP_FLOAT = 4
_CP_LONG = 5
_CP_DOUBLE = 6
_CP_CLASS = 7
_CP_STRING = 8
_CP_FIELDREF = 9
_CP_METHODREF = 10
_CP_INTERFACE_METHODREF = 11
_CP_NAME_AND_TYPE = 12
_CP_METHODHANDLE = 15
_CP_METHODTYPE = 16
_CP_DYNAMIC = 17
_CP_INVOKEDYNAMIC = 18
_CP_MODULE = 19
_CP_PACKAGE = 20

@dataclass
class ClassInfo:
    """Constant-pool facts about one compiled class."""

    entry: str
    name: str = ""
    super_name: str = ""
    major: int = 0
    minor: int = 0
    interfaces: list[str] = field(default_factory=list)
    class_refs: list[str] = field(default_factory=list)
    method_refs: list[str] = field(default_factory=list)
    field_refs: list[str] = field(default_factory=list)
    strings: list[str] = field(default_factory=list)

class _ClassReader:
    __slots__ = ("_data", "_pos")

    def __init__(self, data: bytes) -> None:
        self._data = data
        self._pos = 0

    def _take(self, size: int) -> bytes:
        end = self._pos + size
        if end > len(self._data):
            raise AuditError("Повреждённый class-файл: неожиданный конец данных")
        chunk = self._data[self._pos : end]
        self._pos = end
        return chunk

    def u1(self) -> int:
        return self._take(1)[0]

    def u2(self) -> int:
        return int.from_bytes(self._take(2), "big")

def _decode_mutf8(raw: bytes) -> str:
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("utf-8", errors="replace")

def parse_class_file(data: bytes, *, entry: str = "") -> ClassInfo:
    """Parse one class file and extract constant-pool level facts."""
    if len(data) < 10:
        raise AuditError("Повреждённый class-файл: файл слишком мал")
    if data[:4] != _CLASS_MAGIC:
        raise AuditError("Файл не является Java class-файлом")
    reader = _ClassReader(data[4:])
    minor = reader.u2()
    major = reader.u2()
    count = reader.u2()
    if count == 0:
        raise AuditError("Повреждённый class-файл: пустой constant pool")

    utf8: dict[int, str] = {}
    classes: dict[int, int] = {}
    name_and_type: dict[int, tuple[int, int]] = {}
    member_refs: list[tuple[int, int, int, int]] = []  # (class_index, nat_index, name_index, is_field)
    string_refs: list[int] = []

    index = 1
    while index < count:
        tag = reader.u1()
        if tag == _CP_UTF8:
            length = reader.u2()
            utf8[index] = _decode_mutf8(reader._take(length))
        elif tag in (_CP_INTEGER, _CP_FLOAT):
            reader._take(4)
        elif tag in (_CP_LONG, _CP_DOUBLE):
            reader._take(8)
            index += 1
            if index >= count:
                raise AuditError("Повреждённый class-файл: неверный constant pool")
        elif tag in (_CP_CLASS, _CP_STRING, _CP_METHODTYPE, _CP_MODULE, _CP_PACKAGE):
            ref = reader.u2()
            if tag == _CP_CLASS:
                classes[index] = ref
            elif tag == _CP_STRING:
                string_refs.append(ref)
        elif tag in (_CP_FIELDREF, _CP_METHODREF, _CP_INTERFACE_METHODREF):
            class_index = reader.u2()
            nat_index = reader.u2()
            member_refs.append((class_index, nat_index, 1 if tag == _CP_FIELDREF else 0))
        elif tag == _CP_NAME_AND_TYPE:
            name_index = reader.u2()
            descriptor_index = reader.u2()
            name_and_type[index] = (name_index, descriptor_index)
        elif tag == _CP_METHODHANDLE:
            reader.u1()
            reader.u2()
        elif tag in (_CP_DYNAMIC, _CP_INVOKEDYNAMIC):
            reader.u2()
            reader.u2()
        else:
            raise AuditError(f"Повреждённый class-файл: неизвестный тег {tag}")
        index += 1

    def utf8_value(idx: int) -> str:
        return utf8.get(idx, "")

    def class_name(idx: int) -> str:
        name_index = classes.get(idx)
        return utf8_value(name_index) if name_index is not None else ""

    info = ClassInfo(entry=entry, major=major, minor=minor)
    reader.u2()  # access_flags
    this_index = reader.u2()
    super_index = reader.u2()
    info.name = class_name(this_index) or "unknown/Unknown"
    info.super_name = class_name(super_index)
    interfaces_count = reader.u2()
    for _ in range(min(interfaces_count, 256)):
        info.interfaces.append(class_name(reader.u2()))

    referenced: set[str] = set()
    for name_index in classes.values():
        name = utf8_value(name_index)
        if name:
            referenced.add(name)
    referenced.discard(info.name)
    info.class_refs = sorted(referenced)

    method_refs: set[str] = set()
    field_refs: set[str] = set()
    for class_index, nat_index, is_field in member_refs:
        owner = class_name(class_index)
        nat = name_and_type.get(nat_index)
        name = utf8_value(nat[0]) if nat else ""
        if not owner or not name:
            continue
        target = f"{owner}.{name}"
        if is_field:
            field_refs.add(target)
        else:
            method_refs.add(target)
    info.method_refs = sorted(method_refs)[:AUDIT_MAX_REFS_PER_CLASS]
    info.field_refs = sorted(field_refs)[:AUDIT_MAX_REFS_PER_CLASS]

    strings: set[str] = set()
    for ref in string_refs:
        value = utf8_value(ref)
        if value:
            strings.add(value[:AUDIT_MAX_STRING_CONSTANT])
        if len(strings) >= AUDIT_MAX_STRINGS_PER_CLASS:
            break
    info.strings = sorted(strings)
    return info

_NATIVE_EXTENSIONS = (".dll", ".so", ".dylib", ".jnilib")
_MANIFEST_PLATFORMS = {
    "paper-plugin.yml": "paper",
    "neoforge.mods.toml": "neoforge",
    "META-INF/mods.toml": "forge",
    "fabric.mod.json": "fabric",
    "quilt.mod.json": "quilt",
    "plugin.yml": "bukkit",
    "bungee.yml": "bungee",
}
_PLATFORM_PRIORITY = {"paper": 7, "neoforge": 6, "forge": 5, "fabric": 4, "quilt": 3, "bukkit": 2, "bungee": 1}
_ENTRY_NAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._$/-]{0,255}\Z", re.ASCII)

@dataclass
class JarStructure:
    platform: str = "unknown"
    declared: dict[str, Any] = field(default_factory=dict)
    class_count: int = 0
    string_count: int = 0
    reference_count: int = 0
    natives: list[str] = field(default_factory=list)
    nested_jars: list[str] = field(default_factory=list)
    skipped_entries: list[str] = field(default_factory=list)
    signed: bool = False
    sources_bytes: int = 0
    source_lines: int = 0

def _validate_entry_name(name: str) -> str:
    if not name or "\x00" in name or "\\" in name:
        raise AuditError(f"Недопустимое имя файла в архиве: {name[:120]}")
    if name.startswith("/") or ".." in name.split("/") or not _ENTRY_NAME_RE.fullmatch(name):
        raise AuditError(f"Недопустимое имя файла в архиве: {name[:120]}")
    return name

def _source_path_for_entry(entry: str) -> str:
    name = entry[: -len(".class")] if entry.endswith(".class") else entry
    return _validate_entry_name(f"{name}.jasm")

def _render_class_source(info: ClassInfo) -> str:
    """Render one decompiled pseudo-source file from constant-pool facts."""
    lines: list[str] = [f"v {info.major}.{info.minor}", f"c {info.name}"]
    if info.super_name:
        lines.append(f"s {info.super_name}")
    for iface in info.interfaces[:64]:
        lines.append(f"i {iface}")
    for ref in info.class_refs[:AUDIT_MAX_REFS_PER_CLASS]:
        lines.append(f"r {ref}")
    for ref in info.method_refs:
        lines.append(f"m {ref}")
    for ref in info.field_refs:
        lines.append(f"f {ref}")
    for value in info.strings:
        lines.append(f"k {json.dumps(value, ensure_ascii=False)}")
    lines.append("")
    return "\n".join(lines)

def _parse_simple_yaml(text: str) -> dict[str, str]:
    """Parse a flat YAML subset for top-level manifest keys."""
    root: dict[str, str] = {}
    stack: list[tuple[int, dict[str, str]]] = [(-1, root)]
    for raw_line in text.splitlines():
        if not raw_line.strip() or raw_line.lstrip().startswith("#"):
            continue
        line = raw_line.rstrip()
        indent = len(line) - len(line.lstrip(" "))
        content = line.strip()
        if content.startswith("- ") or ":" not in content:
            continue
        while stack and indent <= stack[-1][0]:
            stack.pop()
        if not stack:
            continue
        key, raw_value = content.split(":", 1)
        key = key.strip().strip("'\"")
        value = raw_value.strip()
        if value.startswith("[") and value.endswith("]"):
            value = value[1:-1]
        stack[-1][1][key] = value.strip("'\"") if value else ""
        if not value:
            stack.append((indent, {}))
    return root

def _manifest_metadata(name: str, data: bytes) -> dict[str, Any]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        text = data.decode("utf-8", errors="replace")
    metadata: dict[str, Any] = {"manifest": name}
    if name.endswith(".json"):
        try:
            document = json.loads(text)
        except (ValueError, RecursionError):
            return metadata
        if not isinstance(document, dict):
            return metadata
        inner = document.get("quilt_loader") if isinstance(document.get("quilt_loader"), dict) else document
        metadata["id"] = str(inner.get("id", ""))[:160]
        metadata["version"] = str(inner.get("version", ""))[:64]
        metadata["name"] = str(inner.get("name", ""))[:160]
        authors = inner.get("authors")
        if isinstance(authors, list):
            metadata["authors"] = [str(item)[:80] for item in authors[:12]]
        for key in ("homepage", "source"):
            if inner.get(key):
                metadata[key] = str(inner[key])[:300]
        entrypoints = inner.get("entrypoints")
        if isinstance(entrypoints, dict):
            metadata["entrypoints"] = sorted(
                str(value)[:200]
                for value in entrypoints.values()
                if isinstance(value, str)
            )[:16]
        depends = inner.get("depends")
        if isinstance(depends, dict):
            metadata["depends"] = sorted(str(key)[:80] for key in depends)[:24]
        return metadata

    values = _parse_simple_yaml(text)
    for key in ("name", "version", "main", "api-version", "bootstrapper", "author", "authors", "website", "description"):
        if values.get(key):
            metadata[key] = str(values[key])[:300]
    return metadata

def _declared_permissions_from_manifest(name: str, data: bytes) -> list[str]:
    if not name.endswith(".yml"):
        return []
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return []
    permissions: set[str] = set()
    for match in re.finditer(r"(?m)^\s+(?:-\s+)?([A-Za-z0-9.*:_.-]+)\s*:?\s*$", text):
        permissions.add(match.group(1)[:120])
    return sorted(permissions)[:64]

def _inspect_and_write_sources(
    archive: zipfile.ZipFile,
    sources_dir: Path,
    structure: JarStructure,
    *,
    prefix: str = "",
    depth: int = 0,
    nested: bool = False,
    total_unpacked: list[int],
) -> None:
    members = archive.infolist()
    if len(members) > AUDIT_MAX_ENTRIES:
        raise AuditError("В архиве слишком много файлов")
    manifests: dict[str, bytes] = {}

    for member in members:
        name = member.filename
        if member.is_dir():
            continue
        lowered = name.lower()
        if member.file_size > AUDIT_MAX_MEMBER_BYTES:
            structure.skipped_entries.append(f"{prefix}{name} (слишком большой)")
            continue
        if lowered.endswith(_NATIVE_EXTENSIONS):
            structure.natives.append(f"{prefix}{name}")
            continue
        if name.startswith("META-INF/") and lowered.endswith((".sf", ".rsa", ".dsa", ".ec")):
            structure.signed = True
            continue
        if not nested and name in _MANIFEST_PLATFORMS:
            if member.file_size > AUDIT_MAX_MANIFEST_BYTES:
                structure.skipped_entries.append(f"{prefix}{name} (manifest слишком большой)")
                continue
            manifests[name] = archive.read(member)
            platform = _MANIFEST_PLATFORMS[name]
            if _PLATFORM_PRIORITY.get(platform, 0) > _PLATFORM_PRIORITY.get(structure.platform, 0):
                structure.platform = platform
            continue
        if lowered.endswith(".jar"):
            if depth >= AUDIT_MAX_NESTED_DEPTH:
                structure.skipped_entries.append(f"{prefix}{name} (вложенность jar)")
                continue
            if len(structure.nested_jars) >= AUDIT_MAX_NESTED_JARS or member.file_size > AUDIT_MAX_NESTED_JAR_BYTES:
                structure.skipped_entries.append(f"{prefix}{name} (лимит вложенных jar)")
                continue
            structure.nested_jars.append(f"{prefix}{name}")
            _inspect_nested_jar(
                archive.read(member),
                f"{prefix}{name}",
                sources_dir,
                structure,
                depth=depth + 1,
                total_unpacked=total_unpacked,
            )
            continue
        if not lowered.endswith(".class"):
            if member.file_size > 64 * 1024:
                structure.skipped_entries.append(f"{prefix}{name} (не class-файл)")
            continue
        if member.file_size > AUDIT_MAX_CLASS_BYTES:
            structure.skipped_entries.append(f"{prefix}{name} (class слишком большой)")
            continue
        total_unpacked[0] += member.file_size
        if total_unpacked[0] > AUDIT_MAX_UNPACKED_BYTES:
            raise AuditError("Распакованный объём превышает лимит аудита")
        try:
            data = archive.read(name)
            info = parse_class_file(data, entry=f"{prefix}{name}")
            target_name = name if not prefix else _flatten_nested(prefix, name)
            target = sources_dir / _source_path_for_entry(target_name)
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            rendered = _render_class_source(info)
            target.write_text(rendered, encoding="utf-8")
        except (AuditError, KeyError, RuntimeError, OSError, ValueError) as exc:
            structure.skipped_entries.append(f"{prefix}{name} ({exc})")
            continue
        structure.class_count += 1
        structure.string_count += len(info.strings)
        structure.reference_count += len(info.method_refs) + len(info.field_refs) + len(info.class_refs)
        structure.sources_bytes += len(rendered)
        structure.source_lines += rendered.count("\n")

    if manifests:
        merged: dict[str, Any] = {}
        declared_permissions: set[str] = set()
        for name, data in sorted(manifests.items()):
            merged.update(_manifest_metadata(name, data))
            declared_permissions.update(_declared_permissions_from_manifest(name, data))
        if declared_permissions:
            merged["declared_permissions"] = sorted(declared_permissions)[:64]
        structure.declared = merged

def _flatten_nested(prefix: str, name: str) -> str:
    inner = prefix.rstrip("/")
    if not inner.endswith(".jar"):
        return f"{prefix}{name}"
    inner = inner[: -len(".jar")]
    inner = inner.replace("/", "__") + "__nested"
    return _validate_entry_name(f"{inner}/{name}")

def _inspect_nested_jar(
    data: bytes,
    label: str,
    sources_dir: Path,
    structure: JarStructure,
    *,
    depth: int,
    total_unpacked: list[int],
) -> None:
    total_unpacked[0] += len(data)
    if total_unpacked[0] > AUDIT_MAX_NESTED_TOTAL_BYTES:
        raise AuditError("Распакованный объём превышает лимит аудита")
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            _inspect_and_write_sources(
                archive,
                sources_dir,
                structure,
                prefix=f"{label}/",
                depth=depth,
                nested=True,
                total_unpacked=total_unpacked,
            )
    except zipfile.BadZipFile:
        structure.skipped_entries.append(f"{label} (не читается как zip)")

@dataclass(frozen=True)
class AuditRule:
    id: str
    category: str
    severity: str
    kind: str  # "c" class refs, "m" method refs, "f" field refs, "k" strings
    pattern: re.Pattern
    title: str
    detail: str
    recommendation: str

def _rule(
    rule_id: str,
    category: str,
    severity: str,
    kind: str,
    pattern: str,
    title: str,
    detail: str,
    recommendation: str,
) -> AuditRule:
    flags = re.DOTALL if kind == "k" else 0
    return AuditRule(
        id=rule_id,
        category=category,
        severity=severity,
        kind=kind,
        pattern=re.compile(pattern, flags),
        title=title,
        detail=detail,
        recommendation=recommendation,
    )

_SENSITIVE_ACCOUNT_FILES = (
    r"(?i)(launcher[_-]?accounts|launcher[_-]?profiles|accounts\.json|essentials?/userdata|"
    r"session[_-]?token|refresh[_-]?token|access[_-]?token|client[_-]?secret|logins\.json|"
    r"signons\.sqlite|key3\.db|key4\.db|id_rsa|known_hosts|\.aws/credentials|"
    r"webco?okies?\.?dat|privatetokens)"
)
_SENSITIVE_SYSTEM_FILES = (
    r"(?i)(/etc/(passwd|shadow|sudoers|hosts)\b|\\sam(\.bak)?$|system32\\config|"
    r"lsass|mimikatz|procdump|/proc/self/environ|\.docker/config\.json|\.kube/config|"
    r"wallet\.dat|keystore\.jks)"
)
_SENSITIVE_GAME_FILES = (
    r"(?i)(\.minecraft[/\\]|options\.txt|servers\.dat|usercache\.json|logs/latest\.log|"
    r"launcher_log|\.minecraft/logs|world/players|playerdata/|session\.lock|screenshots/)"
)
_EXFIL_ENDPOINTS = (
    r"(?i)(discord(?:app)?\.com/api/webhooks|discord\.com/api/v\d+/webhooks|api\.telegram\.org/bot|"
    r"pastebin\.com/api|hasteb(?:in|\.cc)|transfer\.sh|anonfiles|file\.io|gofile\.io|"
    r"hookb\.in|webhook\.site|requestbin|pipedream\.net|beeceptor|ngrok\.(io|com)|"
    r"trycloudflare\.com|loca\.lt|serveo\.net)"
)
_SHELL_STRINGS = (
    r"(?i)(/bin/(ba|z|da)?sh\b|cmd\.exe|powershell(\.exe)?\b|pwsh\b|mshta\s|certutil\s+-|"
    r"bitsadmin\s|certutil\.exe|bash\s+-c|sh\s+-c|reg\s+(add|query)|taskkill\s|net\s+user\b|"
    r"whoami(\.exe)?\b|nslookup\s|ping\s+-n|del\s+/[fqs]|rm\s+-rf)"
)
_PERSISTENCE_STRINGS = (
    r"(?i)(schtasks(\.exe)?\s|/etc/cron|crontab|CurrentVersion\\\\?Run|"
    r"/etc/rc\.local|systemd/system|launchd|~/\.bashrc|~/\.zshrc|"
    r"Startup\\\\?Folder|autoexec\.bat)"
)
# direct IP addresses are only suspicious when used as a connection target
# (with scheme or port); bare dotted numbers are usually version strings.
_RAW_IP_STRING = (
    r"(?i)(?:https?://(?:\d{1,3}\.){3}\d{1,3}(?:[:/]\S*)?|"
    r"\b(?:\d{1,3}\.){3}\d{1,3}:\d{2,5}\b)"
)
_ALLOWLISTED_HOSTS = re.compile(
    r"(?i)\b((?:api|meta)\.modrinth\.com|modrinth\.com|api\.hangar\.papermc\.io|hangar\.papermc\.io|"
    r"(?:hub|www)\.spigotmc\.org|api\.spiget\.org|api\.github\.com|github\.com|"
    r"raw\.githubusercontent\.com|objects\.githubusercontent\.com|api\.curseforge\.com|"
    r"minecraftforge\.net|maven\.minecraftforge\.net|meta\.fabricmc\.net|maven\.fabricmc\.net|"
    r"maven\.quiltmc\.org|quiltmc\.org|papermc\.io|api\.papermc\.io|repo\.papermc\.io|"
    r"bstats\.org|metrics\.bstats\.org|api\.sentry\.io|sentry\.io|"
    r"launcher\.mojang\.com|piston-meta\.mojang\.com|piston-data\.mojang\.com|"
    r"libraries\.minecraft\.net|resources\.download\.minecraft\.net)"
)

def _class_rules() -> list[AuditRule]:
    return [
        _rule("hardware-probe", "system", "notice", "c",
              r"(oshi/SystemInfo|oshi/hardware/[A-Za-z0-9$_]+|com/sun/manage/HotSpotDiagnosticMXBean)",
              "Инвентаризация железа",
              "Опрашивает характеристики процессора, памяти и дисков (OSHI/HotSpot).",
              "Иногда нужно для настроек производительности; иногда — сортировка жертв перед майнингом."),
        _rule("keylogger-native-hook", "keylogger", "critical", "c",
              r"(org/jnativehook/.*|com/github/kwhat/jnativehook/.*)",
              "Глобальный перехват клавиатуры и мыши",
              "Подключается к библиотеке глобальных хуков (JNativeHook), перехватывающей нажатия клавиш и действия мыши во всей системе, а не только в игре.",
              "Такая возможность не нужна обычным модам и плагинам. Не запускайте файл, пока не убедитесь в его происхождении."),
        _rule("keylogger-focus-manager", "keylogger", "warning", "c",
              r"(java/awt/KeyboardFocusManager|java/awt/event/KeyEventDispatcher|java/awt/event/KeyEventPostProcessor)",
              "Отслеживание фокуса и клавиатуры AWT",
              "Обращается к системному менеджеру клавиатурного фокуса, который может перехватывать все события клавиатуры приложения.",
              "Убедитесь, что это часть заявленной функциональности."),
        _rule("keyboard-game-input", "keylogger", "info", "c",
              r"(org/lwjgl/input/Keyboard|org/lwjgl/input/Mouse|net/minecraft/client/option/KeyBinding|com/mojang/blaze3d/platform/Input)",
              "Чтение игровых клавиш",
              "Работает с игровым вводом (привязки клавиш) — нормально для модов с хоткеями.",
              "Не является перехватом вне игры."),
        _rule("window-swing", "windows", "notice", "c",
              r"javax/swing/(JFrame|JDialog|JWindow|JOptionPane|JPopupMenu|JToolTip)",
              "Создание собственных окон (Swing)",
              "Может открывать собственные окна и диалоги поверх игры.",
              "Проверьте, что окна соответствуют заявленным функциям."),
        _rule("window-awt", "windows", "notice", "c",
              r"java/awt/(Frame|Dialog|Window|FileDialog|TrayIcon)",
              "Создание собственных окон (AWT)",
              "Может открывать системные окна, диалоги и значки в трее.",
              "Проверьте, что окна соответствуют заявленным функциям."),
        _rule("window-javafx", "windows", "notice", "c",
              r"javafx/(application/Application|stage/Stage|scene/Scene)",
              "Создание окон JavaFX",
              "Может открывать отдельные JavaFX-окна.",
              "Окна вне игры встречаются у лаунчер-модов; проверьте их назначение."),
        _rule("window-webview", "windows", "warning", "c",
              r"(javafx/scene/web/WebView|javax/swing/JEditorPane)",
              "Встроенный браузер",
              "Может показывать веб-страницы внутри собственного окна — вектор фишинга и скрытой загрузки.",
              "Убедитесь, что плагин не открывает сторонние страницы с запросом данных."),
        _rule("logging-frameworks", "logging", "info", "c",
              r"(java/util/logging/Logger|org/slf4j/Logger|org/apache/logging/log4j/(Logger|LogManager)|"
              r"org/jboss/logmanager/Logger|org/apache/commons/logging/Log)",
              "Использование логирования",
              "Работает со стандартными библиотеками логирования.",
              "Обычное поведение: моды и плагины пишут логи."),
        _rule("logging-file-handler", "logging", "notice", "c",
              r"java/util/logging/FileHandler",
              "Запись логов в файл",
              "Может создавать файлы журнала на диске.",
              "Проверьте, какие пути используются."),
        _rule("files-io", "files", "info", "c",
              r"(java/io/File|java/io/FileInputStream|java/io/FileOutputStream|java/io/RandomAccessFile|"
              r"java/io/FileReader|java/io/FileWriter|java/nio/file/Files|java/nio/file/Path|java/nio/file/Paths)",
              "Работа с файлами",
              "Использует файловый ввод-вывод — нормально для конфигураций и данных плагина.",
              "Обратите внимание на находки о чувствительных путях ниже."),
        _rule("files-zip", "files", "notice", "c",
              r"java/util/zip/(ZipFile|ZipInputStream|ZipOutputStream)",
              "Работа с ZIP-архивами",
              "Читает или создаёт ZIP-архивы.",
              "Создание архивов вместе с сетевой отправкой может означать упаковку данных для выгрузки."),
        _rule("network-url", "network", "notice", "c",
              r"(java/net/URL|java/net/URI|java/net/URLEncoder)",
              "Сетевые адреса",
              "Формирует сетевые адреса и URL.",
              "Само по себе не опасно; смотрите остальные находки сети."),
        _rule("network-http", "network", "notice", "c",
              r"(java/net/HttpURLConnection|java/net/http/HttpClient|okhttp3/OkHttpClient|"
              r"org/apache/http/(client|impl)/[A-Za-z0-9$]+|com/google/api/client/http/[A-Za-z0-9$]+)",
              "HTTP-клиент",
              "Может отправлять HTTP-запросы.",
              "Проверьте, куда отправляются данные."),
        _rule("network-socket", "network", "warning", "c",
              r"(java/net/Socket|java/nio/channels/SocketChannel)",
              "Прямые TCP-соединения",
              "Может устанавливать прямые сокет-соединения с произвольными адресами.",
              "Для большинства модов это нетипично."),
        _rule("network-listener", "network", "warning", "c",
              r"(java/net/ServerSocket|java/nio/channels/ServerSocketChannel)",
              "Открытие сетевого порта",
              "Может слушать входящие соединения на порту.",
              "Игровым модам и большинству плагинов это не нужно."),
        _rule("network-websocket", "network", "notice", "c",
              r"(java/net/http/WebSocket|org/java_websocket/[A-Za-z0-9$]+|io/socket/client/[A-Za-z0-9$]+)",
              "WebSocket-соединения",
              "Использует WebSocket-клиент.",
              "Часто встречается у чат-интеграций; проверьте адрес."),
        _rule("exec-script-engine", "process", "warning", "c",
              r"javax/script/ScriptEngineManager",
              "Встроенный движок скриптов",
              "Может исполнять JavaScript/Groovy-скрипты из строк.",
              "Скрипты из сети в сочетании с этим — признак скрытой загрузки кода."),
        _rule("exec-compiler", "process", "warning", "c",
              r"(javax/tools/JavaCompiler|com/sun/source/util/JavacTask|javax/tools/ToolProvider)",
              "Компиляция кода на лету",
              "Может компилировать Java-код во время работы.",
              "Легитимно лишь для специализированных инструментов."),
        _rule("exec-attach-agent", "process", "critical", "c",
              r"com/sun/tools/attach/[A-Za-z0-9$]+",
              "Подключение к чужой JVM",
              "Может внедряться в другие запущенные Java-процессы (java agent attach).",
              "Крайне опасно: так перехватывают чужие процессы, включая лаунчер."),
        _rule("reflection-unsafe", "reflection", "warning", "c",
              r"(sun/misc/Unsafe|jdk/internal/misc/Unsafe|sun/misc/Signal)",
              "Использование Unsafe/сигналов",
              "Обращается к низкоуровневым API в обход безопасности JVM.",
              "Может использоваться как для оптимизаций, так и для обхода защит."),
        _rule("native-jna", "native", "warning", "c",
              r"net/java/dev/jna/[A-Za-z0-9$]+",
              "Прямые вызовы нативных функций (JNA)",
              "Работает с нативным кодом напрямую через JNA.",
              "Оцените, какие нативные файлы лежат в самом JAR."),
        _rule("crypto-cipher", "crypto", "notice", "c",
              r"javax/crypto/(Cipher|SecretKey|spec/[A-Za-z0-9$]+)",
              "Шифрование данных",
              "Использует javax.crypto.",
              "Вместе с обфускацией или сетью шифрование может скрывать полезную нагрузку."),
        _rule("crypto-hash", "crypto", "info", "c",
              r"(java/security/MessageDigest|java/util/Base64)",
              "Хеширование и Base64",
              "Считает хеши или декодирует Base64.",
              "Обычное поведение; в сочетании с шифрованием строк может означать скрытие данных."),
        _rule("server-console-sender", "server", "notice", "c",
              r"(org/bukkit/command/ConsoleCommandSender|org/bukkit/command/CommandMap)",
              "Работа с консолью сервера",
              "Плагин может отправлять команды от имени консоли.",
              "Проверьте, какие команды выполняются."),
        _rule("chat-observer", "chat", "info", "c",
              r"(org/bukkit/event/player/AsyncPlayerChatEvent|io/papermc/paper/event/player/AsyncChatEvent|"
              r"org/bukkit/event/player/PlayerChatEvent)",
              "Чтение игрового чата",
              "Плагин видит сообщения игроков в чате.",
              "Нормально для чат-плагинов; вместе с сетью может означать пересылку переписки."),
        _rule("command-observer", "chat", "notice", "c",
              r"org/bukkit/event/player/PlayerCommandPreprocessEvent",
              "Наблюдение за командами игроков",
              "Плагин видит все команды, которые вводят игроки (включая пароли из /login и /register).",
              "В сочетании с сетью — канал утечки паролей."),
        _rule("telemetry-bstats", "telemetry", "info", "c",
              r"org/bstats/[A-Za-z0-9$/.]+",
              "Стандартная метрика bStats",
              "Использует bStats — распространённую метрику плагинов.",
              "Отправляет только анонимную статистику использования."),
        _rule("telemetry-analytics", "telemetry", "notice", "c",
              r"io/sentry/[A-Za-z0-9$/.]+",
              "Отчёты об ошибках (Sentry)",
              "Отправляет отчёты об ошибках на Sentry.",
              "Проверьте, настроен ли DSN на сервер автора."),
        _rule("jndi-context", "jndi", "warning", "c",
              r"javax/naming/(InitialContext|ldap/[A-Za-z0-9$]+)",
              "JNDI-вызовы",
              "Может выполнять удалённые JNDI/LDAP-вызовы (вектор атак класса Log4Shell).",
              "Не типично для игровых модов."),
        _rule("persistence-registry", "persistence", "notice", "c",
              r"java/util/prefs/Preferences",
              "Системный реестр/настройки",
              "Читает или пишет системные настройки пользователя.",
              "Само по себе допустимо; смотрите другие находки."),
        _rule("deserialization", "deserialization", "notice", "c",
              r"java/io/ObjectInputStream",
              "Десериализация Java-объектов",
              "Десериализует данные; уязвима к подложенным объектам из сети.",
              "Опасно в сочетании с сетевым вводом."),
        _rule("reflection-proxy-class", "reflection", "notice", "c",
              r"java/lang/reflect/Proxy",
              "Динамические прокси",
              "Создаёт прокси-объекты через рефлексию.",
              "Часто легитимно; в сочетании с загрузкой классов — подозрительно."),
    ]

def _string_rules() -> list[AuditRule]:
    return [
        _rule("keylogger-jna-hook", "keylogger", "critical", "k",
              r"(?i)(SetWindowsHookEx|GetAsyncKeyState|GetKeyState|user32\.dll|keybd_event|"
              r"RegisterRawInputDevices|XGrabKey|XQueryKeymap)",
              "Системный перехват клавиатуры",
              "Найдены вызовы Windows/Linux API для перехвата клавиш на уровне всей системы.",
              "Явный признак кейлоггера."),
        _rule("miner-pool-endpoints", "miner", "critical", "k",
              r"(?i)(stratum\+(tcp|ssl)|xmrig|xmr-stak|cryptonight|randomx\b|"
              r"pool\.(minexmr|supportxmr|nanopool|c3pool|2miners)\.org|"
              r"(xmr|eth|btc)\.(mine|pool)|nicehash\.com/api|hashrate.{0,20}worker)",
              "Признаки майнера криптовалют",
              "Найдены адреса майнинговых пулов или названия известных майнеров (XMRig, CryptoNight, RandomX).",
              "Игровым модам нечего считать хеши на вашем процессоре. Не запускайте файл."),
        _rule("wallet-files", "wallets", "critical", "k",
              r"(?i)(metamask|exodus[ /\\-]wallet|electrum\.wallet|atomic[ /\\-]wallet|"
              r"coinbase[ /\\]wallet|UTC--\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}|wallet\.dat|keystore\.json|"
              r"seed[ -]?phrase|mnemonic[ -]?phrase|bip39)",
              "Доступ к криптокошелькам",
              "Код обращается к файлам или формату криптокошельков (MetaMask, Exodus, keystore, seed-фразы).",
              "Легитимной причины у мода читать кошельки нет — это кража средств."),
        _rule("browser-credential-files", "wallets", "critical", "k",
              r"(?i)(Login Data( For Microsoft Edge)?|Web Data|cookies\.sqlite|"
              r"Local Storage[\\/]+leveldb|AppData[\\/]+Local[\\/]+(Google[\\/]+Chrome|"
              r"Microsoft[\\/]+Edge|BraveSoftware)|\.mozilla[\\/]firefox|Login Data For Chrome)",
              "Чтение данных браузеров",
              "Найдены пути к базам паролей и cookie-файлов браузеров (Chrome, Edge, Firefox).",
              "Мод не должен трогать профили браузеров — это извлечение паролей и сессий."),
        _rule("discord-session-theft", "wallets", "critical", "k",
              r"(?i)(discord(?:app)?[\\/]+Local Storage|discord\.com/api/users/@me|"
              r"discord\.com/api/v\d+/users/@me|Local Storage[\\/]+leveldb[\\/]+\S+\.ldb)",
              "Кража сессии Discord",
              "Код читает локальное хранилище Discord или обращается к API от имени пользователя.",
              "Так вытаскивают токен Discord, чтобы угнать аккаунт."),
        _rule("disk-wiper", "destruction", "critical", "k",
              r"(?i)(rm\s+-rf\s+[/~]|del\s+/[fqs].*:\\\w+|format\s+[c-z]:|"
              r"cipher\s+/w:|rd\s+/s\s+/q|shred\s+-|wipe\s+disk|mkfs\.\w+)",
              "Уничтожение данных",
              "Найдены команды удаления или форматирования больших областей диска.",
              "Такие команды в моде означают вандализм: файл будет стирать ваши данные."),
        _rule("obfuscation-marker", "obfuscation", "warning", "k",
              r"(?i)(allatori|zelix|klassmaster|stringer\s*demo|dexguard|dasho|smokescreen|"
              r"confuserex|obfuscat(ed|or))",
              "Признаки обфускатора",
              "В коде найдены строки-маркеры известных обфускаторов.",
              "Обфускация скрывает реальные имена классов и часто маскирует вредоносный код."),
        _rule("anti-analysis", "obfuscation", "warning", "k",
              r"(?i)(isDebuggerPresent|CheckRemoteDebuggerPresent|anti-?debug|VMwareDetect|"
              r"VirtualBoxDetector|sandbox_detect|frida_server)",
              "Анти-анализ и защита от отладки",
              "Код пытается обнаружить отладчик, виртуальную машину или анализ.",
              "Легитимный софт для игры так себя не ведёт."),
        _rule("mc-auth-urls", "tokens", "warning", "k",
              r"(?i)(login\.microsoftonline\.com|login\.live\.com|auth\.minecraftservices\.com|"
              r"sessionserver\.mojang\.com/session/minecraft/join|accounts\.google\.com/o/oauth2|"
              r"api\.mojang\.com/users/profiles)",
              "Обращение к сервисам авторизации",
              "Содержит адреса сервисов входа Microsoft/Mojang.",
              "Моды-авторизации так не работают: это может быть перехватом учётных данных."),
        _rule("tokens-launcher-config", "tokens", "warning", "k",
              r"(?i)(trial[-_]?launcher|launcher[_-]?settings\.json|launcher[_-]?config\.json|"
              r"launcher[_-]?accounts\.json)",
              "Доступ к файлам лаунчера",
              "Упоминает файлы и каталоги лаунчера с аккаунтами и настройками.",
              "Моду не нужны файлы лаунчера."),
        _rule("mc-session-files", "tokens", "warning", "k",
              r"(?i)((access|refresh)[-_]?token|msa[_-]?token|bearer\s+[a-z0-9._-]{8,}|"
              r"xuid|player[_-]?session)",
              "Игровые токены и сессии",
              "Упоминает маркеры сессий Minecraft/Microsoft.",
              "Перехват токена даёт доступ к аккаунту."),
        _rule("exfil-webhook", "network", "critical", "k",
              _EXFIL_ENDPOINTS,
              "Канал утечки данных",
              "Содержит жёстко прописанные адреса рассылки данных (Discord webhook, Telegram bot, pastebin и т.п.).",
              "Такие адреса внутри кода означают отправку данных автору кода."),
        _rule("network-raw-ip", "network", "warning", "k",
              _RAW_IP_STRING,
              "Прямой IP-адрес",
              "Содержит строку с прямым IP-адресом вместо домена.",
              "Обход DNS и блок-листов; часто встречается в малвари."),
        _rule("jndi-payload", "jndi", "critical", "k",
              r"\$\{jndi:(ldap|ldaps|rmi|dns|iiop)://",
              "JNDI-инъекция",
              "Содержит полезную нагрузку ${jndi:...} (класс Log4Shell).",
              "Эксплуатирует подстановку JNDI для удалённой загрузки кода."),
        _rule("persistence-os", "persistence", "critical", "k",
              _PERSISTENCE_STRINGS,
              "Закрепление в системе",
              "Создаёт автозапуск, задачи планировщика или правит системные скрипты.",
              "Моды и плагины так не распространяются."),
        _rule("exec-shell-string", "process", "critical", "k",
              _SHELL_STRINGS,
              "Команды операционной системы",
              "Найдены строки запуска системных команд и утилит (cmd, powershell, sh, certutil и т.п.).",
              "Модам почти никогда не нужны команды ОС."),
        _rule("self-update-download", "update", "notice", "k",
              r"(?i)(github\.com/[\w.-]+/[\w.-]+/releases/download|/latest\.jar|self[-_]?update|"
              r"autoupdate\.json|update\.json)",
              "Самообновление",
              "Содержит адреса автообновления и загрузки новых версий.",
              "Обновляемый код обходит эту аудиту и может измениться."),
        _rule("files-sensitive-account", "files", "critical", "k",
              _SENSITIVE_ACCOUNT_FILES,
              "Чтение учётных данных",
              "Ссылается на файлы аккаунтов, токенов, ключей и сессий.",
              "Моду не нужны ваши учётные данные."),
        _rule("files-sensitive-system", "files", "critical", "k",
              _SENSITIVE_SYSTEM_FILES,
              "Чтение системных файлов",
              "Ссылается на системные файлы с учётными данными ОС.",
              "Явный признак малвари."),
        _rule("files-sensitive-game", "files", "warning", "k",
              _SENSITIVE_GAME_FILES,
              "Чтение игровых данных вне своей папки",
              "Обращается к файлам .minecraft: настройки, логи, серверы, данные игроков.",
              "Часть данных (servers.dat, логи) может раскрывать приватную информацию."),
        _rule("files-executable-write", "files", "warning", "k",
              r"(?i)[\w./\\-]+\.(exe|msi|bat|cmd|ps1|vbs|scr)\Z",
              "Запись исполняемых файлов",
              "Ссылается на создание исполняемых файлов Windows.",
              "Моды не должны создавать исполняемые файлы ОС."),
        _rule("server-op-command", "server", "warning", "k",
              r"(?i)\A(op|minecraft:op)\s+\S+",
              "Выдача OP командой",
              "Содержит команду выдачи оператора с жёстко прописанным именем игрока.",
              "Признак бэкдора в плагине."),
        _rule("server-files", "server", "warning", "k",
              r"(?i)(ops\.json|usercache\.json|whitelist\.json)\b",
              "Правка служебных файлов сервера",
              "Обращается к файлам операторов, белого списка или кэша игроков напрямую.",
              "Обычно такие файлы меняет только серверное ПО."),
        _rule("rcon-client", "server", "notice", "k",
              r"(?i)\brcon\b",
              "RCON-подключение",
              "Работает с удалённым управлением сервером (RCON).",
              "Проверьте, куда подключается RCON и зачем."),
    ]

def _method_rules() -> list[AuditRule]:
    return [
        _rule("keylogger-robot", "keylogger", "critical", "m",
              r"java/awt/Robot\.(keyPress|keyRelease|mouseMove|mousePress|mouseRelease|mouseWheel|getPixelColor)",
              "Симуляция ввода через Robot",
              "Может программно нажимать клавиши и двигать мышь (java.awt.Robot).",
              "Используется для автоматизации ввода; в руках мода — управление игроком без вашего ведома."),
        _rule("env-harvest", "system", "notice", "m",
              r"java/lang/System\.(getenv|getProperty)",
              "Чтение переменных окружения и свойств системы",
              "Читает переменные окружения и системные свойства (System.getenv/getProperty).",
              "Само по себе типично (пути, версия Java), но вместе с сетью позволяет снять окружение с токенами."),
        _rule("dns-beacon", "network", "notice", "m",
              r"java/net/InetAddress\.(getAllByName|getByName|getHostName)",
              "DNS-запросы к произвольным именам",
              "Разрешает произвольные имена в адреса (InetAddress).",
              "Так устраивают DNS-маячки: сигнал «живой жертвы» уходит на подконтрольный домен."),
        _rule("screenshot-capture", "surveillance", "critical", "m",
              r"java/awt/Robot\.createScreenCapture",
              "Съёмка скриншотов экрана",
              "Делает снимки всего экрана.",
              "В сочетании с сетью — слежка за экраном пользователя."),
        _rule("keylogger-clipboard", "keylogger", "warning", "m",
              r"java/awt/datatransfer/Clipboard\.(getContents|setContents)",
              "Доступ к буферу обмена",
              "Читает или меняет системный буфер обмена.",
              "Чтение буфера позволяет копировать пароли и ссылки, скопированные вами."),
        _rule("window-desktop-open", "windows", "notice", "m",
              r"java/awt/Desktop\.(browse|open|openFile|openMail|mail)",
              "Открытие внешних окон",
              "Открывает ссылки, файлы или почту во внешних приложениях (браузер, проводник).",
              "Проверяйте, что открываются только заявленные адреса."),
        _rule("window-host-services", "windows", "notice", "m",
              r"javafx/application/HostServices\.showDocument",
              "Открытие ссылок JavaFX",
              "Открывает документы в браузере.",
              "Проверяйте открываемые адреса."),
        _rule("window-dispose", "windows", "info", "m",
              r"(java/awt/Window|javax/swing/JFrame|java/awt/Frame)\.dispose",
              "Закрытие окон",
              "Закрывает собственные окна приложения.",
              "Обычное поведение GUI."),
        _rule("window-event-queue", "windows", "warning", "m",
              r"java/awt/Toolkit\.getSystemEventQueue",
              "Доступ к очереди событий окон",
              "Получает системную очередь событий и может перехватывать или синтезировать события всех окон.",
              "Мощный API: используется кейлоггерами и автоматизацией."),
        _rule("close-system-exit", "close", "warning", "m",
              r"java/lang/System\.exit",
              "Завершение JVM",
              "Может принудительно закрыть игру, сервер или лаунчер.",
              "Проверьте, в каких ситуациях вызывается выход."),
        _rule("close-runtime-halt", "close", "warning", "m",
              r"java/lang/Runtime\.(halt|exit)",
              "Жёсткая остановка JVM",
              "Может немедленно завершить процесс без сохранения данных.",
              "Похоже на аварийное завершение или вредоносный выход."),
        _rule("close-process-destroy", "close", "warning", "m",
              r"java/lang/(Process|ProcessHandle)\.(destroy|destroyForcibly)",
              "Убийство процессов",
              "Может завершать другие процессы (включая игру, сервер, лаунчер).",
              "Легитимно для менеджеров серверов; иначе подозрительно."),
        _rule("close-game-shutdown", "close", "warning", "m",
              r"(net/minecraft/server/MinecraftServer\.(halt|stopServer|shutdownServer)|"
              r"org/bukkit/Bukkit\.shutdown|net/minecraft/client/Minecraft\.(scheduleStop|close))",
              "Остановка игры/сервера",
              "Может останавливать сервер или закрывать игровой клиент.",
              "Проверьте, является ли это заявленной функцией."),
        _rule("logging-get-logger", "logging", "info", "m",
              r"[A-Za-z0-9/$_]+\.getLogger",
              "Получение логгера",
              "Стандартное получение логгера.",
              "Не представляет риска."),
        _rule("logging-stdout", "logging", "info", "m",
              r"java/io/PrintStream\.(print|println|printf|write)",
              "Вывод в консоль",
              "Пишет текст в стандартный вывод (консоль).",
              "Не представляет риска."),
        _rule("files-dir-scan", "files", "notice", "m",
              r"(java/io/File\.(listFiles|list)|java/nio/file/Files\.(walk|walkFileTree|newDirectoryStream|find|lines))",
              "Перебор файлов и каталогов",
              "Мод сканирует содержимое каталогов, а не только читает отдельные файлы.",
              "Вместе с чувствительными путями или сетью — признак сбора данных."),
        _rule("files-config-read", "files", "info", "m",
              r"(org/bukkit/configuration/file/YamlConfiguration\.(load|save)|"
              r"org/bukkit/plugin/java/JavaPlugin\.(getConfig|saveConfig|saveDefaultConfig))",
              "Чтение собственной конфигурации",
              "Работает со своей конфигурацией Bukkit/YAML.",
              "Нормальное поведение плагина."),
        _rule("network-open-connection", "network", "notice", "m",
              r"(java/net/URL\.(openConnection|openStream)|java/net/URLConnection\.connect)",
              "Сетевые запросы",
              "Открывает сетевые соединения по URL.",
              "Проверьте, куда отправляются данные."),
        _rule("network-dns", "network", "info", "m",
              r"java/net/InetAddress\.(getByName|getAllByName|getLocalHost)",
              "Определение адресов (DNS)",
              "Резолвит имена хостов и локальный адрес.",
              "Может раскрывать ваш IP-адрес и используемые домены."),
        _rule("network-proxy-selector", "network", "notice", "m",
              r"java/net/(ProxySelector|Proxy)\.",
              "Настройка прокси",
              "Может перенаправлять трафик через прокси.",
              "Перехват трафика возможен при подмене прокси."),
        _rule("exec-runtime", "process", "critical", "m",
              r"java/lang/Runtime\.exec",
              "Запуск внешних программ",
              "Запускает произвольные команды операционной системы.",
              "Классический признак вредоносного кода в моде."),
        _rule("exec-process-builder", "process", "critical", "m",
              r"java/lang/ProcessBuilder\.(<init>|start|command)",
              "Запуск процессов (ProcessBuilder)",
              "Создаёт и запускает внешние процессы.",
              "Классический признак вредоносного кода в моде."),
        _rule("reflection-class-forname", "reflection", "notice", "m",
              r"java/lang/Class\.forName",
              "Загрузка классов по имени",
              "Загружает классы динамически по строковому имени.",
              "Часто легитимно; в сочетании с сетью — скрытая загрузка кода."),
        _rule("reflection-url-classloader", "reflection", "warning", "m",
              r"java/net/URLClassLoader\.(<init>|newInstance)",
              "Загрузка кода по URL",
              "Может загружать и исполнять Java-код из произвольных адресов или файлов.",
              "В сочетании с сетью — скачивание и запуск кода."),
        _rule("reflection-define-class", "reflection", "warning", "m",
              r"java/lang/ClassLoader\.defineClass",
              "Определение классов из байтов",
              "Конструирует классы напрямую из байтового кода.",
              "Используется для запуска кода, отсутствующего в файле."),
        _rule("reflection-method-invoke", "reflection", "notice", "m",
              r"java/lang/reflect/Method\.invoke",
              "Вызов методов через рефлексию",
              "Вызывает методы по имени через рефлексию.",
              "Часто легитимно; скрывает реальные вызовы от анализа."),
        _rule("reflection-set-accessible", "reflection", "info", "m",
              r"java/lang/reflect/(Field|Method|Constructor)\.setAccessible",
              "Снятие инкапсуляции",
              "Обращается к приватным полям Minecraft/сервера (стандартная практика модов).",
              "Не представляет риска само по себе."),
        _rule("reflection-module-opens", "reflection", "notice", "m",
              r"java/lang/Module\.(addOpens|addReads|open)",
              "Открытие модулей JDK",
              "Снимает изоляцию модулей Java.",
              "Иногда нужно для совместимости; иногда — для обхода защит."),
        _rule("reflection-proxy-new", "reflection", "notice", "m",
              r"java/lang/reflect/Proxy\.newProxyInstance",
              "Создание динамических прокси",
              "Подменяет реализации интерфейсов на лету.",
              "Часто легитимно; может подменять игровые объекты."),
        _rule("native-load", "native", "warning", "m",
              r"java/lang/System\.(load|loadLibrary)",
              "Загрузка нативного кода",
              "Загружает внешние библиотеки .dll/.so в процесс.",
              "Нативный код невидим для Java-анализа: относитесь к нему как к исполняемому файлу."),
        _rule("server-op-elevation", "server", "critical", "m",
              r"[A-Za-z0-9/$_]+\.setOp",
              "Выдача прав оператора (OP)",
              "Может выдавать себе или другим игрокам права оператора сервера.",
              "Явный признак бэкдора в плагине."),
        _rule("server-permission-grab", "server", "warning", "m",
              r"(org/bukkit/permissions/Permissible\.addAttachment|"
              r"org/bukkit/permissions/PermissionAttachment\.setPermission)",
              "Изменение прав игроков",
              "Может менять права доступа игроков на сервере.",
              "Проверьте, для кого и какие права выдаются."),
        _rule("server-dispatch-command", "server", "notice", "m",
              r"org/bukkit/Bukkit\.dispatchCommand",
              "Выполнение команд от имени консоли",
              "Выполняет команды сервера от имени консоли.",
              "Легитимно для многих плагинов; важно, какие команды."),
        _rule("server-plugin-manager", "server", "notice", "m",
              r"org/bukkit/plugin/PluginManager\.(disablePlugin|enablePlugin)",
              "Управление другими плагинами",
              "Может отключать или включать другие плагины сервера.",
              "Может маскировать конфликты или вредоносную активность."),
        _rule("deserialization-read", "deserialization", "notice", "m",
              r"java/io/ObjectInputStream\.readObject",
              "Чтение сериализованных объектов",
              "Десериализует объекты — опасно, если источник данных сетевой.",
              "Уязвимость класса «deserialization gadget»."),
    ]

def _field_rules() -> list[AuditRule]:
    return [
        _rule("logging-system-fields", "logging", "info", "f",
              r"java/lang/System\.(out|err)",
              "Стандартный вывод",
              "Использует поля стандартного вывода.",
              "Обычный вывод в консоль."),
    ]

_RULES: list[AuditRule] = _class_rules() + _string_rules() + _method_rules() + _field_rules()
_RULES_BY_KIND: dict[str, tuple[AuditRule, ...]] = {
    kind: tuple(rule for rule in _RULES if rule.kind == kind)
    for kind in ("c", "m", "f", "k")
}
# line kinds c/s/i/r all describe class references
_CLASS_LINE_KINDS = frozenset({"c", "s", "i", "r"})

AUDIT_CATEGORIES: list[dict[str, str]] = [
    {"id": category, "title": title}
    for category, title in (
        ("keylogger", "Клавиатурные шпионы, перехват ввода и буфера обмена"),
        ("surveillance", "Съёмка скриншотов экрана"),
        ("miner", "Майнеры криптовалют"),
        ("wallets", "Кража кошельков, данных браузеров и Discord"),
        ("destruction", "Уничтожение данных на диске"),
        ("system", "Инвентаризация системы и окружения"),
        ("windows", "Открытие окон, ссылок и встроенных браузеров"),
        ("close", "Закрытие окон, завершение игры, сервера и процессов"),
        ("logging", "Логирование и запись логов"),
        ("files", "Работа с файлами и сканирование каталогов"),
        ("network", "Сетевая активность, прямые IP и открытые порты"),
        ("process", "Запуск процессов и команд ОС"),
        ("reflection", "Рефлексия и динамическая загрузка кода"),
        ("native", "Встроенный нативный код (.dll/.so)"),
        ("obfuscation", "Обфускация, шифрование строк и анти-анализ"),
        ("crypto", "Шифрование и кодирование данных"),
        ("server", "Захват сервера: OP-права, команды консоли"),
        ("tokens", "Кража аккаунтов, токенов и сессий"),
        ("chat", "Перехват чата и команд игроков"),
        ("telemetry", "Телеметрия и аналитика"),
        ("persistence", "Закрепление в системе (автозапуск, задачи)"),
        ("jndi", "JNDI/LDAP-инъекции (Log4Shell)"),
        ("deserialization", "Опасная десериализация"),
        ("update", "Самообновление и загрузка кода"),
        ("combo", "Комбинированные схемы (ввод/файлы + сеть и др.)"),
        ("structure", "Структура архива, манифесты и подписи"),
    )
]

_OBFUSCATED_NAME_RE = re.compile(r"[a-z]{1,3}\d?", re.ASCII)

def _structural_findings(structure: JarStructure, source_files: list[Path]) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []

    def add(rule_id: str, category: str, severity: str, title: str, detail: str, recommendation: str, evidence: list[dict[str, str]]) -> None:
        findings.append(
            {
                "id": rule_id,
                "category": category,
                "severity": severity,
                "title": title,
                "detail": detail,
                "recommendation": recommendation,
                "evidence": evidence[:_MAX_EVIDENCE_PER_FINDING],
            }
        )

    if structure.natives:
        listed = ", ".join(structure.natives[:8])
        extra = f" (+{len(structure.natives) - 8})" if len(structure.natives) > 8 else ""
        add(
            "native-bundled", "native", "critical",
            "В JAR лежат нативные библиотеки",
            f"Внутри архива найдены файлы нативного кода: {listed}{extra}. Их содержимое не декомпилируется и исполняется на уровне ОС.",
            "Нативный код исполняется напрямую вашей системой и не проверяется правилами Java. Убедитесь в доверии к источнику.",
            [{"class": name, "value": "нативная библиотека"} for name in structure.natives],
        )

    if structure.class_count >= 25 and source_files:
        class_names = [path.stem for path in source_files]
        suspicious = sum(1 for name in class_names if _OBFUSCATED_NAME_RE.fullmatch(name))
        if suspicious / len(class_names) > 0.5:
            add(
                "obfuscation-names", "obfuscation", "warning",
                "Обфусцированные имена классов",
                f"{suspicious} из {len(class_names)} классов имеют короткие нечитаемые имена (a, b, c1…). Это признак обфускации кода.",
                "Обфускация не делает код вредоносным, но скрывает его назначение от любой проверки.",
                [{"class": name, "value": "имя класса"} for name in sorted(class_names)[:_MAX_EVIDENCE_PER_FINDING]],
            )

    if not structure.declared and structure.class_count > 0:
        add(
            "no-manifest", "structure", "notice",
            "Нет манифеста мода или плагина",
            "В архиве отсутствуют plugin.yml, fabric.mod.json, mods.toml и другие стандартные манифесты.",
            "Легальные дистрибутивы модов и плагинов почти всегда содержат манифест.",
            [],
        )

    if structure.signed:
        add(
            "signed-jar", "structure", "info",
            "Архив подписан",
            "JAR содержит подписи META-INF (*.SF/*.RSA). Это подтверждает целостность файла, но не его безопасность: подпись может принадлежать кому угодно.",
            "Проверьте, кому принадлежит сертификат подписи.",
            [],
        )

    if len(structure.nested_jars) > 40:
        add(
            "many-nested-jars", "structure", "notice",
            f"Много вложенных jar: {len(structure.nested_jars)}",
            "Внутри архива более 40 вложенных jar-файлов. Это нетипично даже для shaded-зависимостей.",
            "Проверьте список вложенных библиотек.",
            [{"class": name, "value": "вложенный jar"} for name in structure.nested_jars],
        )

    if structure.skipped_entries:
        listed = "; ".join(structure.skipped_entries[:6])
        extra = f" (+{len(structure.skipped_entries) - 6})" if len(structure.skipped_entries) > 6 else ""
        add(
            "skipped-entries", "structure", "notice",
            f"Пропущено записей при разборе: {len(structure.skipped_entries)}",
            f"Часть файлов не удалось декомпилировать: {listed}{extra}. Их содержимое не проверялось.",
            "Неполная проверка снижает гарантию отчёта.",
            [{"class": entry.split(" (")[0], "value": entry} for entry in structure.skipped_entries],
        )

    declared_permissions = structure.declared.get("declared_permissions")
    if isinstance(declared_permissions, list) and any(item in {"*", "**"} for item in declared_permissions):
        add(
            "server-wildcard-permissions", "server", "warning",
            "Плагин запрашивает права-подстановку",
            "Манифест объявляет permission «*» — полный доступ ко всем правам сервера.",
            "Проверьте, действительно ли плагину нужны все права.",
            [],
        )
    return findings

_COMBO_RULES: list[tuple[str, str, str, Callable[[set[str], set[str]], bool], str, str, str]] = [
    (
        "combo-keylogger-network", "combo", "critical",
        lambda ids, cats: "keylogger" in cats and "exfil-webhook" in ids,
        "Перехват ввода + канал утечки",
        "Код совмещает перехват клавиатуры/экрана с жёстко прописанными каналами отправки данных. Схема типичного шпиона.",
        "Настоятельно не рекомендуется запускать этот файл.",
    ),
    (
        "combo-input-network", "combo", "warning",
        lambda ids, cats: "keylogger" in cats and "network" in cats and "exfil-webhook" not in ids,
        "Перехват ввода + сеть",
        "Код читает ввод пользователя и одновременно имеет сетевые возможности.",
        "Комбинация позволяет отправлять перехваченные данные наружу.",
    ),
    (
        "combo-sensitive-network", "combo", "critical",
        lambda ids, cats: bool(ids & {"files-sensitive-account", "files-sensitive-system"}) and "network" in cats,
        "Чтение учётных данных + сеть",
        "Код ссылается на файлы с паролями/токенами и одновременно может отправлять данные в сеть.",
        "Схема кражи учётных данных.",
    ),
    (
        "combo-dynamic-network", "combo", "critical",
        lambda ids, cats: bool(ids & {"reflection-url-classloader", "reflection-define-class"}) and "network" in cats,
        "Динамическая загрузка кода + сеть",
        "Код может загружать классы из байтов или URL и имеет сетевые возможности — то есть скачивать и запускать новый код.",
        "Загруженный код не проходит эту проверку.",
    ),
    (
        "combo-exec-network", "combo", "critical",
        lambda ids, cats: bool(ids & {"exec-runtime", "exec-process-builder", "exec-shell-string"}) and "network" in cats,
        "Запуск команд ОС + сеть",
        "Код запускает процессы ОС и работает с сетью — схема дроппера.",
        "Классическое поведение загрузчиков вредоносного кода.",
    ),
    (
        "combo-command-network", "combo", "critical",
        lambda ids, cats: "command-observer" in ids and bool(ids & {"network-socket", "exfil-webhook", "network-http"}),
        "Перехват команд игроков + сеть",
        "Плагин видит команды игроков (включая пароли /login) и может отправлять их в сеть.",
        "Кража паролей с сервера.",
    ),
    (
        "combo-update-exec", "combo", "critical",
        lambda ids, cats: "self-update-download" in ids and bool(
            ids & {"reflection-url-classloader", "reflection-define-class", "exec-runtime", "exec-process-builder"}
        ),
        "Самообновление + запуск кода",
        "Код скачивает обновления и умеет исполнять загруженный код.",
        "Обновлённый код обходит эту проверку и может быть чем угодно.",
    ),
    (
        "combo-obfuscated-suspicious", "combo", "warning",
        lambda ids, cats: "obfuscation" in cats and any(
            rule in ids
            for rule in (
                "keylogger-native-hook", "keylogger-jna-hook", "keylogger-robot", "exec-runtime",
                "exec-process-builder", "exec-shell-string", "exfil-webhook", "reflection-url-classloader",
                "persistence-os", "files-sensitive-account",
            )
        ),
        "Обфускация + опасные сигналы",
        "Опасные вызовы скрыты за обфускацией кода.",
        "Сочетание резко повышает риск: автор намеренно скрывал поведение.",
    ),
    (
        "combo-tokens-network", "combo", "critical",
        lambda ids, cats: bool(ids & {"mc-session-files", "tokens-launcher-config", "mc-auth-urls"}) and "network" in cats,
        "Токены аккаунтов + сеть",
        "Код обращается к маркерам сессий и одновременно работает с сетью.",
        "Возможна отправка ваших игровых токенов злоумышленнику.",
    ),
    (
        "combo-screenshot-network", "combo", "critical",
        lambda ids, cats: "screenshot-capture" in ids and "network" in cats,
        "Съёмка экрана + сеть",
        "Код снимает экран и может отправлять данные в сеть.",
        "Слежка за экраном пользователя.",
    ),
    (
        "combo-chat-network", "combo", "warning",
        lambda ids, cats: "chat-observer" in ids and bool(ids & {"network-socket", "exfil-webhook"}),
        "Чтение чата + канал утечки",
        "Плагин читает чат и содержит адреса отправки данных.",
        "Пересылка переписки игроков третьим лицам.",
    ),
    (
        "combo-miner-network", "combo", "critical",
        lambda ids, cats: "miner" in cats and "network" in cats,
        "Майнер + сеть",
        "Код содержит признаки майнинга и умеет работать с сетью: пул, задачи и статистика ходят по соединению.",
        "Ставить такой мод — отдать процессор чужому майнингу.",
    ),
    (
        "combo-miner-exec", "combo", "critical",
        lambda ids, cats: "miner" in cats and bool(ids & {"exec-runtime", "exec-process-builder", "exec-shell-string"}),
        "Майнер + запуск процессов",
        "Признаки майнинга совмещены с запуском процессов ОС: часто это запуск внешнего майнера-исполняемого файла.",
        "Вредоносный файл может остаться в системе после удаления мода.",
    ),
    (
        "combo-wallet-network", "combo", "critical",
        lambda ids, cats: "wallets" in cats and "network" in cats,
        "Кошельки и данные браузеров + сеть",
        "Код читает файлы кошельков/браузеров/Discord и одновременно работает с сетью.",
        "Это законченная схема кражи: сначала читает, потом отправляет.",
    ),
    (
        "combo-env-exfil", "combo", "warning",
        lambda ids, cats: "env-harvest" in ids and bool(ids & {"network-http", "network-socket", "exfil-webhook"}),
        "Сбор окружения + сеть",
        "Код снимает переменные окружения и может отправлять их наружу.",
        "В переменных окружения часто лежат токены CI, API-ключи и пароли.",
    ),
]

_CPU_BENCHMARK_MIB = 4
_cpu_benchmark_cache: tuple[float, float] = (0.0, 0.0)

def _cpu_benchmark() -> float:
    """Measure hashing throughput in MiB/s; result cached briefly."""
    global _cpu_benchmark_cache
    now = time.monotonic()
    cached_at, cached_value = _cpu_benchmark_cache
    if cached_at and now - cached_at < 600:
        return cached_value
    block = os.urandom(512 * 1024)
    started = time.perf_counter()
    for _ in range(_CPU_BENCHMARK_MIB * 2):
        hashlib.sha256(block).digest()
    elapsed = time.perf_counter() - started
    value = _CPU_BENCHMARK_MIB / max(elapsed, 1e-6)
    _cpu_benchmark_cache = (now, value)
    return value

def _sample_lines(sources_dir: Path) -> list[str]:
    lines: list[str] = []
    files = sorted(sources_dir.rglob("*.jasm"))
    if not files:
        return lines
    step = max(1, len(files) // 24)
    for path in files[::step]:
        try:
            with path.open("r", encoding="utf-8", errors="replace") as handle:
                remaining = _ESTIMATE_SAMPLE_LINES - len(lines)
                for line in handle:
                    lines.append(line[:AUDIT_MAX_SOURCE_LINE])
                    remaining -= 1
                    if remaining <= 0:
                        break
        except OSError:
            continue
        if len(lines) >= _ESTIMATE_SAMPLE_LINES:
            break
    return lines

def _run_rules_on_line(line: str) -> None:
    """Apply the rule set to one IR line (used for scanning and estimation)."""
    kind = line[:1]
    rest = line[1:].strip()
    if len(rest) > AUDIT_MAX_SOURCE_LINE:
        rest = rest[:AUDIT_MAX_SOURCE_LINE]
    if kind == "k":
        value = rest
        if value.startswith('"'):
            try:
                value = json.loads(value)
            except ValueError:
                pass
        if _ALLOWLISTED_HOSTS.search(value):
            return
        for rule in _RULES_BY_KIND["k"]:
            if rule.pattern.search(value):
                return
        return
    if kind in _CLASS_LINE_KINDS:
        rules = _RULES_BY_KIND["c"]
    else:
        rules = _RULES_BY_KIND.get(kind, ())
    for rule in rules:
        if rule.pattern.fullmatch(rest):
            return

def _measure_lines_per_second(sample: Sequence[str]) -> float:
    if not sample:
        return 20000.0
    started = time.perf_counter()
    for line in sample:
        _run_rules_on_line(line)
    elapsed = time.perf_counter() - started
    return len(sample) / max(elapsed, 1e-6)

def format_estimate(seconds: int) -> str:
    if seconds < 60:
        return f"около {seconds} с"
    minutes, sec = divmod(seconds, 60)
    if minutes < 60:
        return f"около {minutes} мин" + (f" {sec} с" if sec else "")
    hours, minutes = divmod(minutes, 60)
    return f"около {hours} ч {minutes} мин"

class _FindingAccumulator:
    def __init__(self) -> None:
        self.findings: dict[str, dict[str, Any]] = {}

    def add(self, rule: AuditRule, source: str, value: str) -> None:
        finding = self.findings.get(rule.id)
        if finding is None:
            finding = {
                "id": rule.id,
                "category": rule.category,
                "severity": rule.severity,
                "title": rule.title,
                "detail": rule.detail,
                "recommendation": rule.recommendation,
                "evidence": [],
            }
            self.findings[rule.id] = finding
        if len(finding["evidence"]) < _MAX_EVIDENCE_PER_FINDING:
            finding["evidence"].append({"class": source, "value": value[:300]})

    def result(self) -> list[dict[str, Any]]:
        return sorted(
            self.findings.values(),
            key=lambda item: (SEVERITY_ORDER.index(item["severity"]), item["category"], item["id"]),
        )

_KNOWN_HOST_RULE = AuditRule(
    id="network-known-host",
    category="network",
    severity="info",
    kind="k",
    pattern=_ALLOWLISTED_HOSTS,
    title="Обращение к известным сервисам",
    detail="Найдены адреса известных сервисов (Modrinth, PaperMC, GitHub, bStats, Mojang и т.п.) — типично для ссылок и обновлений.",
    recommendation="Не представляет риска.",
)

def _iter_source_files(sources_dir: Path) -> list[Path]:
    if not sources_dir.is_dir():
        return []
    return sorted(path for path in sources_dir.rglob("*.jasm") if path.is_file())

def scan_sources(
    sources_dir: Path,
    structure: JarStructure,
    *,
    progress: Callable[[int, str], None] | None = None,
    cancelled: Callable[[], bool] | None = None,
) -> dict[str, Any]:
    """Run the rule engine over decompiled sources and build the report."""
    started = time.monotonic()
    accumulator = _FindingAccumulator()
    files = _iter_source_files(sources_dir)
    if not files:
        raise AuditError("Декомпилированные исходники не найдены или уже уничтожены")
    total_bytes = sum(path.stat().st_size for path in files) or 1
    done_bytes = 0
    for index, path in enumerate(files):
        if cancelled is not None and cancelled():
            raise AuditError("Проверка отменена пользователем")
        source_name = str(path.relative_to(sources_dir))
        try:
            with path.open("r", encoding="utf-8", errors="replace") as handle:
                for line in handle:
                    kind = line[:1]
                    rest = line[1:].strip()
                    if len(rest) > AUDIT_MAX_SOURCE_LINE:
                        rest = rest[:AUDIT_MAX_SOURCE_LINE]
                    if kind == "k":
                        value = rest
                        if value.startswith('"'):
                            try:
                                value = json.loads(value)
                            except ValueError:
                                pass
                        allowlisted = _ALLOWLISTED_HOSTS.search(value)
                        if allowlisted:
                            accumulator.add(_KNOWN_HOST_RULE, source_name, allowlisted.group(0))
                        for rule in _RULES_BY_KIND["k"]:
                            if rule.pattern.search(value):
                                accumulator.add(rule, source_name, str(value)[:200])
                        continue
                    if kind in _CLASS_LINE_KINDS:
                        rules = _RULES_BY_KIND["c"]
                    else:
                        rules = _RULES_BY_KIND.get(kind, ())
                    if not rules:
                        continue
                    for rule in rules:
                        if rule.pattern.fullmatch(rest):
                            accumulator.add(rule, source_name, rest)
        except OSError as exc:
            structure.skipped_entries.append(f"{source_name} ({exc})")
            continue
        done_bytes += path.stat().st_size
        if progress is not None and (index % 25 == 0 or index == len(files) - 1):
            progress(min(99, int(done_bytes * 100 / total_bytes)), source_name)

    findings = accumulator.result()
    findings.extend(_structural_findings(structure, files))

    notable = [finding for finding in findings if finding["severity"] != "info"]
    finding_ids = {finding["id"] for finding in notable}
    categories = {finding["category"] for finding in findings}
    notable_categories = {finding["category"] for finding in notable}
    for rule_id, category, severity, condition, title, detail, recommendation in _COMBO_RULES:
        if condition(finding_ids, notable_categories):
            findings.append(
                {
                    "id": rule_id,
                    "category": category,
                    "severity": severity,
                    "title": title,
                    "detail": detail,
                    "recommendation": recommendation,
                    "evidence": [],
                }
            )
            categories.add(category)

    counts = {severity: 0 for severity in SEVERITY_ORDER}
    for finding in findings:
        counts[finding["severity"]] += 1
    risk_score = min(100, sum(SEVERITY_SCORE[finding["severity"]] for finding in findings))
    if counts["critical"] or risk_score >= 40:
        verdict = "dangerous"
    elif counts["warning"] or risk_score >= 15:
        verdict = "suspicious"
    elif counts["notice"] >= 3:
        verdict = "review"
    else:
        verdict = "clean"
    findings.sort(key=lambda item: (SEVERITY_ORDER.index(item["severity"]), item["category"], item["id"]))

    return {
        "findings": findings,
        "counts": counts,
        "risk_score": risk_score,
        "verdict": verdict,
        "verdict_label": VERDICT_LABELS[verdict],
        "categories": sorted(categories),
        "scanned_files": len(files),
        "duration_seconds": round(time.monotonic() - started, 3),
    }

def _secure_wipe(directory: Path) -> None:
    """Overwrite every file with random bytes before removing it."""
    try:
        entries = sorted(directory.rglob("*"), reverse=True)
    except OSError:
        entries = []
    for path in entries:
        try:
            metadata = path.lstat()
        except OSError:
            continue
        if stat.S_ISLNK(metadata.st_mode):
            try:
                path.unlink()
            except OSError:
                pass
            continue
        if stat.S_ISREG(metadata.st_mode):
            try:
                with path.open("r+b") as handle:
                    remaining = metadata.st_size
                    while remaining > 0:
                        chunk = os.urandom(min(remaining, 1024 * 1024))
                        handle.write(chunk)
                        remaining -= len(chunk)
                    handle.flush()
                    os.fsync(handle.fileno())
            except OSError:
                pass
            try:
                path.unlink()
            except OSError:
                try:
                    path.chmod(0o600)
                    path.unlink()
                except OSError:
                    pass
        elif stat.S_ISDIR(metadata.st_mode):
            try:
                path.rmdir()
            except OSError:
                pass
    try:
        directory.rmdir()
    except OSError:
        shutil.rmtree(directory, ignore_errors=True)

class ModAuditStore:
    """Degrade-safe session store: prepare → (accept | cancel) with mandatory wipe."""

    def __init__(self, root: Path) -> None:
        self.root = Path(root)
        if self.root.is_symlink():
            raise AuditError("Каталог аудита является символьной ссылкой")
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)

    def _session_dir(self, session_id: str) -> Path:
        if not SESSION_ID_RE.fullmatch(session_id):
            raise AuditError("Некорректный идентификатор сессии аудита")
        return self.root / session_id

    def _read_record(self, session_id: str) -> dict[str, Any]:
        directory = self._session_dir(session_id)
        record_path = directory / "session.json"
        if record_path.is_symlink() or not record_path.is_file():
            raise AuditError("Сессия аудита не найдена")
        if record_path.stat().st_size > AUDIT_MAX_MANIFEST_BYTES:
            raise AuditError("Файл сессии аудита повреждён")
        try:
            record = json.loads(record_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise AuditError("Файл сессии аудита повреждён") from exc
        if not isinstance(record, dict) or type(record.get("state")) is not str:
            raise AuditError("Файл сессии аудита повреждён")
        return record

    def _write_record(self, directory: Path, record: dict[str, Any]) -> None:
        payload = json.dumps(record, ensure_ascii=False, indent=2).encode("utf-8") + b"\n"
        record_path = directory / "session.json"
        temporary = directory / f".session.{secrets.token_hex(6)}.tmp"
        try:
            with temporary.open("wb") as handle:
                handle.write(payload)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, record_path)
        finally:
            if temporary.exists():
                temporary.unlink(missing_ok=True)

    def cleanup(self) -> int:
        """Remove stale sessions; returns the number of removed sessions."""
        removed = 0
        now = time.time()
        try:
            entries = list(self.root.iterdir())
        except OSError:
            return removed
        for entry in entries:
            if not SESSION_ID_RE.fullmatch(entry.name):
                continue
            try:
                record = self._read_record(entry.name)
            except AuditError:
                record = None
            try:
                created = float(record.get("created_at", 0)) if record else entry.stat().st_mtime
            except (OSError, TypeError, ValueError):
                created = 0.0
            state = str(record.get("state", "")) if record else "unknown"
            sources_present = (entry / "sources").exists()
            ttl = SESSION_TTL_SECONDS if sources_present or state == "awaiting" else SESSION_RESULT_TTL_SECONDS
            if now - created > ttl:
                _secure_wipe(entry)
                removed += 1
        return removed

    def _limit_active(self) -> None:
        sessions: list[tuple[float, str]] = []
        try:
            for entry in list(self.root.iterdir()):
                if not SESSION_ID_RE.fullmatch(entry.name):
                    continue
                try:
                    record = self._read_record(entry.name)
                except AuditError:
                    continue
                if record.get("state") == "awaiting":
                    sessions.append((float(record.get("created_at", 0) or 0), entry.name))
        except OSError:
            return
        sessions.sort()
        while len(sessions) > SESSION_MAX_ACTIVE:
            _, oldest = sessions.pop(0)
            directory = self.root / oldest
            if directory.exists():
                _secure_wipe(directory)

    def prepare(self, jar_path: Path | str, *, label: str = "", kind: str = "") -> dict[str, Any]:
        """Decompile a JAR into a private session and estimate the scan time."""
        self.cleanup()
        try:
            path = Path(jar_path).expanduser()
        except (TypeError, ValueError) as exc:
            raise AuditError("Некорректный путь к файлу") from exc
        if path.is_symlink() or not path.is_file():
            raise AuditError("Файл не найден")
        try:
            size = path.stat().st_size
        except OSError as exc:
            raise AuditError("Не удалось прочитать файл") from exc
        if size <= 0 or size > AUDIT_MAX_JAR_BYTES:
            raise AuditError(f"Размер файла должен быть от 1 байта до {AUDIT_MAX_JAR_BYTES // (1024 * 1024)} MiB")

        session_id = secrets.token_hex(16)
        directory = self.root / session_id
        directory.mkdir(mode=0o700)
        sources_dir = directory / "sources"
        sources_dir.mkdir(mode=0o700)
        structure = JarStructure()
        sha256 = hashlib.sha256()
        try:
            with path.open("rb") as handle:
                while True:
                    chunk = handle.read(1024 * 1024)
                    if not chunk:
                        break
                    sha256.update(chunk)
            try:
                with zipfile.ZipFile(path) as archive:
                    _inspect_and_write_sources(archive, sources_dir, structure, total_unpacked=[0])
            except zipfile.BadZipFile as exc:
                raise AuditError("Файл не является корректным ZIP/JAR архивом") from exc
            except (OSError, RuntimeError) as exc:
                raise AuditError(f"Не удалось разобрать архив: {exc}") from exc

            if structure.class_count == 0:
                raise AuditError("В архиве нет Java class-файлов: проверять нечего")

            sample = _sample_lines(sources_dir)
            lines_per_second = _measure_lines_per_second(sample)
            estimated = int(structure.source_lines / max(lines_per_second, 1.0)) + 1
            estimated = max(_ESTIMATE_MIN_SECONDS, min(_ESTIMATE_MAX_SECONDS, estimated))
            if not kind:
                kind = "plugin" if structure.platform in {"bukkit", "paper", "bungee"} else "mod"
            record = {
                "session_id": session_id,
                "created_at": time.time(),
                "state": "awaiting",
                "source": {
                    "path": str(path),
                    "label": label[:160],
                    "kind": kind[:32],
                    "sha256": sha256.hexdigest(),
                    "size": size,
                },
                "structure": self._structure_payload(structure),
                "estimate": {
                    "seconds": estimated,
                    "label": format_estimate(estimated),
                    "source_lines": structure.source_lines,
                    "measured_lines_per_second": round(lines_per_second, 1),
                    "cpu_mib_per_second": round(_cpu_benchmark(), 1),
                },
            }
            self._write_record(directory, record)
        except BaseException:
            if directory.exists():
                _secure_wipe(directory)
            raise
        self._limit_active()
        return {
            "session_id": session_id,
            "state": "awaiting",
            "source": record["source"],
            "structure": record["structure"],
            "estimate": record["estimate"],
            "checks": [dict(item) for item in AUDIT_CATEGORIES],
            "checks_count": len(AUDIT_CATEGORIES),
        }

    def accept(
        self,
        session_id: str,
        *,
        progress: Callable[[int, str], None] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ) -> dict[str, Any]:
        """Run the scan, then destroy the decompiled sources regardless of outcome."""
        directory = self._session_dir(session_id)
        record = self._read_record(session_id)
        if record.get("state") != "awaiting":
            raise AuditError("Проверка этой сессии уже завершена или отменена")
        sources_dir = directory / "sources"
        was_cancelled = False
        try:
            structure = self._structure_from_record(record)
            report = scan_sources(sources_dir, structure, progress=progress, cancelled=cancelled)
        except AuditError as exc:
            was_cancelled = "отменена" in str(exc)
            raise
        finally:
            if sources_dir.exists():
                _secure_wipe(sources_dir)
            record["finished_at"] = time.time()
            if was_cancelled:
                record["state"] = "cancelled"
                self._write_record(directory, record)
        report.update(
            {
                "session_id": session_id,
                "file": record["source"]["path"],
                "label": record["source"].get("label", ""),
                "kind": record["source"].get("kind", ""),
                "sha256": record["source"]["sha256"],
                "size_bytes": record["source"]["size"],
                "platform": record["structure"]["platform"],
                "declared": record["structure"]["declared"],
                "sources_wiped": True,
                "scanned_at": time.time(),
            }
        )
        record["state"] = "completed"
        record["report"] = report
        self._write_record(directory, record)
        return report

    def cancel(self, session_id: str) -> dict[str, Any]:
        """Destroy the decompiled sources without checking them."""
        directory = self._session_dir(session_id)
        record = self._read_record(session_id)
        if record.get("state") != "awaiting":
            raise AuditError("Сессия уже завершена или отменена")
        sources_dir = directory / "sources"
        if sources_dir.exists():
            _secure_wipe(sources_dir)
        record["state"] = "cancelled"
        record["finished_at"] = time.time()
        self._write_record(directory, record)
        return {"session_id": session_id, "cancelled": True, "sources_wiped": True}

    def status(self, session_id: str) -> dict[str, Any]:
        record = self._read_record(session_id)
        payload = {
            "session_id": session_id,
            "state": record.get("state"),
            "source": record.get("source", {}),
            "structure": record.get("structure", {}),
            "estimate": record.get("estimate", {}),
            "sources_wiped": not (self._session_dir(session_id) / "sources").exists(),
        }
        if record.get("state") == "completed":
            payload["report"] = record.get("report")
        return payload

    @staticmethod
    def _structure_payload(structure: JarStructure) -> dict[str, Any]:
        return {
            "platform": structure.platform,
            "declared": structure.declared,
            "classes": structure.class_count,
            "strings": structure.string_count,
            "references": structure.reference_count,
            "natives": structure.natives[:64],
            "nested_jars": structure.nested_jars[:96],
            "skipped_entries": structure.skipped_entries[:64],
            "signed": structure.signed,
            "sources_bytes": structure.sources_bytes,
            "source_lines": structure.source_lines,
        }

    @staticmethod
    def _structure_from_record(record: dict[str, Any]) -> JarStructure:
        raw = record.get("structure") or {}
        declared = raw.get("declared") if isinstance(raw.get("declared"), dict) else {}
        return JarStructure(
            platform=str(raw.get("platform", "unknown")),
            declared=declared,
            class_count=int(raw.get("classes", 0) or 0),
            string_count=int(raw.get("strings", 0) or 0),
            reference_count=int(raw.get("references", 0) or 0),
            natives=[str(item) for item in (raw.get("natives") or [])][:64],
            nested_jars=[str(item) for item in (raw.get("nested_jars") or [])][:96],
            skipped_entries=[str(item) for item in (raw.get("skipped_entries") or [])][:64],
            signed=bool(raw.get("signed", False)),
            sources_bytes=int(raw.get("sources_bytes", 0) or 0),
            source_lines=int(raw.get("source_lines", 0) or 0),
        )
