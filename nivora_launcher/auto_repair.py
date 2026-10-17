"""Авто-починка экземпляра после краша или неудачного запуска.

``diagnose`` собирает улики из трёх источников — вывода игры, свежего отчёта
в ``crash-reports/`` и содержимого папки ``mods`` (манифесты jar-файлов) — и
возвращает список проблем. У каждой проблемы один или несколько способов
починки с понятными шагами: интерфейс показывает их пользователю и применяет
выбранный через ``apply_fixes``.

Все починки обратимы или безопасны: моды не удаляются, а получают суффикс
``.disabled``; сломанные конфиги переносятся в резервную папку; восстановление
версии не трогает mods/config/saves. Параметры починок приходят из
интерфейса, поэтому ``apply_fixes`` проверяет их заново, а не доверяет им.

Кроме модов и параметров JVM разбираются ещё три класса сбоев:

- модлоадер — повреждённая установка (переустановка той же версии) или
  слишком старая версия, которую требуют моды (обновление загрузчика);
- аккаунт — истёкший или потерянный токен, удалённый аккаунт, отсутствие
  лицензии, ошибки Xbox Live (повторный вход, смена аккаунта);
- безопасность — песочница экземпляра (bubblewrap/AppContainer) заблокировала
  сеть, файлы, процессы или нативный код. Предлагается ослабить профиль или
  выдать конкретное разрешение либо отключить мод, который упёрся в запрет.
"""
from __future__ import annotations

import json
import re
import shutil
import sys
import time
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .config import format_memory_kib, parse_memory_kib, recommended_memory_kib, system_memory_kib
from .errors import LauncherError
from .mod_dependencies import MAX_MODS, NOISE_IDS, read_mod_manifest
from .security import SECURITY_PERMISSIONS, SECURITY_PROFILES, UNENFORCED_CELL_PERMISSIONS

STATE_FILE = ".nivora-auto-repair.json"
BACKUP_DIRECTORY = ".nivora-repair-backups"
MAX_LOG_LINES = 1500
MAX_CRASH_REPORT_BYTES = 2 * 1024 * 1024
MAX_EVIDENCE = 4
MAX_FILES_PER_FIX = 40

# Порядок — порядок показа в настройках и в окне починки.
ISSUE_KINDS: dict[str, str] = {
    "account": "Проблема с аккаунтом",
    "security_sandbox": "Запуску мешают настройки безопасности",
    "loader_broken": "Модлоадер повреждён или не запускается",
    "loader_outdated": "Устаревшая версия модлоадера",
    "missing_dependencies": "Не хватает зависимостей модов",
    "incompatible_mods": "Несовместимые моды",
    "wrong_loader_mods": "Моды для другого загрузчика",
    "duplicate_mods": "Дубликаты модов",
    "mixin_failure": "Ошибка Mixin в моде",
    "corrupted_mods": "Повреждённые файлы модов",
    "broken_config": "Повреждённые конфиги модов",
    "out_of_memory": "Не хватило памяти",
    "memory_too_high": "Выделено слишком много памяти",
    "java_version": "Неподходящая версия Java",
    "jvm_arguments": "Ошибочные аргументы JVM",
    "jvm_crash": "Аварийное падение Java",
    "game_files": "Повреждены файлы игры или загрузчика",
    "graphics": "Проблема с графикой",
    "recent_mods": "Недавно добавленные моды",
    "unknown_crash": "Неизвестная причина краша",
}

FIX_ACTIONS = frozenset({
    "disable_mods",
    "install_dependencies",
    "set_memory",
    "set_java",
    "reset_jvm_arguments",
    "repair_version",
    "backup_configs",
    "set_graphics_mode",
    "update_loader",
    "set_account",
    "login_microsoft",
    "set_security",
})

# Идентификаторы самих загрузчиков в манифестах и сообщениях о зависимостях.
LOADER_DEPENDENCY_IDS: dict[str, frozenset[str]] = {
    "fabric": frozenset({"fabricloader", "fabric-loader", "fabric_loader"}),
    "quilt": frozenset({"quilt_loader", "quilt-loader", "quiltloader"}),
    "forge": frozenset({"forge", "fml", "javafml"}),
    "neoforge": frozenset({"neoforge", "fml", "javafml"}),
}
ALL_LOADER_DEPENDENCY_IDS = frozenset().union(*LOADER_DEPENDENCY_IDS.values())
LOADER_TITLES = {"fabric": "Fabric", "quilt": "Quilt", "forge": "Forge", "neoforge": "NeoForge", "vanilla": "Vanilla"}
# Разрешения «обычной игры»: их нехватка — настройка, а не подозрительный мод.
GAME_PERMISSIONS = ("network:game", "files:instance", "desktop:display", "desktop:audio")
SANDBOXED_PROFILES = frozenset({"strict", "manual"})
# Пакеты JDK, игры и загрузчиков: в стеке они не указывают на конкретный мод.
_FRAMEWORK_PACKAGES = (
    "java.", "javax.", "jdk.", "sun.", "com.sun.", "net.minecraft.", "com.mojang.", "net.fabricmc.",
    "org.quiltmc.", "cpw.mods.", "net.minecraftforge.", "net.neoforged.", "org.spongepowered.",
    "org.lwjgl.", "io.netty.", "com.google.", "org.apache.", "it.unimi.", "oshi.", "org.slf4j.",
    "org.objectweb.", "com.llamalad7.", "kotlin.", "scala.", "org.jline.", "joptsimple.", "com.electronwill.",
    "org.openjdk.", "net.jodah.", "com.ibm.", "org.yaml.", "org.joml.", "com.machinezoo.", "MC-BOOTSTRAP",
)

SHADER_MOD_IDS = frozenset({"iris", "oculus", "optifabric", "optifine", "canvas", "irisshaders"})
LIBRARY_DEPENDENCY_IDS = frozenset({"java", "minecraft"})
CONFIG_SUFFIXES = (".toml", ".json", ".json5", ".cfg", ".properties", ".yml", ".yaml", ".txt", ".snbt")

# Зависимость в сообщении Fabric: 'Name' (id) | 'id' | id
_DEPENDENCY = r"(?:'([^']+)' \(([\w.\-]+)\)|'([\w.\-]+)'|([\w.\-]+))"
_FABRIC_REQUIRES = re.compile(
    r"Mod '([^']+)' \(([\w.\-]+)\)[^\n]*? requires ([^\n]*?) of (?:mod )?" + _DEPENDENCY + r",\s*which is missing",
    re.IGNORECASE,
)
_FABRIC_WRONG_VERSION = re.compile(
    r"Mod '([^']+)' \(([\w.\-]+)\)[^\n]*? requires ([^\n]*?) of (?:mod )?" + _DEPENDENCY + r",\s*but only the wrong version is present",
    re.IGNORECASE,
)
_FABRIC_INCOMPATIBLE = re.compile(
    r"Mod '([^']+)' \(([\w.\-]+)\)[^\n]*?is incompatible with [^\n]*?(?:mod )?'([^']+)'(?: \(([\w.\-]+)\))?",
    re.IGNORECASE,
)
_FORGE_DEPENDENCY = re.compile(
    r"Mod ID: '([\w.\-]+)', Requested by: '([\w.\-]+)', Expected range: '([^']*)', Actual version: '([^']*)'",
    re.IGNORECASE,
)
_FORGE_DUPLICATE = re.compile(r"Mod ID: '([\w.\-]+)' from mod files: ([^\n]+)", re.IGNORECASE)
_MIXIN_FOR_MOD = re.compile(r"Mixin apply (?:for|of) mod ([\w.\-]+) failed", re.IGNORECASE)
_MIXIN_FROM_MOD = re.compile(r"from mod ([\w.\-]+)(?:\]| failed| ->)", re.IGNORECASE)
_MIXIN_CONFIG = re.compile(r"([\w.\-]+\.mixins?\.json)", re.IGNORECASE)
_MODULE_CONFLICT = re.compile(r"Modules? ([\w.\-]+) and ([\w.\-]+) export package", re.IGNORECASE)
_MODULE_READS = re.compile(r"reads package [\w.]+ from both ([\w.\-]+) and ([\w.\-]+)", re.IGNORECASE)
_CRASH_MOD_FILE = re.compile(r"Mod File: (?:[^\n]*[/\\])?([^\n/\\]+\.jar)", re.IGNORECASE)
_CRASH_SUSPECT_ID = re.compile(r"^\s+[^\n(]+\(([\w.\-]+)\),\s*Version", re.MULTILINE)
_CLASS_FILE_VERSION = re.compile(r"class file version (\d+)(?:\.\d+)?", re.IGNORECASE)
_REQUIRES_JAVA = re.compile(
    r"(?:requires (?:version )?(\d{1,2})(?: or later)? of (?:mod )?'?java'?|requires java (\d{1,2})|требуется Java (\d{1,2})|Java (\d{1,2}) or (?:newer|later) is required)",
    re.IGNORECASE,
)
_CONFIG_PATH = re.compile(r"config[/\\]([\w.\-/\\ ]+?\.(?:toml|json5?|cfg|properties|ya?ml|txt|snbt))", re.IGNORECASE)
_FORGE_CONFIG_FILE = re.compile(r"(?:Failed loading config file|Failed to load config|Error loading config|Configuration file) ([\w.\-]+\.(?:toml|json5?|cfg))", re.IGNORECASE)

