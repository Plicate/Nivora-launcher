"""Bounded UPnP IGD client. Router traffic never uses the public HTTP client.

Only IPv4 literals on RFC1918 LANs are accepted, and description/control
endpoints must match the source of the SSDP reply. No redirects or proxies.
"""
from __future__ import annotations

import http.client
import ipaddress
import socket
import time
from dataclasses import dataclass
from typing import Any
from urllib.parse import urljoin, urlsplit
from xml.sax.saxutils import escape

from defusedxml import ElementTree as XML

from .errors import LauncherError

MAX_XML_BYTES = 256 * 1024
SOAP_NS = "http://schemas.xmlsoap.org/soap/envelope/"
SERVICE_TYPES = (
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
)
LAN_NETWORKS = tuple(ipaddress.ip_network(value) for value in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"))


class UpnpError(LauncherError):
    def __init__(self, message: str, code: str = "") -> None:
        self.code = code
        super().__init__(message)


def lan_ipv4(value: str) -> bool:
    try:
        address = ipaddress.IPv4Address(value)
    except ValueError:
        return False
    return any(address in network for network in LAN_NETWORKS)


def router_url(url: str, peer: str) -> tuple[str, int, str]:
    try:
        parsed = urlsplit(url)
        port = parsed.port or 80
    except ValueError as exc:
        raise UpnpError("Некорректный адрес UPnP-роутера") from exc
    if (parsed.scheme != "http" or not lan_ipv4(peer) or parsed.hostname != peer
            or parsed.username is not None or parsed.password is not None
            or parsed.fragment or not 1 <= port <= 65535
            or len(url) > 2048 or any(ord(char) < 32 for char in url)):
        raise UpnpError("UPnP может обращаться только к локальному роутеру, ответившему на SSDP")
    return peer, port, (parsed.path or "/") + (f"?{parsed.query}" if parsed.query else "")


def router_request(url: str, peer: str, *, body: bytes | None = None,
                   headers: dict[str, str] | None = None) -> tuple[int, bytes]:
    host, port, path = router_url(url, peer)
    connection = http.client.HTTPConnection(host, port, timeout=3)
    try:
        connection.request("POST" if body is not None else "GET", path, body=body, headers=headers or {})
        response = connection.getresponse()
        if 300 <= response.status < 400:
            raise UpnpError("UPnP-роутер вернул запрещённое перенаправление")
        raw = response.read(MAX_XML_BYTES + 1)
        if len(raw) > MAX_XML_BYTES:
            raise UpnpError("Ответ UPnP-роутера слишком большой")
        return response.status, raw
    except (OSError, http.client.HTTPException) as exc:
        raise UpnpError("Не удалось связаться с UPnP-роутером") from exc
    finally:
        connection.close()


def parse_xml(raw: bytes):
    if len(raw) > MAX_XML_BYTES:
        raise UpnpError("Ответ UPnP-роутера слишком большой")
    try:
        return XML.fromstring(raw, forbid_dtd=True, forbid_entities=True, forbid_external=True)
    except Exception as exc:
        raise UpnpError("Роутер вернул некорректный или небезопасный XML") from exc


def child_text(element: Any, name: str) -> str:
    for child in element:
        if child.tag.rsplit("}", 1)[-1] == name:
            return (child.text or "").strip()
    return ""


def ssdp_locations(timeout: float = 2.0) -> list[tuple[str, str]]:
    packet = (
        'M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\n'
        'MAN: "ssdp:discover"\r\nMX: 1\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n'
    ).encode("ascii")
    result: list[tuple[str, str]] = []
    deadline = time.monotonic() + min(4.0, max(0.1, timeout))
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP) as sock:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 1)
            sock.sendto(packet, ("239.255.255.250", 1900))
            sock.sendto(packet.replace(b"InternetGatewayDevice:1", b"InternetGatewayDevice:2"), ("239.255.255.250", 1900))
            # Bound both time and packet count, including malformed LAN responses.
            for _ in range(64):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                sock.settimeout(remaining)
                try:
                    raw, sender = sock.recvfrom(8192)
                except socket.timeout:
                    break
                peer = sender[0]
                if not lan_ipv4(peer):
                    continue
                lines = raw.decode("iso-8859-1").split("\r\n")
                if not lines or not lines[0].startswith("HTTP/1.1 200"):
                    continue
                headers = dict(line.split(":", 1) for line in lines[1:] if ":" in line)
                location = next((v.strip() for k, v in headers.items() if k.lower() == "location"), "")
                if not location:
                    continue
                try:
                    router_url(location, peer)
                except UpnpError:
                    continue
                item = (location, peer)
                if item not in result:
                    result.append(item)
                if len(result) >= 4:
                    break
    except OSError as exc:
        raise UpnpError("UPnP-поиск недоступен. Проверьте локальную сеть и firewall") from exc
    return result


