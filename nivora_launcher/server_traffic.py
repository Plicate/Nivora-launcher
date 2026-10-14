"""Observed TCP payload counters for sockets owned by one server process tree.

Linux: /proc ownership + INET_DIAG/TCP_INFO — no root, no sniffing, no
firewall changes and no host-wide interface counters.

Windows: GetExtendedTcpTable (OWNER_PID) + per-connection TCP_ESTATS_DATA via
iphlpapi — byte-accurate per socket, no admin rights, no drivers. Collection
must be enabled per connection, so the first sample after a new connection
may read zeros.

Short-lived connections between samples can be missed on both platforms;
totals are explicitly observed, not billing.
"""
from __future__ import annotations

import os
import platform
import socket
import struct
import time
from pathlib import Path
from typing import Any


class TrafficUnavailable(OSError):
    pass


def socket_inodes(pid: int, proc_root: Path = Path("/proc")) -> set[int]:
    pending = [pid]
    seen: set[int] = set()
    result: set[int] = set()
    root_read = False
    denied = False
    while pending and len(seen) < 1024:
        current = pending.pop()
        if current <= 0 or current in seen:
            continue
        seen.add(current)
        root = proc_root / str(current)
        try:
            for fd in (root / "fd").iterdir():
                try:
                    target = os.readlink(fd)
                    if target.startswith("socket:[") and target.endswith("]"):
                        result.add(int(target[8:-1]))
                except (OSError, ValueError):
                    continue
            if current == pid:
                root_read = True
        except PermissionError:
            denied = True
        except OSError:
            pass
        try:
            pending.extend(int(value) for value in (root / "task" / str(current) / "children").read_text().split())
        except (OSError, ValueError):
            pass
    if not root_read:
        raise TrafficUnavailable("Нет доступа к socket FD серверного процесса")
    if denied:
        raise TrafficUnavailable("Нет доступа к сокетам части дерева серверного процесса")
    return result


def parse_diag_message(raw: bytes, inodes: set[int], port: int) -> tuple[str, int, int] | None:
    if len(raw) < 72:
        return None
    inode = struct.unpack_from("=I", raw, 68)[0]
    source_port = struct.unpack_from("!H", raw, 4)[0]
    if inode not in inodes or source_port != port or raw[1] == 10:  # TCP_LISTEN
        return None
    offset = 72
    while offset + 4 <= len(raw):
        length, kind = struct.unpack_from("=HH", raw, offset)
        if length < 4 or offset + length > len(raw):
            raise TrafficUnavailable("Некорректный ответ INET_DIAG")
        if kind == 2:  # INET_DIAG_INFO, stable Linux tcp_info ABI
            info = raw[offset + 4:offset + length]
            if len(info) < 208:
                raise TrafficUnavailable("Ядро не предоставляет TCP_INFO bytes_sent/bytes_received")
            received = struct.unpack_from("=Q", info, 128)[0]
            sent = struct.unpack_from("=Q", info, 200)[0]
            # Cookie survives inode reuse and separates IPv4/IPv6 socket identities.
            key = f"{raw[0]}:{inode}:{raw[44:52].hex()}"
            return key, received, sent
        offset += (length + 3) & ~3
    raise TrafficUnavailable("TCP_INFO для сокета сервера недоступен")


def tcp_counters(pid: int, port: int) -> dict[str, tuple[int, int]]:
    if platform.system() == "Windows":
        return windows_tcp_counters(pid, port)
    if platform.system() != "Linux" or not hasattr(socket, "AF_NETLINK"):
        raise TrafficUnavailable("Метрики TCP серверных сокетов доступны на Linux и Windows")
    inodes = socket_inodes(pid)
    if not inodes:
        return {}
    result: dict[str, tuple[int, int]] = {}
    deadline = time.monotonic() + 0.7
    budget = 4 * 1024 * 1024
    sequence = int(time.monotonic_ns() & 0xffffffff)
    with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 4) as netlink:
        netlink.bind((0, 0))
        for family in (socket.AF_INET, socket.AF_INET6):
            sequence = (sequence + 1) & 0xffffffff
            request = struct.pack("=BBBBI", family, socket.IPPROTO_TCP, 2, 0, 0xffffffff) + b"\0" * 48
            header = struct.pack("=IHHII", 16 + len(request), 20, 0x301, sequence, 0)
            netlink.sendto(header + request, (0, 0))
            done = False
            while not done:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or budget <= 0:
                    raise TrafficUnavailable("Превышен лимит чтения TCP-метрик")
                netlink.settimeout(remaining)
                data, _, flags, peer = netlink.recvmsg(65536)
                budget -= len(data)
                if not data or flags & socket.MSG_TRUNC or peer[0] != 0:
                    raise TrafficUnavailable("Неполный ответ ядра TCP-метрик")
                offset = 0
                while offset + 16 <= len(data):
                    length, kind, message_flags, message_seq, _ = struct.unpack_from("=IHHII", data, offset)
                    if length < 16 or offset + length > len(data):
                        raise TrafficUnavailable("Неполный netlink-пакет")
                    raw = data[offset + 16:offset + length]
                    offset += (length + 3) & ~3
                    if message_seq != sequence:
                        continue
                    if message_flags & 0x10:  # NLM_F_DUMP_INTR
                        raise TrafficUnavailable("TCP-таблица изменилась во время чтения")
                    if kind == 3:  # NLMSG_DONE
                        if len(raw) >= 4 and struct.unpack_from("=i", raw)[0] != 0:
                            raise TrafficUnavailable("Ядро прервало чтение TCP-таблицы")
                        done = True
                        break
                    if kind == 2:  # NLMSG_ERROR
                        code = struct.unpack_from("=i", raw)[0] if len(raw) >= 4 else -1
                        if code:
                            raise TrafficUnavailable(f"INET_DIAG недоступен (errno {abs(code)})")
                    elif kind == 20:
                        value = parse_diag_message(raw, inodes, port)
                        if value is not None:
                            result[value[0]] = (value[1], value[2])
    return result