_OOM = re.compile(r"java\.lang\.OutOfMemoryError|GC overhead limit exceeded|Java heap space", re.IGNORECASE)
_MEMORY_TOO_HIGH = re.compile(
    r"Could not reserve enough space for|Invalid maximum heap size|insufficient memory for the Java Runtime|Native memory allocation \(mmap\) failed|Cannot allocate memory",
    re.IGNORECASE,
)
_JVM_ARGUMENTS = re.compile(
    r"Unrecognized VM option|Unrecognized option:|Error occurred during initialization of VM|Invalid initial heap size|Conflicting collector combinations|Multiple garbage collectors selected|Improperly specified VM option|Could not create the Java Virtual Machine",
    re.IGNORECASE,
)
_JVM_CRASH = re.compile(
    r"A fatal error has been detected by the Java Runtime|SIGSEGV|SIGBUS|EXCEPTION_ACCESS_VIOLATION|hs_err_pid\d+",
    re.IGNORECASE,
)
_DRIVER_FRAME = re.compile(r"atio6axx|atioglxx|nvoglv|ig\d+icd|libnvidia|amdvlk|radeonsi|iris_dri|i965_dri|libGLX|opengl32", re.IGNORECASE)
_GRAPHICS = re.compile(
    r"no OpenGL context|could not create (?:the )?(?:GL|OpenGL)|GLFW error (?:65542|65543|65544|65545)|failed to create window|Pixel format not accelerated|WGL: The driver does not appear to support OpenGL|OpenGL[^\n]{0,40}not supported|GLX: Failed to create context|Failed to create OpenGL context",
    re.IGNORECASE,
)
_GAME_FILES = re.compile(
    r"Could not find or load main class|ClassNotFoundException: (?:net\.fabricmc\.loader|cpw\.mods|net\.minecraftforge\.(?:fml|bootstrap)|net\.neoforged\.(?:fml|bootstrap)|org\.quiltmc\.loader|net\.minecraft\.client\.main|net\.minecraft\.launchwrapper)"
    r"|NoClassDefFoundError: (?:org/lwjgl|net/fabricmc/loader|cpw/mods|net/minecraftforge/fml/loading|net/neoforged/fml)"
    r"|no lwjgl[\w]* in java\.library\.path|UnsatisfiedLinkError|Failed to locate library|checksum mismatch|SHA-?1 mismatch|hash mismatch|Failed to download"
    r"|(?:FileNotFound|NoSuchFile)Exception[^\n]*(?:libraries|versions|assets)[/\\]"
    r"|zip END header not found[^\n]*(?:libraries|versions)|Файл версии не найден|не найден JSON версии",
    re.IGNORECASE,
)
_LOADER_BROKEN = re.compile(
    r"Could not find or load main class (?:net\.fabricmc|org\.quiltmc|cpw\.mods|net\.minecraftforge|net\.neoforged)"
    r"|ClassNotFoundException: (?:net\.fabricmc\.loader|org\.quiltmc\.loader|cpw\.mods|net\.minecraftforge\.(?:fml|bootstrap)|net\.neoforged\.(?:fml|bootstrap|neoforgespi))"
    r"|NoClassDefFoundError: (?:net/fabricmc/loader|org/quiltmc/loader|cpw/mods|net/minecraftforge/fml|net/neoforged/fml)"
    r"|Failed to find system mod|Missing (?:minecraft|forge|neoforge|mcp) (?:jar|installation)|InvalidLauncherSetupException"
    r"|Failed to find (?:the )?Minecraft (?:client )?jar|Unable to locate (?:the )?minecraft|Could not find required file"
    r"|client-[\w.]+-(?:srg|extra|slim|official)\.jar|java\.lang\.module\.FindException"
    r"|(?:forge|neoforge|fabric|quilt) installer (?:завершился с кодом|превысил лимит|не создал version JSON|не запустился)"
    r"|There is no minecraft launcher profile|There was an error during installation|A problem installing was detected"
    r"|В экземпляре нет установленной версии|Не удалось прочитать metadata версии|отсутствует mainClass|Metadata версии слишком велика"
    r"|net\.fabricmc\.loader\.impl\.FormattedException: (?:java\.lang\.RuntimeException: )?(?:Failed to|Could not)"
    r"|Loader version [^\n]{0,40}is not compatible|Failed to load (?:FML|ModLauncher)",
    re.IGNORECASE,
)
# NeoForge (FML 4+): «Mod create requires neoforge 21.1.77 or above\nCurrently, neoforge is 21.1.50».
_FML_REQUIRES = re.compile(
    r"Mod ([\w.\-]+) requires ([\w.\-]+) ([\w.\-+]+) or above\s*(?:\n\s*)?Currently, ([\w.\-]+) is (not installed|[\w.\-+]+)",
    re.IGNORECASE,
)

# --- Аккаунты ----------------------------------------------------------------
_ACCOUNT_RELOGIN = re.compile(
    r"Microsoft-токен истёк|Повторите login microsoft|Токены Microsoft-аккаунта не найдены|Не удалось обновить Microsoft-токен"
    r"|invalid_grant|AADSTS\d+|Microsoft OAuth завершился ошибкой|Время ожидания входа Microsoft истекло"
    r"|InvalidCredentialsException|Invalid token|Invalid session|Failed to verify username|Bad login|User not premium"
    r"|(?:sessionserver|authserver|api\.minecraftservices)[^\n]{0,120}\b401\b",
    re.IGNORECASE,
)
_ACCOUNT_MISSING = re.compile(
    r"Аккаунт не найден: ?([\w.\-]*)|Microsoft-аккаунт не найден|Debug-аккаунт ещё не создан|Нужен Microsoft-аккаунт|Microsoft-аккаунт больше не доступен",
    re.IGNORECASE,
)
_ACCOUNT_NO_LICENSE = re.compile(
    r"не найдено право на Minecraft|does not own (?:Minecraft|the game)|Minecraft Services не вернул корректный id профиля"
    r"|Minecraft Services не выдал access token|Minecraft Services вернул некорректный profile|NOT_FOUND[^\n]{0,60}profile",
    re.IGNORECASE,
)
_ACCOUNT_XSTS = re.compile(r"Xbox XSTS отклонил аккаунт, код (\d+)", re.IGNORECASE)
_ACCOUNT_STORAGE = re.compile(
    r"Для Microsoft-токенов нужны libsecret|хранилище паролей недоступно|secret-tool[^\n]{0,80}(?:not found|не найден|failed)|CredWrite|CredRead",
    re.IGNORECASE,
)
_ACCOUNT_MULTIPLAYER = re.compile(r"Multiplayer is disabled|multiplayer (?:has been )?disabled|UserBannedException|banned from (?:online play|multiplayer)", re.IGNORECASE)
_XSTS_HINTS = {
    "2148916233": "У этого Microsoft-аккаунта нет профиля Xbox. Войдите на xbox.com, создайте профиль и повторите вход.",
    "2148916235": "Xbox Live недоступен в стране аккаунта. Смените регион аккаунта Microsoft или используйте другой аккаунт.",
    "2148916236": "Аккаунту нужна проверка возраста на xbox.com — пройдите её и повторите вход.",
    "2148916237": "Аккаунту нужна проверка возраста на xbox.com — пройдите её и повторите вход.",
    "2148916238": "Это детский аккаунт: взрослый член семьи Microsoft должен добавить его в семью и разрешить игру.",
}

# --- Песочница экземпляра ------------------------------------------------------
_SANDBOX_LAUNCHER = re.compile(
    r"Security profile (?:strict|manual) не может безопасно выдать разрешения:? ?([\w:, \-]*)"
    r"|strict profile не выдаёт модам запись в файлы компьютера|files:host-read на Windows не поддерживается в strict"
    r"|strict profile на Windows требует AppContainer|strict profile требует установленный bubblewrap|strict profile поддерживается только на"
    r"|Песочница Windows:[^\n]*|Java недоступна из песочницы|bwrap: [^\n]+|Не удалось (?:запустить процесс в AppContainer|создать Job Object|ограничить Job Object|назначить AppContainer|поместить процесс в Job Object)"
    r"|Security cell (?:является|содержит)",
    re.IGNORECASE,
)
_SANDBOX_NETWORK = re.compile(
    r"java\.net\.UnknownHostException|Network is unreachable|java\.net\.SocketException: (?:Permission denied|Operation not permitted)"
    r"|Temporary failure in name resolution|Name or service not known|An attempt was made to access a socket in a way forbidden"
    r"|No such host is known|Этот хост неизвестен",
    re.IGNORECASE,
)
_SANDBOX_FILES = re.compile(
    r"java\.nio\.file\.AccessDeniedException:? ?([^\n]*)|(?:FileNotFoundException|IOException)[^\n]*\((?:Access is denied|Permission denied|Отказано в доступе)\)"
    r"|Read-only file system|Отказано в доступе|Access is denied",
    re.IGNORECASE,
)
_SANDBOX_PROCESS = re.compile(
    r"Cannot run program \"([^\"]+)\"[^\n]*(?:error=\d+|denied|not permitted)|CreateProcess error=(?:5|1450|1816)\b|java\.io\.IOException: error=(?:1|13),",
    re.IGNORECASE,
)
_SANDBOX_NATIVE = re.compile(
    r"Unable to load library '([^']+)'|UnsatisfiedLinkError[^\n]*(?:Access is denied|Permission denied|failed to map segment|Operation not permitted)",
    re.IGNORECASE,
)
_SANDBOX_DISPLAY = re.compile(
    r"DISPLAY environment variable is missing|Failed to open (?:X )?display|No X11 DISPLAY|Can't connect to X11|Wayland: Failed to connect to display"
    r"|/dev/(?:dri|nvidia)[^\n]{0,80}(?:Permission denied|Operation not permitted)",
    re.IGNORECASE,
)
_STACK_FRAME = re.compile(r"^\s*at (?:[\w.\-]+(?:@[\w.\-]+)?/)?([\w$]+(?:\.[\w$]+)+)\.[\w$<>]+\(", re.MULTILINE)
_MIXIN_HANDLER = re.compile(r"\$zz[a-z]\d{3}\$([a-z][a-z0-9_]{1,63})\$", re.IGNORECASE)

_CORRUPTED_ZIP = re.compile(r"zip END header not found|invalid LOC header|error in opening zip file|ZipException|Invalid or corrupt jarfile|BadZipFile", re.IGNORECASE)
_JAVA_MISSING = re.compile(r"Java не найден|Java executable не найден|Java должна быть исполняемым|Cannot run program \"[^\"]*java", re.IGNORECASE)


@dataclass
class ModEntry:
    file: str
    mod_id: str
    name: str
    enabled: bool
    loaders: frozenset[str]
    dependencies: list[dict[str, Any]]
    mixin_configs: frozenset[str]
    mtime: float
    corrupted: bool = False
    packages: frozenset[str] = frozenset()


def read_state(instance_path: Path) -> dict[str, Any]:
    path = instance_path / STATE_FILE
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 64 * 1024:
            return {}
        value = json.loads(path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError, TypeError):
        return {}


def record_launch_result(instance_path: Path | None, exit_code: int | None) -> None:
    """Запоминает время последнего успешного запуска: по нему ищутся «новые» моды."""
    if instance_path is None or exit_code != 0:
        return
    path = instance_path / STATE_FILE
    if path.is_symlink():
        return
    state = read_state(instance_path)
    state["last_success"] = time.time()
    try:
        path.write_text(json.dumps(state), encoding="utf-8")
    except OSError:
        pass


