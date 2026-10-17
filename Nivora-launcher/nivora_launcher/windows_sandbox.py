"""Windows-песочница экземпляра: AppContainer + Job Object.

Аналог bubblewrap-обёртки из security.py. Лаунчер не может создать процесс
в AppContainer через subprocess.Popen, поэтому команда игры оборачивается
в короткоживущий helper (этот модуль, режим ``--win-sandbox``):

    helper <policy> -- java.exe ...

Helper работает с правами пользователя: создаёт профиль AppContainer
экземпляра, выдаёт его SID доступ только к нужным путям (папка экземпляра,
Java runtime, Java argument file), запускает Java в AppContainer внутри
Job Object и возвращает код выхода Java. Stdout/stderr Java пишет прямо
в унаследованные пайпы лаунчера.

Что даёт AppContainer:
- файловая система: процесс видит только объекты, где явно разрешён его SID
  (или ALL APPLICATION PACKAGES — системные каталоги). Профиль пользователя,
  другие диски и данные лаунчера закрыты;
- сеть: без capabilities internetClient/privateNetworkClientServer сети нет
  вовсе (включая loopback);
- Low integrity + UIPI: нельзя слать сообщения и ставить хуки окнам процессов
  с более высоким уровнем целостности; микрофон и камера закрыты без
  соответствующих capabilities;
- отдельное пространство именованных объектов (аналог --unshare-ipc).

Job Object: игра умирает вместе с helper (KILL_ON_JOB_CLOSE, аналог
--die-with-parent), не может запускать дочерние процессы (лимит 1 процесс)
и получает UI-ограничения (рабочие столы, выход из системы, системные
параметры; без desktop:display — ещё буфер обмена и чужие окна).
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Sequence

POLICY_VERSION = 1
HELPER_FLAG = "--win-sandbox"
PROFILE_PREFIX = "Nivora.Cell."
PROBE_PROFILE = "Nivora.Probe"
MAX_POLICY_PATHS = 64
MAX_POLICY_BYTES = 64 * 1024
SANDBOX_ENV_PREFIX = "NIVORA_SANDBOX_ENV_"
# Переменные, которые для игры указывают в cell, а для helper-а остаются
# пользовательскими (см. child_environment).
CELL_ENVIRONMENT_KEYS = ("HOME", "TMPDIR", "TMP", "TEMP", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA")

# Well-known capability SIDs (winnt.h, SECURITY_CAPABILITY_*).
CAPABILITY_INTERNET_CLIENT = "S-1-15-3-1"
CAPABILITY_INTERNET_CLIENT_SERVER = "S-1-15-3-2"
CAPABILITY_PRIVATE_NETWORK_CLIENT_SERVER = "S-1-15-3-3"

# JOB_OBJECT_UILIMIT_*
UILIMIT_HANDLES = 0x0001
UILIMIT_READCLIPBOARD = 0x0002
UILIMIT_WRITECLIPBOARD = 0x0004
UILIMIT_SYSTEMPARAMETERS = 0x0008
UILIMIT_DISPLAYSETTINGS = 0x0010
UILIMIT_GLOBALATOMS = 0x0020
UILIMIT_DESKTOP = 0x0040
UILIMIT_EXITWINDOWS = 0x0080

# Права, которые снимаются с SID песочницы на служебных файлах лаунчера:
# запись данных, дозапись, атрибуты, удаление, смена ACL и владельца.
DENY_WRITE_RIGHTS = "(WD,AD,WEA,WA,DE,WDAC,WO)"


class WindowsSandboxError(RuntimeError):
    pass


# ── Политика: чистые функции, тестируются на любой ОС ─────────────────────


def appcontainer_name(instance_dir: Path) -> str:
    """Стабильное имя профиля для экземпляра (лимит Windows — 64 символа)."""
    normalized = os.path.normcase(os.path.abspath(str(instance_dir)))
    digest = hashlib.sha256(normalized.encode("utf-8")).hexdigest()[:32]
    return PROFILE_PREFIX + digest


def ui_restrictions(display_access: bool) -> int:
    restrictions = UILIMIT_DESKTOP | UILIMIT_EXITWINDOWS | UILIMIT_SYSTEMPARAMETERS | UILIMIT_GLOBALATOMS
    if not display_access:
        restrictions |= UILIMIT_HANDLES | UILIMIT_READCLIPBOARD | UILIMIT_WRITECLIPBOARD | UILIMIT_DISPLAYSETTINGS
    return restrictions


def network_capabilities(network_access: bool) -> list[str]:
    if not network_access:
        return []
    return [CAPABILITY_INTERNET_CLIENT, CAPABILITY_INTERNET_CLIENT_SERVER, CAPABILITY_PRIVATE_NETWORK_CLIENT_SERVER]


def encode_policy(policy: dict[str, Any]) -> str:
    raw = json.dumps(validate_policy(policy), ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode("utf-8")
    return base64.urlsafe_b64encode(raw).decode("ascii")


def decode_policy(value: str) -> dict[str, Any]:
    if len(value) > MAX_POLICY_BYTES * 2:
        raise WindowsSandboxError("Политика песочницы слишком большая")
    try:
        raw = base64.urlsafe_b64decode(value.encode("ascii"))
        policy = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeError) as exc:
        raise WindowsSandboxError("Политика песочницы повреждена") from exc
    return validate_policy(policy)


def validate_policy(policy: Any) -> dict[str, Any]:
    if not isinstance(policy, dict) or policy.get("version") != POLICY_VERSION:
        raise WindowsSandboxError("Неподдерживаемая политика песочницы")
    name = policy.get("profile_name")
    if not isinstance(name, str) or not name.startswith(PROFILE_PREFIX) or len(name) > 64 or not name.replace(".", "").isalnum():
        raise WindowsSandboxError("Некорректное имя профиля AppContainer")
    instance_dir = policy.get("instance_dir")
    if not isinstance(instance_dir, str) or not os.path.isabs(instance_dir):
        raise WindowsSandboxError("Папка экземпляра в политике должна быть абсолютным путём")
    if policy.get("instance_access") not in {"modify", "read"}:
        raise WindowsSandboxError("Некорректный доступ к папке экземпляра")
    for key in ("writable_dirs", "read_dirs", "read_files", "deny_write_files"):
        values = policy.get(key)
        if not isinstance(values, list) or len(values) > MAX_POLICY_PATHS:
            raise WindowsSandboxError(f"Некорректный список путей: {key}")
        for item in values:
            if not isinstance(item, str) or not os.path.isabs(item):
                raise WindowsSandboxError(f"Путь в политике должен быть абсолютным: {key}")
    for key in ("network", "display", "audio"):
        if not isinstance(policy.get(key), bool):
            raise WindowsSandboxError(f"Некорректный флаг политики: {key}")
    return {
        "version": POLICY_VERSION,
        "profile_name": name,
        "instance_dir": instance_dir,
        "instance_access": policy["instance_access"],
        "writable_dirs": list(policy["writable_dirs"]),
        "read_dirs": list(policy["read_dirs"]),
        "read_files": list(policy["read_files"]),
        "deny_write_files": list(policy["deny_write_files"]),
        "network": policy["network"],
        "display": policy["display"],
        "audio": policy["audio"],
    }


def helper_command() -> list[str]:
    """Команда запуска helper: собранный бэкенд или исходники."""
    if getattr(sys, "frozen", False):
        return [sys.executable, HELPER_FLAG]
    package_root = str(Path(__file__).resolve().parent.parent)
    code = (
        "import sys; sys.path.insert(0, " + repr(package_root) + "); "
        "from nivora_launcher.windows_sandbox import main; raise SystemExit(main(sys.argv[1:]))"
    )
    return [sys.executable, "-c", code]


def wrap_command(policy: dict[str, Any], command: Sequence[str]) -> list[str]:
    return [*helper_command(), encode_policy(policy), "--", *command]


def child_environment(environment: dict[str, str]) -> dict[str, str]:
    """Окружение Java: переменные cell подставляются из NIVORA_SANDBOX_ENV_*.

    Сам helper (и загрузчик PyInstaller, распаковывающийся в TEMP) работает
    с настоящими TEMP/LOCALAPPDATA пользователя: каталог cell доступен игре
    на запись, распаковывать туда код helper-а нельзя.
    """
    result: dict[str, str] = {}
    overrides: dict[str, str] = {}
    for key, value in environment.items():
        upper = key.upper()
        if upper.startswith(("_MEI", "_PYI")):
            continue
        if upper.startswith(SANDBOX_ENV_PREFIX):
            target = key[len(SANDBOX_ENV_PREFIX):]
            if target:
                overrides[target] = value
            continue
        result[key] = value
    for target, value in overrides.items():
        for existing in [key for key in result if key.upper() == target.upper()]:
            del result[existing]
        result[target] = value
    return result


def environment_block(environment: dict[str, str]) -> str:
    items = sorted(child_environment(environment).items(), key=lambda item: item[0].upper())
    return "".join(f"{key}={value}\0" for key, value in items if key and "=" not in key) + "\0"


def acl_marker_path(profile_name: str) -> Path:
    # Метка о выданных ACL хранится вне папки экземпляра: игра может писать
    # в экземпляр и не должна подделывать то, что лаунчер считает выданным.
    base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    return Path(base) / "nivora-launcher" / "sandbox-acl" / f"{profile_name}.json"


def acl_state(policy: dict[str, Any], sid: str, stamps: dict[str, int] | None = None) -> dict[str, Any]:
    return {
        "sid": sid,
        "instance_dir": policy["instance_dir"],
        "instance_access": policy["instance_access"],
        "writable_dirs": sorted(policy["writable_dirs"]),
        "read_dirs": sorted(policy["read_dirs"]),
        # Время создания каталогов: пересозданный экземпляр или обновлённая
        # Java теряют выданные ACE, и метка не должна их «помнить».
        "stamps": dict(sorted((stamps or {}).items())),
    }


def instance_grants(grantee: str, access: str) -> list[str]:
    if access == "read":
        return [f"{grantee}:(OI)(CI)(RX)"]
    # Содержимое — полная запись (M), сам каталог экземпляра — без DELETE:
    # игра не может удалить или переименовать корень, как и в bubblewrap.
    return [f"{grantee}:(OI)(CI)(IO)(M)", f"{grantee}:(RD,X,RA,REA,RC,S,WD,AD,WA,WEA)"]


def _path_stamp(path: str) -> int:
    try:
        metadata = os.stat(path)
    except OSError:
        return 0
    birth = getattr(metadata, "st_birthtime_ns", None)
    return int(birth if birth is not None else metadata.st_ctime_ns)


# ── Win32 (ctypes) ───────────────────────────────────────────────────────


def appcontainer_supported() -> bool:
    if sys.platform != "win32":
        return False
    try:
        import ctypes

        userenv = ctypes.WinDLL("userenv", use_last_error=True)
        return hasattr(userenv, "CreateAppContainerProfile") and hasattr(userenv, "DeriveAppContainerSidFromAppContainerName")
    except (AttributeError, OSError):
        return False


class _Win32:
    """Ленивая привязка Win32 API; создаётся только на Windows."""

    def __init__(self) -> None:
        import ctypes
        from ctypes import wintypes

        self.ctypes = ctypes
        self.wintypes = wintypes
        self.kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
        self.userenv = ctypes.WinDLL("userenv", use_last_error=True)

        HANDLE = wintypes.HANDLE
        DWORD = wintypes.DWORD
        BOOL = wintypes.BOOL
        LPVOID = ctypes.c_void_p
        SIZE_T = ctypes.c_size_t

        class SID_AND_ATTRIBUTES(ctypes.Structure):
            _fields_ = [("Sid", LPVOID), ("Attributes", DWORD)]

        class SECURITY_CAPABILITIES(ctypes.Structure):
            _fields_ = [
                ("AppContainerSid", LPVOID),
                ("Capabilities", ctypes.POINTER(SID_AND_ATTRIBUTES)),
                ("CapabilityCount", DWORD),
                ("Reserved", DWORD),
            ]

        class STARTUPINFOW(ctypes.Structure):
            _fields_ = [
                ("cb", DWORD),
                ("lpReserved", wintypes.LPWSTR),
                ("lpDesktop", wintypes.LPWSTR),
                ("lpTitle", wintypes.LPWSTR),
                ("dwX", DWORD),
                ("dwY", DWORD),
                ("dwXSize", DWORD),
                ("dwYSize", DWORD),
                ("dwXCountChars", DWORD),
                ("dwYCountChars", DWORD),
                ("dwFillAttribute", DWORD),
                ("dwFlags", DWORD),
                ("wShowWindow", wintypes.WORD),
                ("cbReserved2", wintypes.WORD),
                ("lpReserved2", LPVOID),
                ("hStdInput", HANDLE),
                ("hStdOutput", HANDLE),
                ("hStdError", HANDLE),
            ]

        class STARTUPINFOEXW(ctypes.Structure):
            _fields_ = [("StartupInfo", STARTUPINFOW), ("lpAttributeList", LPVOID)]

        class PROCESS_INFORMATION(ctypes.Structure):
            _fields_ = [("hProcess", HANDLE), ("hThread", HANDLE), ("dwProcessId", DWORD), ("dwThreadId", DWORD)]

        class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("PerProcessUserTimeLimit", ctypes.c_int64),
                ("PerJobUserTimeLimit", ctypes.c_int64),
                ("LimitFlags", DWORD),
                ("MinimumWorkingSetSize", SIZE_T),
                ("MaximumWorkingSetSize", SIZE_T),
                ("ActiveProcessLimit", DWORD),
                ("Affinity", ctypes.c_size_t),
                ("PriorityClass", DWORD),
                ("SchedulingClass", DWORD),
            ]

        class IO_COUNTERS(ctypes.Structure):
            _fields_ = [(name, ctypes.c_uint64) for name in (
                "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
                "ReadTransferCount", "WriteTransferCount", "OtherTransferCount",
            )]

        class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
                ("IoInfo", IO_COUNTERS),
                ("ProcessMemoryLimit", SIZE_T),
                ("JobMemoryLimit", SIZE_T),
                ("PeakProcessMemoryUsed", SIZE_T),
                ("PeakJobMemoryUsed", SIZE_T),
            ]

        class JOBOBJECT_BASIC_UI_RESTRICTIONS(ctypes.Structure):
            _fields_ = [("UIRestrictionsClass", DWORD)]

        class TOKEN_USER(ctypes.Structure):
            _fields_ = [("User", SID_AND_ATTRIBUTES)]

        self.SID_AND_ATTRIBUTES = SID_AND_ATTRIBUTES
        self.SECURITY_CAPABILITIES = SECURITY_CAPABILITIES
        self.STARTUPINFOEXW = STARTUPINFOEXW
        self.PROCESS_INFORMATION = PROCESS_INFORMATION
        self.JOBOBJECT_EXTENDED_LIMIT_INFORMATION = JOBOBJECT_EXTENDED_LIMIT_INFORMATION
        self.JOBOBJECT_BASIC_UI_RESTRICTIONS = JOBOBJECT_BASIC_UI_RESTRICTIONS
        self.TOKEN_USER = TOKEN_USER

        def bind(library: Any, name: str, restype: Any, *argtypes: Any) -> Any:
            function = getattr(library, name)
            function.restype = restype
            function.argtypes = argtypes
            return function

        k32, adv, uenv = self.kernel32, self.advapi32, self.userenv
        self.CreateAppContainerProfile = bind(
            uenv, "CreateAppContainerProfile", ctypes.c_long,
            wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPCWSTR, LPVOID, DWORD, ctypes.POINTER(LPVOID),
        )
        self.DeriveAppContainerSidFromAppContainerName = bind(
            uenv, "DeriveAppContainerSidFromAppContainerName", ctypes.c_long, wintypes.LPCWSTR, ctypes.POINTER(LPVOID)
        )
        self.FreeSid = bind(adv, "FreeSid", LPVOID, LPVOID)
        self.ConvertSidToStringSidW = bind(adv, "ConvertSidToStringSidW", BOOL, LPVOID, ctypes.POINTER(wintypes.LPWSTR))
        self.ConvertStringSidToSidW = bind(adv, "ConvertStringSidToSidW", BOOL, wintypes.LPCWSTR, ctypes.POINTER(LPVOID))
        self.OpenProcessToken = bind(adv, "OpenProcessToken", BOOL, HANDLE, DWORD, ctypes.POINTER(HANDLE))
        self.GetTokenInformation = bind(adv, "GetTokenInformation", BOOL, HANDLE, ctypes.c_int, LPVOID, DWORD, ctypes.POINTER(DWORD))
        self.LocalFree = bind(k32, "LocalFree", LPVOID, LPVOID)
        self.GetCurrentProcess = bind(k32, "GetCurrentProcess", HANDLE)
        self.GetStdHandle = bind(k32, "GetStdHandle", HANDLE, DWORD)
        self.GetHandleInformation = bind(k32, "GetHandleInformation", BOOL, HANDLE, ctypes.POINTER(DWORD))
        self.SetHandleInformation = bind(k32, "SetHandleInformation", BOOL, HANDLE, DWORD, DWORD)
        self.InitializeProcThreadAttributeList = bind(
            k32, "InitializeProcThreadAttributeList", BOOL, LPVOID, DWORD, DWORD, ctypes.POINTER(SIZE_T)
        )
        self.UpdateProcThreadAttribute = bind(
            k32, "UpdateProcThreadAttribute", BOOL, LPVOID, DWORD, ctypes.c_size_t, LPVOID, SIZE_T, LPVOID, LPVOID
        )
        self.DeleteProcThreadAttributeList = bind(k32, "DeleteProcThreadAttributeList", None, LPVOID)
        self.CreateProcessW = bind(
            k32, "CreateProcessW", BOOL,
            wintypes.LPCWSTR, wintypes.LPWSTR, LPVOID, LPVOID, BOOL, DWORD, LPVOID, wintypes.LPCWSTR,
            ctypes.POINTER(STARTUPINFOEXW), ctypes.POINTER(PROCESS_INFORMATION),
        )
        self.CreateJobObjectW = bind(k32, "CreateJobObjectW", HANDLE, LPVOID, wintypes.LPCWSTR)
        self.SetInformationJobObject = bind(k32, "SetInformationJobObject", BOOL, HANDLE, ctypes.c_int, LPVOID, DWORD)
        self.AssignProcessToJobObject = bind(k32, "AssignProcessToJobObject", BOOL, HANDLE, HANDLE)
        self.ResumeThread = bind(k32, "ResumeThread", DWORD, HANDLE)
        self.TerminateProcess = bind(k32, "TerminateProcess", BOOL, HANDLE, ctypes.c_uint)
        self.WaitForSingleObject = bind(k32, "WaitForSingleObject", DWORD, HANDLE, DWORD)
        self.GetExitCodeProcess = bind(k32, "GetExitCodeProcess", BOOL, HANDLE, ctypes.POINTER(DWORD))
        self.CloseHandle = bind(k32, "CloseHandle", BOOL, HANDLE)

    def error(self, message: str) -> WindowsSandboxError:
        code = self.ctypes.get_last_error()
        return WindowsSandboxError(f"{message} (Win32 {code}: {self.ctypes.FormatError(code).strip()})")

    def sid_to_string(self, sid: int) -> str:
        buffer = self.wintypes.LPWSTR()
        if not self.ConvertSidToStringSidW(sid, self.ctypes.byref(buffer)):
            raise self.error("Не удалось прочитать SID")
        try:
            return str(buffer.value)
        finally:
            self.LocalFree(buffer)

    def string_to_sid(self, value: str) -> int:
        sid = self.ctypes.c_void_p()
        if not self.ConvertStringSidToSidW(value, self.ctypes.byref(sid)):
            raise self.error(f"Некорректный SID {value}")
        return int(sid.value or 0)


def _win32() -> _Win32:
    if sys.platform != "win32":
        raise WindowsSandboxError("Windows-песочница доступна только на Windows")
    return _Win32()


def _appcontainer_sid(api: _Win32, name: str) -> tuple[int, bool]:
    """Создаёт (или находит) профиль. Возвращает SID и признак «освободить через FreeSid»."""
    ctypes = api.ctypes
    sid = ctypes.c_void_p()
    result = api.CreateAppContainerProfile(
        name,
        "Nivora Launcher instance",
        "Песочница игрового экземпляра Nivora Launcher",
        None,
        0,
        ctypes.byref(sid),
    )
    already_exists = ctypes.c_long(0x800700B7).value  # HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)
    if result == already_exists:
        result = api.DeriveAppContainerSidFromAppContainerName(name, ctypes.byref(sid))
    if result != 0 or not sid.value:
        raise WindowsSandboxError(f"Не удалось создать профиль AppContainer {name} (HRESULT 0x{result & 0xFFFFFFFF:08X})")
    return int(sid.value), True


def current_user_sid() -> str:
    api = _win32()
    ctypes, wintypes = api.ctypes, api.wintypes
    token = wintypes.HANDLE()
    if not api.OpenProcessToken(api.GetCurrentProcess(), 0x0008, ctypes.byref(token)):  # TOKEN_QUERY
        raise api.error("Не удалось открыть токен процесса")
    try:
        size = wintypes.DWORD(0)
        api.GetTokenInformation(token, 1, None, 0, ctypes.byref(size))  # TokenUser
        buffer = ctypes.create_string_buffer(size.value)
        if not api.GetTokenInformation(token, 1, buffer, size, ctypes.byref(size)):
            raise api.error("Не удалось прочитать пользователя токена")
        user = ctypes.cast(buffer, ctypes.POINTER(api.TOKEN_USER)).contents
        return api.sid_to_string(user.User.Sid)
    finally:
        api.CloseHandle(token)


def _icacls(*arguments: str) -> None:
    system_root = os.environ.get("SystemRoot") or r"C:\Windows"
    icacls = str(Path(system_root) / "System32" / "icacls.exe")
    result = subprocess.run(
        [icacls, *arguments, "/Q"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=600,
        check=False,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()[:300]
        raise WindowsSandboxError(f"icacls {arguments[0]}: {detail or f'код {result.returncode}'}")


def restrict_to_current_user(path: Path) -> None:
    """Аналог chmod 0600: только текущий пользователь, без наследуемых ACE."""
    sid = current_user_sid()
    _icacls(str(path), "/inheritance:r", "/grant:r", f"*{sid}:(F)")


def apply_acl(policy: dict[str, Any], sid: str) -> None:
    grantee = f"*{sid}"
    marker = acl_marker_path(policy["profile_name"])
    directories = [policy["instance_dir"], *policy["writable_dirs"], *policy["read_dirs"]]
    state = acl_state(policy, sid, {path: _path_stamp(path) for path in directories})
    try:
        previous = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        previous = None
    if previous != state:
        instance_dir = policy["instance_dir"]
        # Сначала снимаем прежний ACE: так переключение modify → read
        # действительно отбирает запись, а не добавляет второе правило.
        _icacls(instance_dir, "/remove:g", grantee)
        for grant in instance_grants(grantee, policy["instance_access"]):
            _icacls(instance_dir, "/grant", grant)
        for directory in policy["writable_dirs"]:
            _icacls(directory, "/grant", f"{grantee}:(OI)(CI)(M)")
        for directory in policy["read_dirs"]:
            try:
                _icacls(directory, "/grant", f"{grantee}:(OI)(CI)(RX)")
            except WindowsSandboxError:
                # Java в Program Files: пользователь не может менять ACL, но
                # ALL APPLICATION PACKAGES там и так читает. Если нет — Java
                # не стартует, и ошибка CreateProcess объяснит причину.
                pass
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(json.dumps(state), encoding="utf-8")
    for path in policy["read_files"]:
        _icacls(path, "/grant", f"{grantee}:(R)")
    for path in policy["deny_write_files"]:
        _icacls(path, "/deny", f"{grantee}:{DENY_WRITE_RIGHTS}")


def _inheritable_std_handles(api: _Win32) -> list[int]:
    handles: list[int] = []
    for std in (-10, -11, -12):  # STD_INPUT/OUTPUT/ERROR_HANDLE
        value = int(api.GetStdHandle(std & 0xFFFFFFFF) or 0)
        if value in (0, api.ctypes.c_void_p(-1).value) or value in handles:
            continue
        flags = api.wintypes.DWORD(0)
        if not api.GetHandleInformation(value, api.ctypes.byref(flags)):
            continue
        api.SetHandleInformation(value, 0x1, 0x1)  # HANDLE_FLAG_INHERIT
        handles.append(value)
    return handles


def spawn_in_appcontainer(
    profile_name: str,
    command: Sequence[str],
    *,
    network: bool,
    display: bool,
    environment: dict[str, str] | None = None,
    inherit_std: bool = True,
) -> int:
    """Запускает команду в AppContainer внутри Job Object и ждёт код выхода."""
    api = _win32()
    ctypes, wintypes = api.ctypes, api.wintypes
    container_sid, _ = _appcontainer_sid(api, profile_name)
    capability_sids: list[int] = []
    attribute_list = None
    job = None
    process_info = api.PROCESS_INFORMATION()
    try:
        capability_sids = [api.string_to_sid(value) for value in network_capabilities(network)]
        capabilities = (api.SID_AND_ATTRIBUTES * max(1, len(capability_sids)))()
        for index, sid in enumerate(capability_sids):
            capabilities[index].Sid = sid
            capabilities[index].Attributes = 0x4  # SE_GROUP_ENABLED
        security = api.SECURITY_CAPABILITIES(
            container_sid,
            ctypes.cast(capabilities, ctypes.POINTER(api.SID_AND_ATTRIBUTES)) if capability_sids else None,
            len(capability_sids),
            0,
        )

        handles = _inheritable_std_handles(api) if inherit_std else []
        handle_array = (wintypes.HANDLE * max(1, len(handles)))(*handles)
        attribute_count = 2 if handles else 1
        size = ctypes.c_size_t(0)
        api.InitializeProcThreadAttributeList(None, attribute_count, 0, ctypes.byref(size))
        attribute_list = ctypes.create_string_buffer(size.value)
        if not api.InitializeProcThreadAttributeList(attribute_list, attribute_count, 0, ctypes.byref(size)):
            raise api.error("Не удалось подготовить атрибуты процесса")
        if not api.UpdateProcThreadAttribute(
            attribute_list, 0, 0x00020009, ctypes.byref(security), ctypes.sizeof(security), None, None
        ):  # PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES
            raise api.error("Не удалось назначить AppContainer")
        if handles and not api.UpdateProcThreadAttribute(
            attribute_list, 0, 0x00020002, handle_array, ctypes.sizeof(wintypes.HANDLE) * len(handles), None, None
        ):  # PROC_THREAD_ATTRIBUTE_HANDLE_LIST: наследуются только std-пайпы
            raise api.error("Не удалось ограничить наследуемые дескрипторы")

        startup = api.STARTUPINFOEXW()
        startup.StartupInfo.cb = ctypes.sizeof(api.STARTUPINFOEXW)
        startup.lpAttributeList = ctypes.addressof(attribute_list)
        if handles:
            standard = [int(api.GetStdHandle(std & 0xFFFFFFFF) or 0) for std in (-10, -11, -12)]
            startup.StartupInfo.dwFlags = 0x100  # STARTF_USESTDHANDLES
            startup.StartupInfo.hStdInput = standard[0] if standard[0] in handles else None
            startup.StartupInfo.hStdOutput = standard[1] if standard[1] in handles else None
            startup.StartupInfo.hStdError = standard[2] if standard[2] in handles else None

        job = api.CreateJobObjectW(None, None)
        if not job:
            raise api.error("Не удалось создать Job Object")
        limits = api.JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        # KILL_ON_JOB_CLOSE | DIE_ON_UNHANDLED_EXCEPTION | ACTIVE_PROCESS
        limits.BasicLimitInformation.LimitFlags = 0x2000 | 0x400 | 0x8
        limits.BasicLimitInformation.ActiveProcessLimit = 1
        if not api.SetInformationJobObject(job, 9, ctypes.byref(limits), ctypes.sizeof(limits)):
            raise api.error("Не удалось ограничить Job Object")
        ui = api.JOBOBJECT_BASIC_UI_RESTRICTIONS(ui_restrictions(display))
        if not api.SetInformationJobObject(job, 4, ctypes.byref(ui), ctypes.sizeof(ui)):
            raise api.error("Не удалось применить UI-ограничения")

        command_line = ctypes.create_unicode_buffer(subprocess.list2cmdline(list(command)))
        block = ctypes.create_unicode_buffer(environment_block(dict(os.environ if environment is None else environment)))
        # EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT
        # | DETACHED_PROCESS: без консоли нет conhost.exe, который съел бы лимит процессов.
        flags = 0x00080000 | 0x00000004 | 0x00000400 | 0x00000008
        if not api.CreateProcessW(
            None, command_line, None, None, bool(handles), flags, block, None,
            ctypes.byref(startup), ctypes.byref(process_info),
        ):
            code = ctypes.get_last_error()
            if code == 5:
                raise WindowsSandboxError(
                    "Java недоступна из песочницы (нет доступа). Выберите Java в папке пользователя "
                    "или Mojang runtime либо используйте профиль off для доверенной сборки."
                )
            raise api.error("Не удалось запустить процесс в AppContainer")
        if not api.AssignProcessToJobObject(job, process_info.hProcess):
            api.TerminateProcess(process_info.hProcess, 1)
            raise api.error("Не удалось поместить процесс в Job Object")
        api.ResumeThread(process_info.hThread)
        api.WaitForSingleObject(process_info.hProcess, 0xFFFFFFFF)
        exit_code = wintypes.DWORD(1)
        api.GetExitCodeProcess(process_info.hProcess, ctypes.byref(exit_code))
        return int(exit_code.value)
    finally:
        if process_info.hThread:
            api.CloseHandle(process_info.hThread)
        if process_info.hProcess:
            api.CloseHandle(process_info.hProcess)
        if attribute_list is not None:
            api.DeleteProcThreadAttributeList(attribute_list)
        # Закрытие job убивает всё, что могло остаться в песочнице.
        if job:
            api.CloseHandle(job)
        for sid in capability_sids:
            api.LocalFree(sid)
        api.FreeSid(container_sid)


def appcontainer_sid_string(profile_name: str) -> str:
    api = _win32()
    sid, _ = _appcontainer_sid(api, profile_name)
    try:
        return api.sid_to_string(sid)
    finally:
        api.FreeSid(sid)


def probe() -> tuple[bool, str]:
    """Проверяет, что AppContainer реально запускает процесс на этой системе."""
    if not appcontainer_supported():
        return False, "AppContainer недоступен: нужна Windows 8 или новее"
    system_root = os.environ.get("SystemRoot") or r"C:\Windows"
    cmd = str(Path(system_root) / "System32" / "cmd.exe")
    try:
        # Без std-дескрипторов: у бэкенда stdout — JSON-протокол с оболочкой.
        code = spawn_in_appcontainer(
            PROBE_PROFILE, [cmd, "/d", "/c", "exit 0"], network=False, display=False, inherit_std=False
        )
    except (WindowsSandboxError, OSError) as exc:
        return False, str(exc)[:256]
    if code != 0:
        return False, f"пробный процесс в AppContainer завершился с кодом {code}"
    return True, ""


def run(policy: dict[str, Any], command: Sequence[str]) -> int:
    if not command:
        raise WindowsSandboxError("Не передана команда для песочницы")
    sid = appcontainer_sid_string(policy["profile_name"])
    apply_acl(policy, sid)
    return spawn_in_appcontainer(
        policy["profile_name"],
        command,
        network=policy["network"],
        display=policy["display"],
    )


def _report(message: str) -> None:
    # Собранный бэкенд — windowed-exe: sys.stderr там None или NullWriter,
    # хотя OS-дескриптор stderr (пайп лога игры) у процесса есть. На Windows
    # пишем прямо в него.
    if sys.platform != "win32":
        try:
            sys.stderr.write(message)
            sys.stderr.flush()
        except (AttributeError, OSError, ValueError):
            pass
        return
    try:
        import ctypes
        import msvcrt

        get_std_handle = ctypes.WinDLL("kernel32").GetStdHandle
        get_std_handle.restype = ctypes.c_void_p
        get_std_handle.argtypes = (ctypes.c_uint32,)
        handle = get_std_handle(0xFFFFFFF4)  # STD_ERROR_HANDLE
        if handle and handle != ctypes.c_void_p(-1).value:
            fd = msvcrt.open_osfhandle(handle, os.O_WRONLY)
            os.write(fd, message.encode("utf-8", "replace"))
    except (AttributeError, OSError, ValueError):
        pass


def main(argv: Sequence[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if len(arguments) < 3 or arguments[1] != "--":
        _report("usage: --win-sandbox <policy> -- <command...>\n")
        return 2
    try:
        policy = decode_policy(arguments[0])
        code = run(policy, arguments[2:])
    except (WindowsSandboxError, OSError, subprocess.SubprocessError) as exc:
        _report(f"Песочница Windows: {exc}\n")
        return 1
    if sys.platform == "win32":
        # ExitProcess сохраняет полный 32-битный код (например, 0xC0000005).
        import ctypes

        try:
            sys.stdout.flush()
        except (AttributeError, OSError):
            pass
        ctypes.WinDLL("kernel32").ExitProcess(ctypes.c_uint(code & 0xFFFFFFFF))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
