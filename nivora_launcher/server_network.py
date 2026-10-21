"""opt-in publication of Java Minecraft servers with a public IPv4."""
from __future__ import annotations

import atexit
import hashlib
import ipaddress
import re
import socket
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .errors import LauncherError
from .net import HttpClient
from .upnp import Gateway, UpnpError, discover_gateway, source_ip

LEASE_SECONDS = 600
RENEW_SECONDS = 240

def port_number(value: Any) -> int:
    if isinstance(value, bool) or not re.fullmatch(r"[0-9]{1,5}", str(value)):
        raise LauncherError("Порт должен быть целым числом от 1 до 65535")
    port = int(value)
    if not 1 <= port <= 65535:
        raise LauncherError("Порт должен быть от 1 до 65535")
    return port

def public_ipv4(value: str) -> bool:
    try:
        address = ipaddress.IPv4Address(value)
        return (
            address.is_global
            and not address.is_multicast
            and not address.is_reserved
            and address not in ipaddress.ip_network("192.0.0.0/24")
        )
    except ValueError:
        return False

def nat_kind(wan: str, observed: str, local: str) -> str:
    if public_ipv4(local) and (not observed or local == observed):
        return "direct"
    if wan:
        try:
            address = ipaddress.IPv4Address(wan)
        except ValueError:
            return "unknown"
        if address in ipaddress.ip_network("100.64.0.0/10"):
            return "cgnat"
        if not public_ipv4(wan):
            return "private"
        if observed and observed != wan:
            return "double_nat"
        return "public"
    return "unknown"

def properties_info(text: str, configured_port: int) -> dict[str, Any]:
    properties: dict[str, str] = {}
    for line in text.splitlines():
        if line.lstrip().startswith(("#", "!")):
            continue
        match = re.match(r"^\s*([\w-]+)(?:\s*[=:]\s*|\s+)(.*?)\s*$", line)
        if match:
            properties[match[1]] = match[2]
    bind = properties.get("server-ip", "")
    return {
        "bind_address": bind,
        "bind_all": bind in {"", "0.0.0.0", "::"},
        "online_mode": properties.get("online-mode", "true").lower() != "false",
        "whitelist": properties.get("white-list", "false").lower() == "true",
        "properties_port": properties.get("server-port", str(configured_port)),
    }

def observed_ipv4() -> str:
    # a single fixed HTTPS IP-echo endpoint, no player/account data or tokens.
    raw = HttpClient(timeout=5, retries=1).request("https://api.ipify.org", max_bytes=64)
    value = raw.decode("ascii").strip()
    if not public_ipv4(value):
        raise LauncherError("Сервис определения IP не вернул публичный IPv4")
    return value

def tcp_local(ip: str, port: int) -> bool:
    if not ip:
        return False
    try:
        with socket.create_connection((ip, port), timeout=0.7):
            return True
    except OSError:
        return False

def firewall_help(port: int) -> list[dict[str, str]]:
    # display only. Never run these commands or disable a firewall.
    return [
        {"platform": "Linux · UFW", "command": f"sudo ufw allow {port}/tcp", "undo": f"sudo ufw delete allow {port}/tcp"},
        {"platform": "Linux · firewalld", "command": f"sudo firewall-cmd --add-port={port}/tcp", "undo": f"sudo firewall-cmd --remove-port={port}/tcp"},
        {"platform": "Windows · PowerShell (администратор)",
         "command": f'New-NetFirewallRule -DisplayName "Nivora Launcher TCP {port}" -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port}',
         "undo": f'Remove-NetFirewallRule -DisplayName "Nivora Launcher TCP {port}"'},
    ]

@dataclass
class Publication:
    gateway: Gateway
    root: Path
    internal_port: int
    external_port: int
    description: str
    public_ip: str
    expires_at: float
    renew_at: float
    running: Callable[[], bool]
    error: str = ""
    closing: bool = False
    may_exist: bool = False
    uncertain_lease: bool = False