def scan_mods(instance_path: Path | None) -> list[ModEntry]:
    if instance_path is None:
        return []
    directory = instance_path / "mods"
    if directory.is_symlink() or not directory.is_dir():
        return []
    result: list[ModEntry] = []
    try:
        entries = sorted(directory.iterdir(), key=lambda item: item.name.lower())
    except OSError:
        return []
    for path in entries:
        if len(result) >= MAX_MODS:
            break
        lowered = path.name.lower()
        enabled = not lowered.endswith(".disabled")
        base = lowered.removesuffix(".disabled")
        if not base.endswith((".jar", ".zip")) or path.is_symlink() or not path.is_file():
            continue
        try:
            mtime = path.stat().st_mtime
        except OSError:
            continue
        loaders: set[str] = set()
        mixins: set[str] = set()
        packages: set[str] = set()
        corrupted = False
        try:
            with zipfile.ZipFile(path) as archive:
                for name in archive.namelist():
                    if name.endswith(".class") and not name.startswith("META-INF/") and len(packages) < 64:
                        # Пакеты классов мода: по ним стек ошибки сводится к jar-файлу.
                        parts = name.split("/")[:-1]
                        if len(parts) >= 2:
                            packages.add("/".join(parts[:3]))
                    if name in {"fabric.mod.json"}:
                        loaders.add("fabric")
                    elif name == "quilt.mod.json":
                        loaders.add("quilt")
                    elif name == "META-INF/neoforge.mods.toml":
                        loaders.add("neoforge")
                    elif name in {"META-INF/mods.toml", "mcmod.info"}:
                        loaders.add("forge")
                    if "/" not in name and name.lower().endswith(".json") and "mixin" in name.lower():
                        mixins.add(name.lower())
        except (OSError, ValueError, RuntimeError, zipfile.BadZipFile):
            corrupted = True
        manifest = None if corrupted else _manifest(path)
        mod_id = str((manifest or {}).get("mod_id") or "").strip().lower()
        result.append(
            ModEntry(
                file=path.name,
                mod_id=mod_id,
                name=str((manifest or {}).get("name") or path.stem),
                enabled=enabled,
                loaders=frozenset(loaders),
                dependencies=list((manifest or {}).get("dependencies") or []),
                mixin_configs=frozenset(mixins),
                mtime=mtime,
                corrupted=corrupted,
                packages=frozenset(packages),
            )
        )
    return result


def _manifest(path: Path) -> dict[str, Any] | None:
    # Выключенные моды в анализе зависимостей не участвуют.
    if path.name.lower().endswith(".disabled"):
        return None
    return read_mod_manifest(path)


def latest_crash_report(instance_path: Path | None, since: float = 0.0) -> str:
    if instance_path is None:
        return ""
    directory = instance_path / "crash-reports"
    if directory.is_symlink() or not directory.is_dir():
        return ""
    try:
        candidates = [
            path for path in directory.iterdir()
            if path.is_file() and not path.is_symlink() and path.suffix.lower() == ".txt" and path.stat().st_mtime >= since
        ]
    except OSError:
        return ""
    if not candidates:
        return ""
    newest = max(candidates, key=lambda path: path.stat().st_mtime)
    try:
        with newest.open("rb") as handle:
            return handle.read(MAX_CRASH_REPORT_BYTES).decode("utf-8", errors="replace")
    except OSError:
        return ""