@dataclass(frozen=True)
class Gateway:
    peer: str
    control_url: str
    service_type: str
    name: str
    local_ip: str

    def action(self, action: str, values: dict[str, Any] | None = None) -> dict[str, str]:
        # Names are constants chosen by this module, never XML supplied by a user.
        elements = "".join(f"<{key}>{escape(str(value))}</{key}>" for key, value in (values or {}).items())
        body = (f'<?xml version="1.0"?><s:Envelope xmlns:s="{SOAP_NS}" '
                's:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">'
                f'<s:Body><u:{action} xmlns:u="{self.service_type}">{elements}</u:{action}>'
                '</s:Body></s:Envelope>').encode("utf-8")
        status, raw = router_request(self.control_url, self.peer, body=body, headers={
            "Content-Type": 'text/xml; charset="utf-8"',
            "SOAPAction": f'"{self.service_type}#{action}"',
        })
        root = parse_xml(raw)
        code = next(((node.text or "").strip() for node in root.iter() if node.tag.rsplit("}", 1)[-1] == "errorCode"), "")
        if status != 200 or code:
            hints = {"718": "Внешний порт уже занят другим правилом роутера",
                     "725": "Роутер поддерживает только постоянные правила. Используйте ручной проброс",
                     "606": "Роутер запретил UPnP-проброс", "401": "Роутер не поддерживает эту UPnP-команду"}
            raise UpnpError(hints.get(code, f"Ошибка UPnP {code or status}"), code)
        response = next((node for node in root.iter() if node.tag.rsplit("}", 1)[-1] == f"{action}Response"), None)
        if response is None:
            raise UpnpError("Некорректный SOAP-ответ роутера")
        return {node.tag.rsplit("}", 1)[-1]: (node.text or "").strip() for node in response}

    def external_ip(self) -> str:
        value = self.action("GetExternalIPAddress").get("NewExternalIPAddress", "")
        try:
            return str(ipaddress.IPv4Address(value))
        except ValueError as exc:
            raise UpnpError("Роутер не вернул корректный WAN IPv4") from exc

    def mapping(self, port: int) -> dict[str, str] | None:
        try:
            return self.action("GetSpecificPortMappingEntry", {
                "NewRemoteHost": "", "NewExternalPort": port, "NewProtocol": "TCP",
            })
        except UpnpError as exc:
            if exc.code == "714":
                return None
            raise

    def add(self, external_port: int, internal_port: int, description: str, lease: int) -> None:
        self.action("AddPortMapping", {
            "NewRemoteHost": "", "NewExternalPort": external_port, "NewProtocol": "TCP",
            "NewInternalPort": internal_port, "NewInternalClient": self.local_ip,
            "NewEnabled": 1, "NewPortMappingDescription": description, "NewLeaseDuration": lease,
        })

    def delete(self, port: int) -> None:
        try:
            self.action("DeletePortMapping", {"NewRemoteHost": "", "NewExternalPort": port, "NewProtocol": "TCP"})
        except UpnpError as exc:
            if exc.code != "714":
                raise


def source_ip(peer: str = "1.1.1.1") -> str:
    """UDP route lookup only; connect() does not send packets."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.connect((peer, 1900 if lan_ipv4(peer) else 443))
            return str(ipaddress.IPv4Address(sock.getsockname()[0]))
    except (OSError, ValueError):
        return ""


def gateways_from_description(raw: bytes, location: str, peer: str) -> list[Gateway]:
    router_url(location, peer)
    root = parse_xml(raw)
    friendly = next(((node.text or "").strip()[:80] for node in root.iter()
                     if node.tag.rsplit("}", 1)[-1] == "friendlyName"), "UPnP Router")
    base = location
    url_base = child_text(root, "URLBase")
    if url_base:
        try:
            router_url(url_base, peer)
        except UpnpError:
            pass
        else:
            base = url_base
    result = []
    local_ip = source_ip(peer)
    if not lan_ipv4(local_ip):
        return []
    for service_type in SERVICE_TYPES:
        for service in root.iter():
            if service.tag.rsplit("}", 1)[-1] != "service" or child_text(service, "serviceType") != service_type:
                continue
            url = urljoin(base, child_text(service, "controlURL"))
            router_url(url, peer)
            result.append(Gateway(peer, url, service_type, friendly, local_ip))
    return result


def discover_gateway() -> Gateway:
    for location, peer in ssdp_locations():
        try:
            status, raw = router_request(location, peer)
            if status != 200:
                continue
            for gateway in gateways_from_description(raw, location, peer)[:3]:
                try:
                    gateway.external_ip()
                    return gateway
                except UpnpError:
                    continue
        except UpnpError:
            continue
    raise UpnpError("UPnP IGD не найден. Включите UPnP на роутере или настройте проброс вручную")