class ServerTrafficTracker:
    def __init__(self, pid: int, port: int) -> None:
        self.pid, self.port = pid, port
        self.previous: dict[str, tuple[int, int]] = {}
        self.last_at: float | None = None
        self.received = self.sent = 0
        self.cached: dict[str, Any] = self.unavailable("Ожидание первой TCP-выборки")
        self.next_sample = 0.0

    def unavailable(self, reason: str) -> dict[str, Any]:
        return {"network_available": False, "network_source": _traffic_source(),
                "network_scope": "observed-game-tcp", "network_error": reason,
                "network_rx_bps": None, "network_tx_bps": None,
                "network_rx_bytes": self.received, "network_tx_bytes": self.sent}

    def sample(self, *, running: bool = True) -> dict[str, Any]:
        now = time.monotonic()
        if not running:
            value = dict(self.cached)
            value.update(network_rx_bps=0.0 if value["network_available"] else None,
                         network_tx_bps=0.0 if value["network_available"] else None)
            return value
        if now < self.next_sample:
            return dict(self.cached)
        self.next_sample = now + 0.8
        try:
            current = tcp_counters(self.pid, self.port)
            rx = sum(max(0, value[0] - self.previous.get(key, (0, 0))[0]) for key, value in current.items())
            tx = sum(max(0, value[1] - self.previous.get(key, (0, 0))[1]) for key, value in current.items())
            self.received += rx
            self.sent += tx
            elapsed = max(0.001, now - self.last_at) if self.last_at is not None else None
            self.cached = {"network_available": True, "network_source": _traffic_source(),
                           "network_scope": "observed-game-tcp", "network_error": "",
                           "network_rx_bps": round(rx / elapsed, 2) if elapsed else None,
                           "network_tx_bps": round(tx / elapsed, 2) if elapsed else None,
                           "network_rx_bytes": self.received, "network_tx_bytes": self.sent}
            self.previous = current
            self.last_at = now
        except (OSError, ValueError, struct.error) as exc:
            self.cached = self.unavailable(str(exc))
            self.next_sample = now + 5.0
        return dict(self.cached)


def _traffic_source() -> str:
    return "windows-tcp-estats" if platform.system() == "Windows" else "linux-inet-diag-tcp-info"


# ── Windows: per-connection byte counters через iphlpapi ────────────────────
#
# GetExtendedTcpTable отдаёт соединения с PID владельца, GetPerTcpConnectionEstats
# с TcpConnectionEstatsData — накопленные DataBytesIn/Out по каждому соединению.
# Оба вызова документированы и не требуют прав администратора.

_TCP_ESTATS_DATA = 1  # TCP_ESTATS_TYPE.TcpConnectionEstatsData
_MIB_TCP_STATE_ESTAB = 5
_TCP_TABLE_OWNER_PID_ALL = 5
_windows_estats_enabled: set[str] = set()