def diagnose(
    lines: list[str],
    exit_code: int | None,
    *,
    instance_path: Path | None,
    instance_settings: dict[str, Any] | None = None,
    effective_memory: str = "",
    loader: str = "",
    minecraft_version: str = "",
    error: str = "",
    since: float = 0.0,
    muted: list[str] | tuple[str, ...] = (),
    crash_report: str | None = None,
    loader_version: str = "",
    accounts: list[dict[str, Any]] | None = None,
    current_account_id: str = "",
) -> dict[str, Any]:
    settings = instance_settings or {}
    loader = (loader or "vanilla").strip().lower()
    loader_title = LOADER_TITLES.get(loader, loader.capitalize())
    loader_ids = LOADER_DEPENDENCY_IDS.get(loader, frozenset())
    log_lines = [str(line) for line in lines[-MAX_LOG_LINES:]]
    if error:
        log_lines.append(str(error))
    report = latest_crash_report(instance_path, since) if crash_report is None else crash_report
    text = "\n".join(log_lines)
    combined = f"{text}\n{report}" if report else text
    mods = scan_mods(instance_path) if loader != "vanilla" else []
    enabled_mods = [mod for mod in mods if mod.enabled]
    by_id = {mod.mod_id: mod for mod in enabled_mods if mod.mod_id}
    issues: dict[str, dict[str, Any]] = {}

    def add(
        kind: str,
        detail: str,
        fixes: list[dict[str, Any] | None],
        evidence: list[str] | None = None,
        title: str = "",
        manual: str = "",
    ) -> None:
        fixes = [fix for fix in fixes if fix]
        if not fixes and not manual:
            return
        if kind in issues:
            # Повторная улика той же природы — дополняем, а не дублируем.
            existing = issues[kind]
            existing["evidence"] = _unique([*existing["evidence"], *(evidence or [])])[:MAX_EVIDENCE]
            if kind in {"account", "security_sandbox"} and detail and detail not in existing["detail"]:
                # У этих проблем причины независимы — показываем все.
                existing["detail"] = f"{existing['detail']} {detail}"
                if manual and manual not in existing["manual"]:
                    existing["manual"] = f"{existing['manual']} {manual}".strip()
            for fix in fixes:
                current = next((item for item in existing["fixes"] if item["action"] == fix["action"] and item.get("params") == fix.get("params")), None)
                if current is None and fix["action"] in {"disable_mods", "install_dependencies"}:
                    current = next((item for item in existing["fixes"] if item["action"] == fix["action"]), None)
                if current is None:
                    fix["id"] = f"{kind}:{fix['action']}:{len(existing['fixes']) + 1}"
                    fix["recommended"] = False
                    existing["fixes"].append(fix)
                elif fix["action"] == "disable_mods":
                    current["params"]["files"] = _unique([*current["params"]["files"], *fix["params"]["files"]])[:MAX_FILES_PER_FIX]
                    current["steps"] = _disable_steps(current["params"]["files"])
                elif fix["action"] == "install_dependencies":
                    current["params"]["ids"] = _unique([*current["params"]["ids"], *fix["params"]["ids"]])[:MAX_FILES_PER_FIX]
                    current["steps"] = _install_steps(current["params"]["ids"])
            return
        used_ids: set[str] = set()
        for index, fix in enumerate(fixes):
            # У одной проблемы может быть несколько починок одного типа
            # (например, «переключиться на аккаунт A / B») — id обязан быть уникальным.
            fix_id = f"{kind}:{fix['action']}"
            suffix = 2
            while fix_id in used_ids:
                fix_id = f"{kind}:{fix['action']}:{suffix}"
                suffix += 1
            used_ids.add(fix_id)
            fix["id"] = fix_id
            fix["recommended"] = index == 0
        issues[kind] = {
            "id": kind,
            "title": title or ISSUE_KINDS[kind],
            "detail": detail,
            "evidence": _unique(evidence or [])[:MAX_EVIDENCE],
            "fixes": fixes,
            "manual": manual,
        }

    # --- Зависимости и совместимость модов ---------------------------------
    missing: dict[str, set[str]] = {}
    requesters_wrong_version: list[tuple[str, str, str]] = []
    wrong_game_version: list[str] = []
    java_required = 0
    # Моды, которым нужен более новый загрузчик: (requester, минимальная версия, строка лога).
    loader_requirements: list[tuple[str, str, str]] = []
    for match in _FABRIC_REQUIRES.finditer(combined):
        requester, dependency = match.group(2).lower(), _dependency_id(match)
        if dependency == "java" or dependency in ALL_LOADER_DEPENDENCY_IDS:
            # Чужой загрузчик (quilt_loader на Fabric) ловит проверка wrong_loader_mods.
            continue
        if dependency == "minecraft":
            wrong_game_version.append(requester)
            continue
        missing.setdefault(dependency, set()).add(requester)
    for match in _FABRIC_WRONG_VERSION.finditer(combined):
        requester, dependency = match.group(2).lower(), _dependency_id(match)
        if dependency == "java":
            java_required = max(java_required, _first_int(match.group(3)))
        elif dependency == "minecraft":
            wrong_game_version.append(requester)
        elif dependency in loader_ids:
            loader_requirements.append((requester, _first_version(match.group(3)), match.group(0).strip()))
        else:
            requesters_wrong_version.append((requester, dependency, match.group(0).strip()))
    for match in _FORGE_DEPENDENCY.finditer(combined):
        dependency, requester, actual = match.group(1).lower(), match.group(2).lower(), match.group(4)
        if dependency == "java":
            java_required = max(java_required, _first_int(match.group(3)))
        elif dependency == "minecraft":
            wrong_game_version.append(requester)
        elif dependency in loader_ids:
            loader_requirements.append((requester, _first_version(match.group(3)), match.group(0).strip()))
        elif "missing" in actual.lower() or not actual.strip():
            missing.setdefault(dependency, set()).add(requester)
        else:
            requesters_wrong_version.append((requester, dependency, match.group(0).strip()))
    for match in _FML_REQUIRES.finditer(combined):
        requester, dependency, minimum, state = match.group(1).lower(), match.group(2).lower(), match.group(3), match.group(5)
        if dependency == "minecraft":
            wrong_game_version.append(requester)
        elif dependency in loader_ids:
            loader_requirements.append((requester, _first_version(minimum), match.group(0).strip()))
        elif dependency == "java" or dependency in ALL_LOADER_DEPENDENCY_IDS:
            continue
        elif state.lower() == "not installed":
            missing.setdefault(dependency, set()).add(requester)
        else:
            requesters_wrong_version.append((requester, dependency, match.group(0).strip()))
    # Статический анализ: обязательные зависимости, которых нет в папке mods.
    installed_ids = {mod.mod_id for mod in enabled_mods if mod.mod_id}
    provided = _provided_ids(enabled_mods)
    for mod in enabled_mods:
        for dependency in mod.dependencies:
            dependency_id = str(dependency.get("id", "")).strip().lower()
            if (
                not dependency_id
                or dependency.get("optional")
                or dependency_id in NOISE_IDS
                or dependency_id in LIBRARY_DEPENDENCY_IDS
                or dependency_id in ALL_LOADER_DEPENDENCY_IDS
                or dependency_id in installed_ids
                or dependency_id in provided
                # подмодули Fabric/Quilt API лежат jar-in-jar внутри самого API
                or (dependency_id.startswith(("fabric-", "fabric_")) and dependency_id != "fabric-api")
                or dependency_id.startswith(("quilt_", "quilted_", "forge", "neoforge", "java"))
            ):
                continue
            # статикой ловим только то, что лог тоже назвал, или когда лога нет вовсе
            if dependency_id in missing or not combined.strip():
                missing.setdefault(dependency_id, set()).add(mod.mod_id)
    if missing:
        names = sorted(missing)
        requesters = sorted({requester for values in missing.values() for requester in values})
        requester_files = _files_for_ids(requesters, by_id)
        add(
            "missing_dependencies",
            "Модам не хватает обязательных библиотек: " + ", ".join(names[:8])
            + (f" (нужны для: {', '.join(requesters[:6])})" if requesters else "") + ".",
            [
                _install_fix(names),
                _disable_fix(requester_files, "Отключить моды, которым не хватает зависимостей"),
            ],
            _evidence(combined, [*names, "missing"]),
        )
    if loader_requirements and loader in LOADER_DEPENDENCY_IDS:
        minimum = max((value for _, value, _ in loader_requirements if value), key=_version_key, default="")
        requesters = _unique([requester for requester, _, _ in loader_requirements])
        add(
            "loader_outdated",
            f"Модам нужен {loader_title} {minimum + ' или новее' if minimum else 'новее'}"
            + (f", а установлен {loader_version}" if loader_version else "")
            + f" (требуют: {', '.join(requesters[:6])}).",
            [
                _update_loader_fix(loader, minimum, loader_version, minecraft_version),
                _disable_fix(_files_for_ids(requesters, by_id), f"Отключить моды, которым нужен новый {loader_title}"),
            ],
            [line for _, _, line in loader_requirements],
        )
    if requesters_wrong_version:
        files = _files_for_ids([requester for requester, _, _ in requesters_wrong_version], by_id)
        add(
            "incompatible_mods",
            "Моды требуют другую версию зависимости: "
            + "; ".join(f"{requester} → {dependency}" for requester, dependency, _ in requesters_wrong_version[:5]) + ".",
            [_disable_fix(files, "Отключить моды с неподходящими версиями зависимостей")],
            [line for _, _, line in requesters_wrong_version],
        )
    if wrong_game_version:
        files = _files_for_ids(wrong_game_version, by_id)
        add(
            "incompatible_mods",
            f"Часть модов собрана для другой версии Minecraft{f' (у экземпляра {minecraft_version})' if minecraft_version else ''}.",
            [_disable_fix(files, "Отключить моды для другой версии Minecraft")],
            _evidence(combined, ["minecraft"]),
        )
    incompatible_files: list[str] = []
    incompatible_lines: list[str] = []
    for match in _FABRIC_INCOMPATIBLE.finditer(combined):
        incompatible_files.extend(_files_for_ids([match.group(2).lower()], by_id))
        incompatible_lines.append(match.group(0).strip())
    for pattern in (_MODULE_CONFLICT, _MODULE_READS):
        for match in pattern.finditer(combined):
            for module in match.groups():
                incompatible_files.extend(_files_for_names([module], enabled_mods))
            incompatible_lines.append(match.group(0).strip())
    if incompatible_lines:
        add(
            "incompatible_mods",
            "Загрузчик сообщил о конфликте модов между собой.",
            [_disable_fix(_unique(incompatible_files), "Отключить конфликтующие моды")],
            incompatible_lines,
        )
    if re.search(r"some of your mods are incompatible|incompatible mod set|Mod resolution failed|ModResolutionException", combined, re.IGNORECASE) and "incompatible_mods" not in issues and "missing_dependencies" not in issues:
        suspects = _crash_report_suspects(report, enabled_mods)
        add(
            "incompatible_mods",
            "Загрузчик не смог собрать набор модов, но не назвал конкретного виновника.",
            [_disable_fix(suspects, "Отключить подозреваемые моды"), _recent_fix(enabled_mods, instance_path)],
            _evidence(combined, ["incompatible", "resolution"]),
        )

    # --- Моды для другого загрузчика -------------------------------------------
    wrong_loader = [mod.file for mod in enabled_mods if _wrong_loader(mod, loader, by_id)]
    if wrong_loader:
        add(
            "wrong_loader_mods",
            f"В папке mods есть моды, рассчитанные не на {loader.capitalize()}: они не загрузятся или уронят игру.",
            [_disable_fix(wrong_loader, f"Отключить моды не для {loader.capitalize()}")],
            wrong_loader[:MAX_EVIDENCE],
        )

    # --- Дубликаты ---------------------------------------------------------------
    duplicates: list[str] = []
    seen_ids: dict[str, list[ModEntry]] = {}
    for mod in enabled_mods:
        if mod.mod_id:
            seen_ids.setdefault(mod.mod_id, []).append(mod)
    duplicate_lines: list[str] = []
    for mod_id, group in seen_ids.items():
        if len(group) > 1:
            # оставляем самый свежий файл, остальные выключаем
            group = sorted(group, key=lambda item: item.mtime, reverse=True)
            duplicates.extend(item.file for item in group[1:])
            duplicate_lines.append(f"{mod_id}: {', '.join(item.file for item in group)}")
    for match in _FORGE_DUPLICATE.finditer(combined):
        duplicate_lines.append(match.group(0).strip())
    if duplicates:
        add(
            "duplicate_mods",
            "Один и тот же мод лежит в папке mods несколько раз (разные версии).",
            [_disable_fix(duplicates, "Оставить самую новую копию, остальные отключить")],
            duplicate_lines,
        )

    # --- Mixin -------------------------------------------------------------------
    mixin_mods: list[str] = []
    for pattern in (_MIXIN_FOR_MOD, _MIXIN_FROM_MOD):
        mixin_mods.extend(match.group(1).lower() for match in pattern.finditer(combined))
    mixin_files = _files_for_ids(mixin_mods, by_id)
    if re.search(r"mixin", combined, re.IGNORECASE) and re.search(r"MixinApplyError|MixinTransformerError|InvalidInjectionException|InjectionError|Mixin apply|Critical injection failure|MixinPrepareError", combined, re.IGNORECASE):
        configs = {match.group(1).lower() for match in _MIXIN_CONFIG.finditer(combined)}
        mixin_files.extend(mod.file for mod in enabled_mods if configs & mod.mixin_configs)
        mixin_files = _unique(mixin_files)
        add(
            "mixin_failure",
            "Мод не смог внедрить свой код в игру (Mixin). Обычно это мод для другой версии игры или конфликт с другим модом."
            + (f" Виновник: {', '.join(mixin_files[:4])}." if mixin_files else ""),
            [_disable_fix(mixin_files, "Отключить мод с ошибкой Mixin"), _recent_fix(enabled_mods, instance_path)],
            _evidence(combined, ["mixin"]),
        )

    # --- Повреждённые jar --------------------------------------------------------
    corrupted = [mod.file for mod in enabled_mods if mod.corrupted]
    if corrupted:
        add(
            "corrupted_mods",
            "Часть jar-файлов в папке mods не открывается как архив — вероятно, загрузка прервалась.",
            [_disable_fix(corrupted, "Отключить повреждённые файлы (потом скачайте их заново)")],
            [*corrupted[:2], *_evidence(combined, ["zip"])],
        )

    # --- Конфиги -------------------------------------------------------------
    config_files = _broken_configs(combined, instance_path)
    if config_files and re.search(r"ParsingException|JsonSyntaxException|MalformedJsonException|Failed (?:loading|to load) config|Error loading config|Config(?:uration)? (?:file )?(?:is )?(?:corrupt|invalid|not correct)|TomlParsingException|ParseException", combined, re.IGNORECASE):
        add(
            "broken_config",
            "Мод не смог прочитать свой конфиг. После переноса файла мод создаст настройки по умолчанию.",
            [_config_fix(config_files)],
            _evidence(combined, ["config"]),
        )

    # --- Память ---------------------------------------------------------------
    total_kib = system_memory_kib()
    try:
        current_kib = parse_memory_kib(effective_memory or str(settings.get("memory", "auto")))
    except LauncherError:
        current_kib = recommended_memory_kib()
    if _MEMORY_TOO_HIGH.search(combined):
        target = max(1024 * 1024, min(recommended_memory_kib(), current_kib // 2))
        target = (target // (512 * 1024)) * 512 * 1024 or 1024 * 1024
        if target < current_kib:
            add(
                "memory_too_high",
                f"Java не смогла зарезервировать {format_memory_kib(current_kib)} — системе не хватает свободной памяти.",
                [_memory_fix(current_kib, target, "Уменьшить память экземпляра")],
                _evidence(combined, ["memory", "heap", "reserve"]),
            )
    elif _OOM.search(combined):
        ceiling = int(total_kib * 0.75) if total_kib else 8 * 1024 * 1024
        target = min(ceiling, max(current_kib + 2 * 1024 * 1024, current_kib * 3 // 2, 4 * 1024 * 1024))
        target = (target // (512 * 1024)) * 512 * 1024
        if target > current_kib:
            add(
                "out_of_memory",
                f"Игре не хватило выделенной памяти ({format_memory_kib(current_kib)}). Тяжёлым сборкам обычно нужно 4–8 ГБ.",
                [_memory_fix(current_kib, target, "Увеличить память экземпляра")],
                _evidence(combined, ["OutOfMemory", "heap"]),
            )

    # --- Java ---------------------------------------------------------------------
    for match in _CLASS_FILE_VERSION.finditer(combined):
        if "UnsupportedClassVersion" in combined or "more recent version of the Java" in combined:
            java_required = max(java_required, int(match.group(1)) - 44)
    for match in _REQUIRES_JAVA.finditer(combined):
        java_required = max(java_required, _first_int("".join(group or "" for group in match.groups())))
    java_path = str(settings.get("java_path", "")).strip()
    if java_required >= 8 or "UnsupportedClassVersionError" in combined:
        add(
            "java_version",
            f"Игре или модам нужна Java {java_required or 'новее'}, а запуск шёл на более старой.",
            _java_fixes(java_required, java_path),
            _evidence(combined, ["java", "class file version"]),
            manual=f"Если подходящей Java нет, установите Java {java_required or 'новее'} (например, Eclipse Temurin) и выберите её в настройках экземпляра.",
        )
    elif _JAVA_MISSING.search(combined):
        add(
            "java_version",
            "Лаунчер не нашёл подходящую Java для запуска.",
            _java_fixes(0, java_path),
            _evidence(combined, ["java"]),
            title="Java не найдена",
        )

    # --- JVM -------------------------------------------------------------------
    custom_jvm = bool(settings.get("jvm_arguments")) or str(settings.get("jvm_preset", "none") or "none") != "none"
    if _JVM_ARGUMENTS.search(combined) and "memory_too_high" not in issues:
        fixes = [_jvm_reset_fix(settings)] if custom_jvm else []
        if not custom_jvm and re.search(r"heap size", combined, re.IGNORECASE):
            fixes.append(_memory_fix(current_kib, recommended_memory_kib(), "Сбросить память на рекомендуемую"))
        add(
            "jvm_arguments",
            "Java отказалась стартовать из-за параметров запуска (аргументы JVM или пресет GC).",
            fixes,
            _evidence(combined, ["VM option", "Virtual Machine", "option"]),
        )
    if _JVM_CRASH.search(combined):
        driver_crash = bool(_DRIVER_FRAME.search(combined))
        fixes = []
        if driver_crash:
            fixes.extend(_graphics_fixes(settings, enabled_mods))
        if custom_jvm:
            fixes.append(_jvm_reset_fix(settings))
        if java_path:
            # Для краша JVM другую версию наугад не подбираем: старым версиям игры
            # нужна именно их Java. Автовыбор возьмёт ту, что требует версия.
            fixes.extend(fix for fix in _java_fixes(0, java_path) if not fix["params"]["java_path"])
        add(
            "jvm_crash",
            "Упала сама Java, а не Minecraft"
            + (" — сбой внутри графического драйвера." if driver_crash else ". Частые причины: кастомные аргументы JVM, сбойная сборка Java или драйвер."),
            fixes,
            _evidence(combined, ["fatal error", "SIGSEGV", "ACCESS_VIOLATION", "hs_err"]),
            manual="Если краш повторяется, обновите драйвер видеокарты и Java." if driver_crash else "",
        )

    # --- Модлоадер -------------------------------------------------------------
    if loader in LOADER_DEPENDENCY_IDS and _LOADER_BROKEN.search(combined):
        add(
            "loader_broken",
            f"{loader_title} не смог стартовать: файлы загрузчика отсутствуют, повреждены или установлены не до конца"
            + (f" (версия {loader_version})." if loader_version else "."),
            [
                _repair_fix(loader),
                _update_loader_fix(loader, "", loader_version, minecraft_version, reinstall=True),
            ],
            _evidence(combined, ["main class", "ClassNotFound", "NoClassDefFound", "installer", "jar", "mainClass", "FindException", "system mod", "metadata"]),
            manual=(
                "В пути к игре есть не-латинские символы. Если переустановка не поможет, обновите лаунчер "
                "или перенесите папку игры в путь из латинских букв (например, C:\\Games\\Nivora)."
                if re.search(r"[A-Za-z]:\\[^\n]*[^\x00-\x7f]", combined) else ""
            ),
        )

    # --- Аккаунт ----------------------------------------------------------------
    _diagnose_account(add, combined, accounts or [], current_account_id)

    # --- Безопасность -----------------------------------------------------------
    _diagnose_sandbox(add, combined, report, settings, enabled_mods, instance_path)

    # --- Файлы игры и загрузчика -----------------------------------------------
    game_files_hit = _GAME_FILES.search(combined)
    zip_in_libraries = _CORRUPTED_ZIP.search(combined) and re.search(r"(?:libraries|versions)[/\\]", combined)
    if (game_files_hit or zip_in_libraries) and "loader_broken" not in issues:
        add(
            "game_files",
            "Не хватает или повреждены файлы версии, библиотек или модлоадера.",
            [_repair_fix(loader)],
            _evidence(combined, ["ClassNotFound", "main class", "lwjgl", "libraries", "download", "mismatch"]),
        )

    # --- Графика ---------------------------------------------------------------------
    if _GRAPHICS.search(combined):
        add(
            "graphics",
            "Minecraft не смог создать окно с OpenGL. Обычно помогает обновить драйвер видеокарты; лаунчер может попробовать обходные пути.",
            _graphics_fixes(settings, enabled_mods),
            _evidence(combined, ["OpenGL", "GLFW", "GL", "window"]),
            manual="Установите свежий драйвер видеокарты с сайта NVIDIA/AMD/Intel; на ноутбуке убедитесь, что игра запускается на дискретной видеокарте.",
        )

    # --- Запасные варианты --------------------------------------------------------
    if not issues and (exit_code not in (0, None) or error):
        suspects = _crash_report_suspects(report, enabled_mods)
        if suspects:
            add(
                "incompatible_mods",
                "Отчёт о краше указывает на конкретные моды.",
                [_disable_fix(suspects, "Отключить моды из отчёта о краше")],
                _evidence(report, ["Suspected", "Mod File"]),
                title="Краш вызвал мод",
            )
        recent = _recent_fix(enabled_mods, instance_path)
        if recent:
            add(
                "recent_mods",
                "Явной причины в логе нет, но после последнего успешного запуска появились новые моды — частый источник крашей.",
                [recent],
                recent["params"]["files"][:MAX_EVIDENCE],
            )
        if not issues:
            add(
                "unknown_crash",
                "Явных признаков в логе не нашлось. Можно проверить и восстановить файлы версии и загрузчика — моды, миры и настройки не затрагиваются.",
                [_repair_fix(loader)],
                _tail_evidence(log_lines),
            )

    muted_set = {str(item) for item in muted}
    ordered = [issues[kind] for kind in ISSUE_KINDS if kind in issues]
    visible = [issue for issue in ordered if issue["id"] not in muted_set]
    return {
        "exit_code": exit_code,
        "issues": visible,
        "muted_issues": [issue["id"] for issue in ordered if issue["id"] in muted_set],
    }


# --- Аккаунт и песочница ------------------------------------------------------------


def _account_label(account: dict[str, Any]) -> str:
    name = str(account.get("alias") or account.get("username") or account.get("account_id") or "аккаунт")
    kind = "Microsoft" if account.get("kind") == "microsoft" else "офлайн"
    return f"{name} ({kind})"


def _set_account_fix(account: dict[str, Any], reason: str = "") -> dict[str, Any]:
    return {
        "action": "set_account",
        "label": f"Запускать от аккаунта {_account_label(account)}",
        "steps": [
            f"Аккаунт по умолчанию → {_account_label(account)}",
            *( [reason] if reason else [] ),
            "Вернуть прежний можно на вкладке «Аккаунты».",
        ],
        "params": {"account_id": str(account.get("account_id", ""))},
    }


def _login_fix(label: str = "Войти в Microsoft заново") -> dict[str, Any]:
    return {
        "action": "login_microsoft",
        "label": label,
        "steps": [
            "Откроется вход Microsoft по коду устройства — пароль вводится только на сайте Microsoft.",
            "Новые токены сохранятся в системном хранилище и заменят устаревшие.",
        ],
        "params": {},
    }


def _diagnose_account(
    add: Callable[..., None],
    combined: str,
    accounts: list[dict[str, Any]],
    current_account_id: str,
) -> None:
    known = [item for item in accounts if isinstance(item, dict) and item.get("account_id")]
    current = next((item for item in known if str(item.get("account_id")) == current_account_id), None)
    if current is None and not current_account_id:
        current = next((item for item in known if item.get("is_default")), None)
    current_id = str((current or {}).get("account_id", ""))
    others = [item for item in known if str(item.get("account_id")) != current_id]
    microsoft_others = [item for item in others if item.get("kind") == "microsoft"]

    xsts = _ACCOUNT_XSTS.search(combined)
    if xsts:
        code = xsts.group(1)
        add(
            "account",
            f"Xbox Live отклонил вход (код {code}).",
            [*(_set_account_fix(item) for item in microsoft_others[:2]), _login_fix("Войти другим аккаунтом Microsoft")],
            _evidence(combined, ["XSTS"]),
            title="Xbox Live не пустил аккаунт",
            manual=_XSTS_HINTS.get(code, "Проверьте аккаунт на xbox.com и повторите вход."),
        )
    if _ACCOUNT_NO_LICENSE.search(combined):
        add(
            "account",
            "У аккаунта Microsoft не найдена лицензия Minecraft: Java Edition.",
            [*(_set_account_fix(item) for item in microsoft_others[:2]), _login_fix("Войти аккаунтом, на котором куплена игра")],
            _evidence(combined, ["право", "own", "profile", "Minecraft Services"]),
            title="Нет лицензии Minecraft",
            manual="Игру нужно купить на minecraft.net тем же аккаунтом Microsoft. Game Pass даёт лицензию, только пока подписка активна.",
        )
    missing = _ACCOUNT_MISSING.search(combined)
    if missing:
        fixes: list[dict[str, Any] | None] = []
        fallback = next((item for item in others if item.get("kind") == "microsoft"), None) or (others[0] if others else None)
        if fallback:
            fixes.append(_set_account_fix(fallback, "Выбранный аккаунт удалён или недоступен — экземпляр получит рабочий."))
        fixes.append(_login_fix("Войти через Microsoft"))
        add(
            "account",
            "Аккаунт, выбранный для запуска, не найден"
            + (f" ({missing.group(1)})" if missing.groups() and missing.group(1) else "")
            + ": его удалили или он ещё не создан.",
            fixes,
            _evidence(combined, ["Аккаунт", "аккаунт"]),
            title="Аккаунт для запуска не найден",
        )
    storage = _ACCOUNT_STORAGE.search(combined)
    if _ACCOUNT_RELOGIN.search(combined) or storage:
        offline_on_server = bool(current and current.get("kind") == "offline" and re.search(r"Failed to verify username|Invalid session", combined, re.IGNORECASE))
        fixes = []
        if offline_on_server:
            fixes.extend(_set_account_fix(item, "Онлайн-серверы пускают только лицензионные аккаунты.") for item in microsoft_others[:2])
            fixes.append(_login_fix("Войти через Microsoft"))
        else:
            fixes.append(_login_fix())
            fixes.extend(_set_account_fix(item) for item in others[:2])
        manual = ""
        if storage:
            manual = (
                "Токены хранятся в системном хранилище паролей (Credential Manager / gnome-keyring / KWallet). "
                "Если оно не запущено или заблокировано, вход придётся повторять — проверьте, что хранилище работает."
            )
        elif offline_on_server:
            manual = "Офлайн-аккаунт подходит для одиночной игры и серверов с online-mode=false."
        add(
            "account",
            "Сервер отклонил офлайн-аккаунт: для этого сервера нужна лицензия." if offline_on_server
            else "Сессия Microsoft устарела или токены потеряны — Minecraft не может подтвердить аккаунт.",
            fixes,
            _evidence(combined, ["token", "токен", "session", "verify", "login", "401", "хранилищ", "secret"]),
            title="Офлайн-аккаунт не пускают на сервер" if offline_on_server else "Нужно войти в аккаунт заново",
            manual=manual,
        )
    if _ACCOUNT_MULTIPLAYER.search(combined):
        add(
            "account",
            "Сетевая игра для аккаунта отключена.",
            [*(_set_account_fix(item) for item in microsoft_others[:2])],
            _evidence(combined, ["multiplayer", "banned"]),
            title="Сетевая игра запрещена для аккаунта",
            manual="Разрешите сетевую игру в настройках конфиденциальности Xbox (account.xbox.com → Конфиденциальность) или в настройках семьи Microsoft.",
        )


def _security_fix(label: str, steps: list[str], *, profile: str = "", add: list[str] | None = None, remove: list[str] | None = None) -> dict[str, Any]:
    return {
        "action": "set_security",
        "label": label,
        "steps": [*steps, "Изменение касается только этого экземпляра и откатывается в его настройках → «Безопасность»."],
        "params": {"profile": profile, "add_permissions": list(add or []), "remove_permissions": list(remove or [])},
    }


def _profile_fixes(current_profile: str, *, include_compat: bool = True) -> list[dict[str, Any]]:
    fixes: list[dict[str, Any]] = []
    if include_compat and current_profile != "compat":
        fixes.append(_security_fix(
            "Ослабить защиту: профиль Compatibility",
            [f"Профиль безопасности: {current_profile} → compat", "Игра запускается без песочницы, но с очищенным окружением лаунчера."],
            profile="compat",
        ))
    if current_profile != "off":
        fixes.append(_security_fix(
            "Отключить защиту для экземпляра (Off)",
            [f"Профиль безопасности: {current_profile} → off", "Только для доверенных сборок: моды получают те же права, что и вы."],
            profile="off",
        ))
    return fixes


def _diagnose_sandbox(
    add: Callable[..., None],
    combined: str,
    report: str,
    settings: dict[str, Any],
    mods: list[ModEntry],
    instance_path: Path | None,
) -> None:
    profile = str(settings.get("security_profile", "strict") or "strict").strip().lower()
    raw_permissions = settings.get("security_permissions")
    permissions = [str(item) for item in raw_permissions] if isinstance(raw_permissions, list) else list(GAME_PERMISSIONS)

    launcher_error = _SANDBOX_LAUNCHER.search(combined)
    if launcher_error:
        text = launcher_error.group(0)
        fixes: list[dict[str, Any] | None] = []
        unsupported = [item for item in permissions if item in UNENFORCED_CELL_PERMISSIONS]
        host = [item for item in permissions if item in {"files:host-read", "files:host-write"}]
        if "не может безопасно выдать" in text and unsupported:
            fixes.append(_security_fix(
                "Убрать разрешения, которые песочница не умеет выдавать",
                [f"Снять: {', '.join(unsupported)}", f"Профиль {profile} остаётся включённым."],
                remove=unsupported,
            ))
        elif ("host-read" in text or "запись в файлы компьютера" in text) and host:
            fixes.append(_security_fix(
                "Закрыть модам доступ к файлам компьютера",
                [f"Снять: {', '.join(host)}", f"Профиль {profile} остаётся включённым."],
                remove=host,
            ))
        fixes.extend(_profile_fixes(profile))
        add(
            "security_sandbox",
            f"Песочница профиля «{profile}» не смогла запустить игру с текущими разрешениями.",
            fixes,
            [text.strip()[:300], *_evidence(combined, ["sandbox", "песочниц", "bwrap", "AppContainer"])],
            manual="Для Linux-профиля Strict нужен пакет bubblewrap (bwrap)." if "bubblewrap" in text else "",
        )
        return
    if profile not in SANDBOXED_PROFILES:
        return

    def culprits(pattern: re.Pattern[str]) -> list[str]:
        return _stack_culprits(combined, pattern, mods) or _crash_report_suspects(report, mods)

    def disable(files: list[str], label: str) -> dict[str, Any] | None:
        fix = _disable_fix(files, label)
        if fix:
            fix["steps"].insert(0, "Песочница сработала как задумано: мод попытался сделать то, что этому экземпляру запрещено.")
        return fix

    def grant(permission: str, label: str, why: str) -> dict[str, Any]:
        return _security_fix(label, [f"Выдать разрешение «{permission}» — {why}", f"Профиль {profile} остаётся включённым."], add=[permission])

    if "network:game" not in permissions and _SANDBOX_NETWORK.search(combined):
        add(
            "security_sandbox",
            "Сеть экземпляра выключена в настройках безопасности, поэтому игра или мод не смогли подключиться.",
            [grant("network:game", "Включить сеть для игры", "доступ к серверам и интернету"), disable(culprits(_SANDBOX_NETWORK), "Отключить мод, которому нужна сеть")],
            _evidence(combined, ["UnknownHost", "unreachable", "socket", "resolution", "host"]),
        )
    if "desktop:display" not in permissions and _SANDBOX_DISPLAY.search(combined):
        add(
            "security_sandbox",
            "Песочнице запрещён доступ к экрану и видеокарте — окно игры не может открыться.",
            [grant("desktop:display", "Разрешить окно игры и GPU", "без него Minecraft не создаёт окно"), *_profile_fixes(profile)],
            _evidence(combined, ["DISPLAY", "display", "X11", "Wayland", "/dev/"]),
        )
    files_match = _SANDBOX_FILES.search(combined)
    if files_match:
        denied_path = (files_match.group(1) or "").strip() if files_match.groups() else ""
        inside = bool(denied_path) and _path_inside_instance(denied_path, instance_path)
        files = culprits(_SANDBOX_FILES)
        fixes = []
        if "files:instance" not in permissions:
            fixes.append(grant("files:instance", "Разрешить запись в папку экземпляра", "моды сохраняют конфиги и миры"))
            fixes.append(disable(files, "Отключить мод, упёршийся в запрет"))
        else:
            fixes.append(disable(files, "Отключить мод, который лезет за пределы экземпляра"))
            fixes.extend(_profile_fixes(profile))
        add(
            "security_sandbox",
            ("Песочница запретила запись в папку экземпляра." if "files:instance" not in permissions
             else "Мод обратился к файлам, к которым у экземпляра нет доступа"
             + (f": {denied_path[:160]}" if denied_path and not inside else "") + "."),
            fixes,
            _evidence(combined, ["AccessDenied", "Access is denied", "Permission denied", "Read-only", "Отказано"]),
        )
    process_match = _SANDBOX_PROCESS.search(combined)
    if process_match and not re.search(r"javaw?(?:\.exe)?\"?$", (process_match.group(1) or "") if process_match.groups() else ""):
        add(
            "security_sandbox",
            "Мод попытался запустить внешнюю программу — профиль «" + profile + "» это запрещает.",
            [disable(culprits(_SANDBOX_PROCESS), "Отключить мод, запускающий программы"), *_profile_fixes(profile, include_compat=True)],
            _evidence(combined, ["Cannot run program", "CreateProcess", "error="]),
            manual="Разрешение «Запуск процессов» внутри песочницы не выдаётся: нужен профиль Compatibility или Off.",
        )
    if _SANDBOX_NATIVE.search(combined):
        add(
            "security_sandbox",
            "Мод не смог загрузить нативную библиотеку из-за ограничений песочницы.",
            [disable(culprits(_SANDBOX_NATIVE), "Отключить мод с нативным кодом"), *_profile_fixes(profile)],
            _evidence(combined, ["load library", "UnsatisfiedLink"]),
            manual="Разрешение «Нативный код» доступно только без песочницы (Compatibility или Off).",
        )


def _path_inside_instance(value: str, instance_path: Path | None) -> bool:
    candidate = value.strip().strip("'\"").replace("\\", "/")
    if candidate.startswith("/instance/") or not re.match(r"^(?:/|[A-Za-z]:/)", candidate):
        return True
    if instance_path is None:
        return False
    root = str(instance_path).replace("\\", "/").rstrip("/").lower()
    return candidate.lower().startswith(root + "/")


def _stack_culprits(text: str, pattern: re.Pattern[str], mods: list[ModEntry]) -> list[str]:
    """Моды, чей код стоит в стеке рядом с ошибкой песочницы."""
    if not text or not mods:
        return []
    by_id = {mod.mod_id: mod for mod in mods if mod.mod_id}
    lines = text.splitlines()
    files: list[str] = []
    for index, line in enumerate(lines):
        if not pattern.search(line):
            continue
        window = "\n".join(lines[index : index + 60])
        for match in _MIXIN_HANDLER.finditer(window):
            files.extend(_files_for_ids([match.group(1).lower()], by_id))
        for match in _STACK_FRAME.finditer(window):
            class_name = match.group(1)
            if class_name.startswith(_FRAMEWORK_PACKAGES):
                continue
            path = class_name.replace(".", "/")
            parts = path.split("/")
            prefixes = {"/".join(parts[:depth]) for depth in (3, 2) if len(parts) > depth}
            for mod in mods:
                if prefixes & mod.packages:
                    files.append(mod.file)
                    break
        if len(_unique(files)) >= 4:
            break
    return _unique(files)[:6]


# --- Конструкторы починок --------------------------------------------------------


def _disable_steps(files: list[str]) -> list[str]:
    shown = ", ".join(files[:6]) + (f" и ещё {len(files) - 6}" if len(files) > 6 else "")
    return [
        f"Переименовать в *.jar.disabled: {shown}",
        "Файлы не удаляются — включить их обратно можно на вкладке «Моды».",
    ]


def _disable_fix(files: list[str], label: str) -> dict[str, Any] | None:
    files = _unique(files)[:MAX_FILES_PER_FIX]
    if not files:
        return None
    return {"action": "disable_mods", "label": label, "steps": _disable_steps(files), "params": {"files": files}}


def _install_steps(ids: list[str]) -> list[str]:
    return [
        f"Найти в каталоге Modrinth и установить: {', '.join(ids[:8])}",
        "Берётся версия под Minecraft и загрузчик этого экземпляра; ненайденные зависимости будут перечислены.",
    ]


def _install_fix(ids: list[str]) -> dict[str, Any] | None:
    ids = [item for item in _unique(ids) if re.fullmatch(r"[\w.\-]{2,64}", item)][:MAX_FILES_PER_FIX]
    if not ids:
        return None
    return {"action": "install_dependencies", "label": "Скачать недостающие зависимости", "steps": _install_steps(ids), "params": {"ids": ids}}


def _memory_fix(current_kib: int, target_kib: int, label: str) -> dict[str, Any] | None:
    if target_kib <= 0 or target_kib == current_kib:
        return None
    target = format_memory_kib(target_kib)
    return {
        "action": "set_memory",
        "label": f"{label} до {_human_memory(target_kib)}",
        "steps": [
            f"Память экземпляра: {_human_memory(current_kib)} → {_human_memory(target_kib)}",
            "Сохраняется в настройках этого экземпляра; глобальные настройки не меняются.",
        ],
        "params": {"memory": target},
    }


def _java_fixes(required: int, java_path: str) -> list[dict[str, Any]]:
    from .runtime import discover_java_paths, java_version

    fixes: list[dict[str, Any]] = []
    candidates: list[tuple[int, Path]] = []
    for path in discover_java_paths():
        major = java_version(path)
        if major and (not required or major >= required) and str(path) != java_path:
            candidates.append((major, path))
    candidates.sort(key=lambda item: item[0])
    if candidates:
        major, path = candidates[0] if required else candidates[-1]
        fixes.append({
            "action": "set_java",
            "label": f"Использовать установленную Java {major}",
            "steps": [f"Java экземпляра → {path} (Java {major})", "Сохраняется в настройках этого экземпляра."],
            "params": {"java_path": str(path)},
        })
    if java_path:
        fixes.append({
            "action": "set_java",
            "label": "Вернуть автоматический выбор Java",
            "steps": [
                f"Убрать ручной путь к Java: {java_path}",
                "Лаунчер сам подберёт системную Java нужной версии или скачает официальную среду Mojang.",
            ],
            "params": {"java_path": ""},
        })
    elif not candidates and not required:
        fixes.append({
            "action": "set_java",
            "label": "Скачать подходящую Java автоматически",
            "steps": ["Оставить автоматический выбор Java: при следующем запуске лаунчер скачает официальную среду Mojang для этой версии."],
            "params": {"java_path": ""},
        })
    return fixes


def _jvm_reset_fix(settings: dict[str, Any]) -> dict[str, Any]:
    arguments = [str(item) for item in settings.get("jvm_arguments") or []]
    preset = str(settings.get("jvm_preset", "none") or "none")
    steps = []
    if arguments:
        steps.append("Убрать аргументы JVM: " + " ".join(arguments[:8]) + (" …" if len(arguments) > 8 else ""))
    if preset != "none":
        steps.append(f"Отключить пресет JVM «{preset}»")
    steps.append("Старые значения можно вернуть в настройках экземпляра.")
    return {"action": "reset_jvm_arguments", "label": "Сбросить аргументы JVM", "steps": steps, "params": {}}


def _repair_fix(loader: str) -> dict[str, Any]:
    target = "версию Minecraft" + (f" и {loader.capitalize()}" if loader and loader != "vanilla" else "")
    return {
        "action": "repair_version",
        "label": "Восстановить файлы версии и загрузчика",
        "steps": [
            f"Заново скачать и проверить {target}",
            "Старая версия сохраняется в резервной копии и возвращается, если восстановление не удастся.",
            "Моды, миры, конфиги и скриншоты не затрагиваются.",
        ],
        "params": {},
    }


def _update_loader_fix(loader: str, minimum: str, current: str, minecraft_version: str, *, reinstall: bool = False) -> dict[str, Any] | None:
    if loader not in LOADER_DEPENDENCY_IDS:
        return None
    title = LOADER_TITLES.get(loader, loader.capitalize())
    target = f"{minimum} или новее" if minimum else "последнюю стабильную версию"
    label = f"Переустановить {title} последней стабильной версии" if reinstall else f"Обновить {title} до {target}"
    return {
        "action": "update_loader",
        "label": label,
        "steps": [
            f"Скачать {title} {target}"
            + (f" для Minecraft {minecraft_version}" if minecraft_version else "")
            + (f" (сейчас {current})" if current else ""),
            "Старая версия загрузчика удаляется только после успешной установки новой.",
            "Моды, миры, конфиги и скриншоты не затрагиваются.",
        ],
        "params": {"minimum": minimum},
    }


def _config_fix(files: list[str]) -> dict[str, Any] | None:
    files = _unique(files)[:MAX_FILES_PER_FIX]
    if not files:
        return None
    return {
        "action": "backup_configs",
        "label": "Сбросить повреждённые конфиги",
        "steps": [
            "Перенести в резервную папку: " + ", ".join(files[:6]),
            f"Копии лежат в {BACKUP_DIRECTORY}/ внутри экземпляра — их можно вернуть вручную.",
            "При следующем запуске моды создадут конфиги по умолчанию.",
        ],
        "params": {"files": files},
    }


def _graphics_fixes(settings: dict[str, Any], mods: list[ModEntry]) -> list[dict[str, Any]]:
    fixes: list[dict[str, Any]] = []
    mode = str(settings.get("graphics_mode", "auto") or "auto")
    if mode != "auto":
        fixes.append({
            "action": "set_graphics_mode",
            "label": "Вернуть автоматический выбор видеокарты",
            "steps": [f"Режим графики экземпляра: {mode} → auto"],
            "params": {"mode": "auto"},
        })
    elif sys.platform.startswith("linux"):
        fixes.append({
            "action": "set_graphics_mode",
            "label": "Запускать через Mesa",
            "steps": ["Режим графики экземпляра: auto → mesa (открытый драйвер OpenGL)", "Вернуть режим можно в настройках экземпляра."],
            "params": {"mode": "mesa"},
        })
    shaders = [mod.file for mod in mods if mod.mod_id in SHADER_MOD_IDS or any(name in mod.file.lower() for name in ("iris", "oculus", "optifine", "optifabric"))]
    fix = _disable_fix(shaders, "Отключить моды шейдеров")
    if fix:
        fixes.append(fix)
    return fixes


def _recent_fix(mods: list[ModEntry], instance_path: Path | None) -> dict[str, Any] | None:
    if instance_path is None:
        return None
    last_success = read_state(instance_path).get("last_success")
    if not isinstance(last_success, (int, float)) or last_success <= 0:
        return None
    recent = [mod.file for mod in mods if mod.mtime > float(last_success)]
    if not recent or len(recent) > MAX_FILES_PER_FIX:
        return None
    fix = _disable_fix(recent, "Отключить моды, добавленные после последнего удачного запуска")
    return fix


# --- Вспомогательное ---------------------------------------------------------------


def _wrong_loader(mod: ModEntry, loader: str, by_id: dict[str, ModEntry]) -> bool:
    if not mod.loaders or mod.corrupted:
        return False
    if loader == "fabric":
        return "fabric" not in mod.loaders
    if loader == "quilt":
        return not ({"fabric", "quilt"} & mod.loaders)
    if loader == "forge":
        # Sinytra Connector запускает Fabric-моды на Forge — тогда это не ошибка.
        if "connector" in by_id and {"fabric", "quilt"} & mod.loaders:
            return False
        return "forge" not in mod.loaders
    if loader == "neoforge":
        if "connector" in by_id and {"fabric", "quilt"} & mod.loaders:
            return False
        return not ({"forge", "neoforge"} & mod.loaders)
    return False


def _provided_ids(mods: list[ModEntry]) -> set[str]:
    provided: set[str] = set()
    for mod in mods:
        if mod.mod_id:
            provided.add(mod.mod_id)
            provided.add(mod.mod_id.replace("_", "-"))
            provided.add(mod.mod_id.replace("-", "_"))
    return provided


def _files_for_ids(ids: list[str], by_id: dict[str, ModEntry]) -> list[str]:
    files: list[str] = []
    for mod_id in ids:
        key = mod_id.strip().lower()
        mod = by_id.get(key) or by_id.get(key.replace("-", "_")) or by_id.get(key.replace("_", "-"))
        if mod:
            files.append(mod.file)
    return _unique(files)


def _files_for_names(names: list[str], mods: list[ModEntry]) -> list[str]:
    files: list[str] = []
    for name in names:
        needle = re.sub(r"[^a-z0-9]", "", name.lower())
        if len(needle) < 3:
            continue
        for mod in mods:
            candidates = {re.sub(r"[^a-z0-9]", "", mod.mod_id), re.sub(r"[^a-z0-9]", "", mod.file.lower().split("-")[0])}
            if needle in candidates:
                files.append(mod.file)
                break
    return _unique(files)


def _crash_report_suspects(report: str, mods: list[ModEntry]) -> list[str]:
    if not report:
        return []
    by_id = {mod.mod_id: mod for mod in mods if mod.mod_id}
    by_file = {mod.file.lower(): mod.file for mod in mods}
    files: list[str] = []
    suspect_block = re.search(r"Suspected Mods?:(.*?)(?:\n\S|\n\n|\Z)", report, re.IGNORECASE | re.DOTALL)
    if suspect_block:
        files.extend(_files_for_ids([match.group(1).lower() for match in _CRASH_SUSPECT_ID.finditer(suspect_block.group(1))], by_id))
        files.extend(_files_for_ids(re.findall(r"\(([\w.\-]+)\)", suspect_block.group(1)), by_id))
    for match in _CRASH_MOD_FILE.finditer(report):
        name = match.group(1).lower()
        if name in by_file:
            files.append(by_file[name])
    # Ванильные/загрузочные файлы не считаем виновниками.
    return [file for file in _unique(files) if not re.match(r"(?:minecraft|client|forge|neoforge|fabric-loader|quilt-loader)[-.]", file.lower())][:10]


def _broken_configs(text: str, instance_path: Path | None) -> list[str]:
    if instance_path is None:
        return []
    root = instance_path / "config"
    if root.is_symlink() or not root.is_dir():
        return []
    found: list[str] = []
    candidates = [match.group(1) for match in _CONFIG_PATH.finditer(text)]
    candidates.extend(match.group(1) for match in _FORGE_CONFIG_FILE.finditer(text))
    for candidate in candidates:
        relative = candidate.replace("\\", "/").strip().lstrip("/")
        path = _safe_child(root, relative)
        if path is None or not path.is_file():
            # Forge пишет только имя файла — ищем его в корне config.
            path = _safe_child(root, Path(relative).name)
            if path is None or not path.is_file():
                continue
        found.append("config/" + path.relative_to(root).as_posix())
    return _unique(found)


def _safe_child(root: Path, relative: str) -> Path | None:
    if not relative or "\x00" in relative:
        return None
    parts = Path(relative).parts
    if any(part in {"..", ""} for part in parts) or Path(relative).is_absolute():
        return None
    candidate = root / relative
    try:
        resolved = candidate.resolve()
        resolved.relative_to(root.resolve())
    except (OSError, ValueError):
        return None
    if candidate.is_symlink():
        return None
    return candidate


def _evidence(text: str, needles: list[str]) -> list[str]:
    if not text:
        return []
    lowered_needles = [needle.lower() for needle in needles if needle]
    result: list[str] = []
    for line in text.splitlines():
        lowered = line.lower()
        if any(needle in lowered for needle in lowered_needles):
            stripped = line.strip()
            if stripped and stripped not in result:
                result.append(stripped[:300])
            if len(result) >= MAX_EVIDENCE:
                break
    return result


def _tail_evidence(lines: list[str]) -> list[str]:
    for index in range(len(lines) - 1, max(-1, len(lines) - 200), -1):
        if re.search(r"error|exception|caused by", lines[index], re.IGNORECASE):
            return [line.strip()[:300] for line in lines[index : index + MAX_EVIDENCE] if line.strip()]
    return [line.strip()[:300] for line in lines[-MAX_EVIDENCE:] if line.strip()]


def _unique(values: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if value and value not in seen:
            seen.add(value)
            result.append(value)
    return result


def _dependency_id(match: re.Match[str]) -> str:
    return (match.group(5) or match.group(6) or match.group(7) or match.group(4) or "").strip().lower()


def _first_version(value: str) -> str:
    match = re.search(r"\d+(?:\.\d+)+(?:[-+][\w.]+)?|\d+", value or "")
    return match.group(0) if match else ""


_PRERELEASE = re.compile(r"alpha|beta|rc|pre|snapshot|dev", re.IGNORECASE)


def _version_key(value: str) -> tuple[Any, ...]:
    """Ключ сравнения версий загрузчиков: 0.16.10 > 0.16.9, 47.3.0 > 47.2.20."""
    core = re.split(r"[-+]", str(value), maxsplit=1)[0]
    numbers = tuple(int(part) for part in re.findall(r"\d+", core)[:6])
    stable = 0 if _PRERELEASE.search(str(value)) else 1
    return (numbers, stable)


def pick_loader_version(choices: list[str], minimum: str = "", current: str = "") -> str:
    """Выбрать версию загрузчика: новейшая стабильная, не ниже ``minimum``.

    Списки версий у Fabric/Quilt идут от новых к старым, у Forge/NeoForge —
    наоборот, поэтому порядок входа не используется.
    """
    values = _unique([str(item).strip() for item in choices if str(item).strip()])
    if not values:
        return ""
    minimum_key = _version_key(minimum)[0] if minimum else ()
    eligible = [item for item in values if not minimum_key or _version_key(item)[0] >= minimum_key]
    if not eligible:
        return ""
    stable = [item for item in eligible if not _PRERELEASE.search(item)]
    return max(stable or eligible, key=_version_key)


def _first_int(value: str) -> int:
    match = re.search(r"\d{1,2}", value or "")
    return int(match.group(0)) if match else 0


def _human_memory(kib: int) -> str:
    gib = kib / (1024 * 1024)
    if gib >= 1:
        return f"{gib:.1f}".rstrip("0").rstrip(".") + " ГБ"
    return f"{kib // 1024} МБ"


# --- Применение починок -----------------------------------------------------------


@dataclass
class RepairContext:
    instance_name: str
    instance_path: Path
    save_settings: Callable[[dict[str, Any]], Any]
    settings: Callable[[], dict[str, Any]]
    toggle_mod: Callable[[str], Any]
    repair_version: Callable[[], Any]
    install_mod: Callable[[str], Any]
    # Обновление загрузчика: (минимальная версия) -> установленная версия.
    update_loader: Callable[[str], str] | None = None
    # Смена аккаунта по умолчанию: (account_id) -> отображаемое имя.
    set_account: Callable[[str], str] | None = None


def apply_fixes(context: RepairContext, fixes: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not isinstance(fixes, list) or not fixes:
        raise LauncherError("Не выбрано ни одной починки")
    if len(fixes) > 20:
        raise LauncherError("Слишком много починок за раз")
    results: list[dict[str, Any]] = []
    # Восстановление версии и обновление загрузчика — самые долгие и не зависят
    # от остальных, делаем последними.
    ordered = sorted(fixes, key=lambda fix: 1 if isinstance(fix, dict) and fix.get("action") in {"repair_version", "update_loader"} else 0)
    for fix in ordered:
        if not isinstance(fix, dict):
            continue
        action = str(fix.get("action", ""))
        params = fix.get("params") if isinstance(fix.get("params"), dict) else {}
        fix_id = str(fix.get("id", action))[:120]
        if action not in FIX_ACTIONS:
            results.append({"id": fix_id, "ok": False, "message": f"Неизвестная починка: {action}"})
            continue
        try:
            message = _apply_one(context, action, params)
            results.append({"id": fix_id, "ok": True, "message": message})
        except LauncherError as exc:
            results.append({"id": fix_id, "ok": False, "message": str(exc)})
        except OSError as exc:
            results.append({"id": fix_id, "ok": False, "message": f"Ошибка файловой системы: {exc}"})
    return results


def _string_list(value: Any, limit: int = MAX_FILES_PER_FIX) -> list[str]:
    if not isinstance(value, list):
        raise LauncherError("Параметры починки имеют неожиданный формат")
    return [str(item) for item in value[:limit] if str(item).strip()]


def _apply_one(context: RepairContext, action: str, params: dict[str, Any]) -> str:
    if action == "disable_mods":
        files = _string_list(params.get("files"))
        done: list[str] = []
        skipped: list[str] = []
        for name in files:
            # только имя файла прямо в mods, без подкаталогов
            if Path(name).name != name or name.lower().endswith(".disabled"):
                skipped.append(name)
                continue
            try:
                context.toggle_mod(name)
                done.append(name)
            except LauncherError:
                skipped.append(name)
        if not done:
            raise LauncherError("Не удалось отключить ни одного мода" + (f": {', '.join(skipped[:4])}" if skipped else ""))
        return f"Отключено модов: {len(done)}" + (f"; пропущено: {', '.join(skipped[:4])}" if skipped else "")

    if action == "install_dependencies":
        ids = [item for item in _string_list(params.get("ids")) if re.fullmatch(r"[\w.\-]{2,64}", item)]
        installed: list[str] = []
        failed: list[str] = []
        for project_id in ids:
            try:
                context.install_mod(project_id)
                installed.append(project_id)
            except Exception:  # noqa: BLE001 — сеть/каталог: просто не нашли
                failed.append(project_id)
        if not installed:
            raise LauncherError("Не удалось найти зависимости в каталоге: " + ", ".join(failed[:6]) + ". Установите их вручную на вкладке «Моды».")
        return f"Установлено: {', '.join(installed)}" + (f"; не найдено: {', '.join(failed)}" if failed else "")

    if action == "set_memory":
        memory = str(params.get("memory", "")).strip()
        kib = parse_memory_kib(memory)
        total = system_memory_kib()
        if kib < 512 * 1024 or (total and kib > total):
            raise LauncherError("Недопустимый объём памяти")
        context.save_settings({"memory": format_memory_kib(kib)})
        return f"Память экземпляра: {_human_memory(kib)}"

    if action == "set_java":
        from .runtime import validate_java_path

        java_path = str(params.get("java_path", "")).strip()
        if java_path:
            java_path = str(validate_java_path(java_path, allow_directory=True))
        context.save_settings({"java_path": java_path})
        return f"Java экземпляра: {java_path}" if java_path else "Java выбирается автоматически"

    if action == "reset_jvm_arguments":
        context.save_settings({"jvm_arguments": [], "jvm_preset": "none"})
        return "Аргументы JVM и пресет сброшены"

    if action == "set_graphics_mode":
        mode = str(params.get("mode", "")).strip().lower()
        if mode not in {"auto", "nvidia", "mesa", "software"}:
            raise LauncherError("Неизвестный режим графики")
        context.save_settings({"graphics_mode": mode})
        return f"Режим графики: {mode}"

    if action == "backup_configs":
        files = _string_list(params.get("files"))
        root = context.instance_path / "config"
        backup_root = context.instance_path / BACKUP_DIRECTORY / time.strftime("%Y%m%d-%H%M%S")
        moved: list[str] = []
        for name in files:
            relative = name.replace("\\", "/").removeprefix("config/")
            source = _safe_child(root, relative)
            if source is None or not source.is_file():
                continue
            target = backup_root / "config" / relative
            if target.exists():
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(source), str(target))
            moved.append("config/" + relative)
        if not moved:
            raise LauncherError("Конфиги не найдены — возможно, их уже пересоздали")
        return f"Перенесено в {BACKUP_DIRECTORY}/: {', '.join(moved[:6])}"

    if action == "repair_version":
        context.repair_version()
        return "Версия и загрузчик восстановлены"

    if action == "update_loader":
        if context.update_loader is None:
            raise LauncherError("Обновление загрузчика недоступно для этого экземпляра")
        minimum = str(params.get("minimum", "")).strip()
        if minimum and not re.fullmatch(r"[\w.+\-]{1,64}", minimum):
            raise LauncherError("Некорректная версия загрузчика")
        installed = context.update_loader(minimum)
        return f"Загрузчик установлен: {installed}" if installed else "Загрузчик переустановлен"

    if action == "set_account":
        if context.set_account is None:
            raise LauncherError("Смена аккаунта недоступна")
        account_id = str(params.get("account_id", "")).strip()
        if not account_id or len(account_id) > 200 or not re.fullmatch(r"[\w.\-]+", account_id):
            raise LauncherError("Некорректный аккаунт")
        label = context.set_account(account_id)
        return f"Аккаунт по умолчанию: {label or account_id}"

    if action == "login_microsoft":
        # Вход по коду устройства интерактивный: его запускает интерфейс
        # после применения починок (см. AutoRepairDialog).
        return "Откроется вход Microsoft — подтвердите его в браузере"

    if action == "set_security":
        current = context.settings()
        profile = str(params.get("profile", "") or current.get("security_profile", "strict")).strip().lower()
        if profile not in SECURITY_PROFILES:
            raise LauncherError(f"Неизвестный профиль безопасности: {profile}")
        add_permissions = [item for item in _string_list(params.get("add_permissions") or []) if item in SECURITY_PERMISSIONS]
        remove_permissions = {item for item in _string_list(params.get("remove_permissions") or []) if item in SECURITY_PERMISSIONS}
        raw = current.get("security_permissions")
        permissions = [str(item) for item in raw] if isinstance(raw, list) else list(GAME_PERMISSIONS)
        permissions = [item for item in permissions if item not in remove_permissions]
        permissions.extend(item for item in add_permissions if item not in permissions)
        if profile in SANDBOXED_PROFILES:
            # Эти разрешения песочница выдать не может — запуск упал бы снова.
            permissions = [item for item in permissions if item not in UNENFORCED_CELL_PERMISSIONS]
        context.save_settings({"security_profile": profile, "security_permissions": permissions})
        changes = [f"профиль {profile}"]
        if add_permissions:
            changes.append("выдано: " + ", ".join(add_permissions))
        if remove_permissions:
            changes.append("снято: " + ", ".join(sorted(remove_permissions)))
        return "Безопасность экземпляра: " + "; ".join(changes)

    raise LauncherError(f"Неизвестная починка: {action}")


def normalize_muted(value: Any) -> list[str]:
    if not isinstance(value, (list, tuple)):
        return []
    return [kind for kind in ISSUE_KINDS if kind in {str(item) for item in value}]


def issue_kinds_public() -> list[dict[str, str]]:
    return [{"id": kind, "title": title} for kind, title in ISSUE_KINDS.items()]


__all__ = [
    "ISSUE_KINDS",
    "RepairContext",
    "apply_fixes",
    "diagnose",
    "issue_kinds_public",
    "normalize_muted",
    "pick_loader_version",
    "record_launch_result",
    "scan_mods",
]