class ServerPublisher:
    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._publications: dict[str, Publication] = {}
        self._shutdown = threading.Event()
        self._thread: threading.Thread | None = None

    @staticmethod
    def _key(root: Path) -> str:
        return str(root.resolve())

    @classmethod
    def _description(cls, root: Path) -> str:
        return "TrialLauncher-" + hashlib.sha256(cls._key(root).encode()).hexdigest()[:16]

    def state(self, root: Path) -> dict[str, Any]:
        with self._lock:
            value = self._publications.get(self._key(root))
            if value is None:
                return {"active": False, "error": ""}
            return {"active": not value.closing and value.expires_at > time.time(), "method": "upnp",
                    "internal_port": value.internal_port, "external_port": value.external_port,
                    "local_ip": value.gateway.local_ip, "public_ip": value.public_ip,
                    "address": f"{value.public_ip}:{value.external_port}", "expires_at": value.expires_at,
                    "lease_seconds": LEASE_SECONDS, "error": value.error, "closing": value.closing,
                    "uncertain_lease": value.uncertain_lease}

    @staticmethod
    def _owns(value: dict[str, str], publication: Publication) -> bool:
        return (value.get("NewInternalClient") == publication.gateway.local_ip
                and value.get("NewInternalPort") == str(publication.internal_port)
                and value.get("NewPortMappingDescription") == publication.description)

    @staticmethod
    def _lease(value: dict[str, str]) -> int:
        try:
            lease = int(value.get("NewLeaseDuration", ""))
        except ValueError as exc:
            raise UpnpError("Роутер не подтвердил срок UPnP-правила") from exc
        if not 60 <= lease <= LEASE_SECONDS:
            raise UpnpError("Роутер не поддерживает безопасное временное правило. Используйте ручной проброс")
        return lease

    def _map(self, publication: Publication) -> None:
        existing = publication.gateway.mapping(publication.external_port)
        if existing is not None and not self._owns(existing, publication):
            raise UpnpError("Внешний порт уже занят. Выберите другой порт; чужие правила не заменяются")
        # keep a cleanup record even if the router times out after applying AddPortMapping.
        publication.may_exist = True
        publication.gateway.add(publication.external_port, publication.internal_port, publication.description, LEASE_SECONDS)
        mapped = publication.gateway.mapping(publication.external_port)
        if mapped is None or not self._owns(mapped, publication):
            raise UpnpError("Роутер не подтвердил созданный проброс")
        try:
            lease = self._lease(mapped)
            if mapped.get("NewEnabled") not in {"1", "true"}:
                raise UpnpError("Роутер создал отключённое UPnP-правило")
        except UpnpError:
            publication.uncertain_lease = True
            publication.gateway.delete(publication.external_port)
            publication.may_exist = False
            raise
        publication.expires_at = time.time() + lease
        publication.renew_at = time.time() + min(RENEW_SECONDS, max(1, lease / 2))
        publication.error = ""
        publication.uncertain_lease = False

    def open(self, root: Path, internal_port: int, external_port: int, *, running: Callable[[], bool],
             confirmed: bool, info: dict[str, Any], gateway: Gateway | None = None) -> dict[str, Any]:
        if confirmed is not True:
            raise LauncherError("Подтвердите публикацию игрового TCP-порта в интернете")
        internal_port, external_port = port_number(internal_port), port_number(external_port)
        if not running():
            raise LauncherError("Сначала запустите сервер и дождитесь его готовности")
        if not info.get("bind_all"):
            raise LauncherError("server-ip ограничивает сетевые интерфейсы. Подготовьте server.properties и перезапустите сервер")
        gateway = gateway or discover_gateway()
        wan = gateway.external_ip()
        kind = nat_kind(wan, str(info.get("observed_ip", "")), gateway.local_ip)
        if kind in {"private", "cgnat", "double_nat", "unknown"}:
            raise LauncherError("WAN роутера не совпадает с белым IPv4 или находится за NAT провайдера. Нужен белый IP / проброс на внешнем роутере")
        if not tcp_local(gateway.local_ip, internal_port):
            raise LauncherError("Игровой TCP-порт ещё не отвечает на локальном IP. Дождитесь запуска или проверьте server-ip / firewall")
        key = self._key(root)
        with self._lock:
            previous = self._publications.get(key)
            if previous:
                if previous.external_port != external_port or previous.internal_port != internal_port:
                    raise LauncherError("Сначала закройте текущий проброс перед изменением порта")
                if previous.closing:
                    raise LauncherError("Предыдущий проброс ещё закрывается. Повторите удаление и проверьте его статус")
                if previous.expires_at <= time.time() or previous.error:
                    previous.public_ip = wan
                    try:
                        self._map(previous)
                    except LauncherError as exc:
                        previous.error = str(exc)
                        raise
                return self.state(root)
            if any(value.gateway.peer == gateway.peer and value.external_port == external_port
                   for value in self._publications.values()):
                raise LauncherError("Этот внешний порт уже используется другим сервером лаунчера")
            now = time.time()
            value = Publication(gateway, root, internal_port, external_port, self._description(root), wan,
                                now + LEASE_SECONDS, now, running)
            try:
                self._map(value)
            except LauncherError as exc:
                if value.may_exist:
                    value.closing = True
                    value.error = str(exc)
                    self._publications[key] = value
                    self._start_monitor()
                raise
            self._publications[key] = value
            self._start_monitor()
            return self.state(root)

    def _start_monitor(self) -> None:
        if self._thread is None:
            self._thread = threading.Thread(target=self._maintain, name="server-publication", daemon=True)
            self._thread.start()

    def close(self, root: Path) -> dict[str, Any]:
        key = self._key(root)
        with self._lock:
            value = self._publications.get(key)
            if value is None:
                return self.state(root)
            value.closing = True
            try:
                existing = value.gateway.mapping(value.external_port)
                if existing is not None:
                    if not self._owns(existing, value):
                        # a replacement is no longer ours: leave it alone.
                        self._publications.pop(key, None)
                        raise UpnpError("Правило порта изменено вне лаунчера; чужое правило не удалено")
                    value.gateway.delete(value.external_port)
                self._publications.pop(key, None)
            except LauncherError as exc:
                value.error = str(exc)
                raise
            return self.state(root)

    def tick(self, now: float | None = None) -> None:
        now = time.time() if now is None else now
        with self._lock:
            for key, value in list(self._publications.items()):
                try:
                    if value.closing or not value.running():
                        self.close(value.root)
                    elif now >= value.renew_at:
                        current_wan = value.gateway.external_ip()
                        if not public_ipv4(current_wan):
                            value.closing = True
                            self.close(value.root)
                            continue
                        value.public_ip = current_wan
                        self._map(value)
                except LauncherError as exc:
                    value.error = str(exc)
                    value.renew_at = now + 30
                    if value.uncertain_lease:
                        value.closing = True
                    if not value.may_exist:
                        self._publications.pop(key, None)
                    if now >= value.expires_at and not value.uncertain_lease:
                        self._publications.pop(key, None)

    def _maintain(self) -> None:
        while not self._shutdown.wait(15):
            self.tick()

    def shutdown(self) -> None:
        self._shutdown.set()
        with self._lock:
            roots = [value.root for value in self._publications.values()]
        for root in roots:
            try:
                self.close(root)
            except LauncherError:
                pass  # A hard exit still has the bounded router lease as fallback.

