import ipaddress
import socket

import pytest

from nivora_launcher.errors import LauncherError
from nivora_launcher.net import _PUBLIC_RESERVED_HOSTS, _validate_url


def _answer(*addresses: str):
    """getaddrinfo-подобный ответ с заданными адресами."""
    results = []
    for address in addresses:
        family = socket.AF_INET6 if ":" in address else socket.AF_INET
        sockaddr = (address, 443, 0, 0) if family == socket.AF_INET6 else (address, 443)
        results.append((family, socket.SOCK_STREAM, 6, "", sockaddr))
    return results


@pytest.fixture()
def fake_dns(monkeypatch):
    def install(*addresses: str):
        monkeypatch.setattr(socket, "getaddrinfo", lambda host, port, type=0: _answer(*addresses))

    return install


def test_private_ipv4_literals_are_rejected():
    for url in (
        "https://10.0.0.1/",
        "https://172.16.0.1/",
        "https://192.168.1.1/",
        "https://127.0.0.1/",
        "https://169.254.0.1/",
        "https://0.1.2.3/",
        "https://100.64.1.1/",
        "https://198.18.0.1/",
        "https://192.0.2.1/",
        "https://255.255.255.255/",
    ):
        with pytest.raises(LauncherError):
            _validate_url(url)


def test_public_ipv4_literal_passes_and_pins():
    pinned = _validate_url("https://93.184.216.34/index.html")
    assert pinned == (ipaddress.ip_address("93.184.216.34"),)


def test_ipv6_literals():
    assert _validate_url("https://[2606:4700:4700::1111]/") == (ipaddress.ip_address("2606:4700:4700::1111"),)
    for url in ("https://[::1]/", "https://[fe80::1]/", "https://[fc00::1]/", "https://[2001:db8::1]/"):
        with pytest.raises(LauncherError):
            _validate_url(url)


def test_ipv4_mapped_ipv6_is_checked_as_ipv4():
    for url in ("https://[::ffff:10.0.0.1]/", "https://[::ffff:127.0.0.1]/", "https://[::ffff:169.254.0.1]/"):
        with pytest.raises(LauncherError):
            _validate_url(url)
    # публичный адрес в mapped-форме проходит
    assert _validate_url("https://[::ffff:93.184.216.34]/") == (ipaddress.ip_address("::ffff:93.184.216.34"),)


def test_scheme_and_credentials():
    for url in (
        "http://example.com/",
        "ftp://example.com/",
        "https://user:pass@example.com/",
        "https://localhost/",
        "https://printer.local/",
    ):
        with pytest.raises(LauncherError):
            _validate_url(url)


def test_resolved_hostname_pins_all_addresses(fake_dns):
    fake_dns("93.184.216.34", "2606:4700:4700::1111")
    pinned = _validate_url("https://example.com/")
    assert pinned == (ipaddress.ip_address("93.184.216.34"), ipaddress.ip_address("2606:4700:4700::1111"))


def test_resolved_private_address_is_rejected(fake_dns):
    fake_dns("93.184.216.34", "10.0.0.7")
    with pytest.raises(LauncherError):
        _validate_url("https://example.com/")


def test_resolved_mapped_private_address_is_rejected(fake_dns):
    fake_dns("::ffff:192.168.0.4")
    with pytest.raises(LauncherError):
        _validate_url("https://example.com/")


def test_documentation_range_is_rejected_for_every_host(fake_dns):
    # is_private у CPython перекрывает зарезервированное исключение Mojang:
    # 2001:db8::/32 блокируется независимо от хоста
    fake_dns("2001:db8::abcd")
    with pytest.raises(LauncherError):
        _validate_url("https://piston-meta.mojang.com/")
    with pytest.raises(LauncherError):
        _validate_url("https://not-mojang.example/")


def test_no_dns_lookup_when_disabled(monkeypatch):
    def unexpected(*args, **kwargs):
        raise AssertionError("сеть не должна трогаться")

    monkeypatch.setattr(socket, "getaddrinfo", unexpected)
    assert _validate_url("https://example.com/", resolve_dns=False) == ()


def test_broken_dns_answer_is_rejected(monkeypatch):
    monkeypatch.setattr(socket, "getaddrinfo", lambda host, port, type=0: [("junk",)])
    with pytest.raises(LauncherError):
        _validate_url("https://example.com/")