def windows_pid_tree(pid: int) -> set[int]:
    """Дерево процессов через Toolhelp32-снимок (stdlib-ctypes, без psutil)."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)  # type: ignore[attr-defined]
    class PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.POINTER(wintypes.ULONG)),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", wintypes.LONG),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", wintypes.WCHAR * 260),
        ]

    snapshot = kernel32.CreateToolhelp32Snapshot(0x2, 0)  # TH32CS_SNAPPROCESS
    if snapshot == -1 or snapshot == 0:
        raise TrafficUnavailable(f"CreateToolhelp32Snapshot failed ({ctypes.get_last_error()})")
    parents: dict[int, int] = {}
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        if kernel32.Process32FirstW(snapshot, ctypes.byref(entry)):
            while True:
                parents[int(entry.th32ProcessID)] = int(entry.th32ParentProcessID)
                if not kernel32.Process32NextW(snapshot, ctypes.byref(entry)):
                    break
    finally:
        kernel32.CloseHandle(snapshot)
    children: dict[int, list[int]] = {}
    for child, parent in parents.items():
        children.setdefault(parent, []).append(child)
    tree: set[int] = set()
    pending = [int(pid)]
    while pending:
        current = pending.pop()
        if current <= 0 or current in tree:
            continue
        tree.add(current)
        pending.extend(children.get(current, []))
    return tree


def _parse_owner_pid_table(raw: bytes, pids: set[int], port: int) -> list[tuple[int, ...]]:
    """Чистый разбор MIB_TCPTABLE_OWNER_PID: строки сервера в состоянии ESTAB."""
    if len(raw) < 4:
        raise TrafficUnavailable("Некорректная TCP-таблица Windows")
    count = struct.unpack_from("<I", raw, 0)[0]
    rows: list[tuple[int, ...]] = []
    for index in range(min(count, 65536)):
        offset = 4 + index * 24
        if offset + 24 > len(raw):
            raise TrafficUnavailable("Обрезанная TCP-таблица Windows")
        state, local_addr, local_port, remote_addr, remote_port, owner = struct.unpack_from("<6I", raw, offset)
        if owner not in pids or state != _MIB_TCP_STATE_ESTAB:
            continue
        if socket.ntohs(local_port & 0xFFFF) != port:
            continue
        rows.append((state, local_addr, local_port, remote_addr, remote_port, owner))
    return rows


def windows_row_key(row: tuple[int, ...]) -> str:
    _, local_addr, local_port, remote_addr, remote_port, owner = row
    local_ip = socket.inet_ntoa(struct.pack("<I", local_addr))
    remote_ip = socket.inet_ntoa(struct.pack("<I", remote_addr))
    return f"{owner}:{local_ip}:{socket.ntohs(local_port & 0xFFFF)}>{remote_ip}:{socket.ntohs(remote_port & 0xFFFF)}"


def windows_tcp_counters(pid: int, port: int) -> dict[str, tuple[int, int]]:
    import ctypes
    from ctypes import wintypes

    class MIB_TCPROW_OWNER_PID(ctypes.Structure):
        _fields_ = [(name, wintypes.DWORD) for name in
                    ("State", "LocalAddr", "LocalPort", "RemoteAddr", "RemotePort", "OwningPid")]

    class TCP_ESTATS_DATA_RW_V0(ctypes.Structure):
        _fields_ = [("EnableCollection", wintypes.BOOLEAN)]

    class TCP_ESTATS_DATA_ROD_V0(ctypes.Structure):
        _fields_ = [("DataBytesIn", ctypes.c_uint64), ("DataBytesOut", ctypes.c_uint64)]

    iphlpapi = ctypes.WinDLL("iphlpapi", use_last_error=True)  # type: ignore[attr-defined]
    iphlpapi.GetExtendedTcpTable.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.ULONG), wintypes.BOOL, wintypes.ULONG, wintypes.ULONG, wintypes.ULONG]
    iphlpapi.GetPerTcpConnectionEstats.argtypes = [ctypes.POINTER(MIB_TCPROW_OWNER_PID), wintypes.ULONG, ctypes.c_void_p, wintypes.ULONG, ctypes.c_void_p, wintypes.ULONG, ctypes.c_void_p, wintypes.ULONG]
    iphlpapi.GetPerTcpConnectionEstats.restype = wintypes.ULONG
    iphlpapi.SetPerTcpConnectionEstats.argtypes = [ctypes.POINTER(MIB_TCPROW_OWNER_PID), wintypes.ULONG, ctypes.c_void_p, wintypes.ULONG]
    iphlpapi.SetPerTcpConnectionEstats.restype = wintypes.ULONG

    pids = windows_pid_tree(pid)
    size = wintypes.ULONG(0)
    iphlpapi.GetExtendedTcpTable(None, ctypes.byref(size), False, socket.AF_INET, _TCP_TABLE_OWNER_PID_ALL, 0)
    buffer = ctypes.create_string_buffer(size.value)
    result = iphlpapi.GetExtendedTcpTable(buffer, ctypes.byref(size), False, socket.AF_INET, _TCP_TABLE_OWNER_PID_ALL, 0)
    if result != 0:
        raise TrafficUnavailable(f"GetExtendedTcpTable failed ({result})")
    rows = _parse_owner_pid_table(bytes(buffer), pids, port)

    counters: dict[str, tuple[int, int]] = {}
    for row in rows:
        key = windows_row_key(row)
        row_struct = MIB_TCPROW_OWNER_PID(*row)
        if key not in _windows_estats_enabled:
            # счётчики соединения копятся только после явного включения сбора
            rw = TCP_ESTATS_DATA_RW_V0(True)
            iphlpapi.SetPerTcpConnectionEstats(ctypes.byref(row_struct), _TCP_ESTATS_DATA, ctypes.byref(rw), ctypes.sizeof(rw))
            _windows_estats_enabled.add(key)
        rod = TCP_ESTATS_DATA_ROD_V0()
        if iphlpapi.GetPerTcpConnectionEstats(ctypes.byref(row_struct), _TCP_ESTATS_DATA, None, 0, None, 0, ctypes.byref(rod), ctypes.sizeof(rod)) == 0:
            counters[key] = (int(rod.DataBytesIn), int(rod.DataBytesOut))
    # ушедшие соединения больше не требуют включённого сбора
    if len(_windows_estats_enabled) > 4096:
        _windows_estats_enabled.intersection_update(counters.keys())
    return counters