publisher = ServerPublisher()
atexit.register(publisher.shutdown)

def diagnose(root: Path, port: int, properties: str, running: bool, *, external_port: int | None = None) -> dict[str, Any]:
    port = port_number(port)
    external_port = port_number(external_port if external_port is not None else port)
    warnings: list[str] = []
    observed = ""
    try:
        observed = observed_ipv4()
    except (LauncherError, ValueError, UnicodeError):
        warnings.append("Внешний IPv4 не удалось определить через HTTPS. Это не означает отсутствие белого IP")
    local = source_ip()
    gateway = None
    wan = ""
    try:
        gateway = discover_gateway()
        local = gateway.local_ip
        wan = gateway.external_ip()
    except LauncherError as exc:
        warnings.append(str(exc))
    kind = nat_kind(wan, observed, local)
    if kind == "direct":
        public = observed if public_ipv4(observed) else local
    elif kind == "public":
        public = observed if public_ipv4(observed) else wan
    elif kind == "unknown":
        public = observed
    else:
        public = ""
    if kind in {"private", "cgnat", "double_nat"}:
        warnings.append("Есть NAT провайдера или второй роутер. Проброс на одном роутере не откроет сервер в интернет")
    info = properties_info(properties, port)
    if not info["bind_all"]:
        warnings.append("server-ip должен быть пустым или 0.0.0.0 для входящих подключений")
    if not info["online_mode"]:
        warnings.append("online-mode=false: публичный сервер не проверяет учётные записи. Рекомендуется online-mode=true")
    if not info["whitelist"]:
        warnings.append("Рекомендуется включить white-list=true и добавить доверенных игроков командой whitelist add <ник>")
    mapping = publisher.state(root)
    return {**info, "internal_port": port, "external_port": external_port,
            "local_ip": local, "observed_ip": observed, "wan_ip": wan, "public_ip": public,
            "address": f"{public}:{external_port}" if public else "", "nat_kind": kind,
            "upnp_available": gateway is not None, "router_name": gateway.name if gateway else "",
            "router_ip": gateway.peer if gateway else "", "running": running,
            "local_listening": running and tcp_local(local or "127.0.0.1", port),
            "external_verified": False, "mapping": mapping, "warnings": warnings,
            "firewall_help": firewall_help(port),
            "verification_note": "Локальная проверка не доказывает доступность из интернета. Проверьте адрес с другого подключения, например мобильного интернета"}
