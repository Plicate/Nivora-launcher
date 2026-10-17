from __future__ import annotations

import hashlib
import http.client
import ipaddress
import json
import socket
import struct
import threading
import time
from pathlib import Path
from typing import Any, Callable
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urljoin, urlsplit
from urllib.request import HTTPHandler, HTTPRedirectHandler, HTTPSHandler, ProxyHandler, Request, build_opener

from .constants import APP_NAME, APP_VERSION
from .errors import IntegrityError, LauncherError
from .operations import CancellationToken

Progress = Callable[[int, int | None], None]
DownloadObserver = Callable[[str, str, Path, int, int | None], None]
RequestObserver = Callable[[str, str, int], None]
MAX_RESPONSE_BYTES = 32 * 1024 * 1024
MAX_DOWNLOAD_BYTES = 512 * 1024 * 1024
_PUBLIC_RESERVED_HOSTS = frozenset({"piston-meta.mojang.com"})
_DNS_PIN_ATTRIBUTE = "_nivora_launcher_dns_pins"
_DEFAULT_RATE_LIMIT_INTERVALS = {
    "api.modrinth.com": 0.35,
    "cdn.modrinth.com": 0.15,
    "api.curseforge.com": 0.35,
    "api.cfwidget.com": 0.25,
    "edge.forgecdn.net": 0.1,
    "mediafilez.forgecdn.net": 0.1,
}
# Batch operations (update checks, modpack imports) reserve many future slots. Without a
# cap, a single interactive search queued behind them - or behind a 429 cooldown push -
# could block for minutes and look like an endless spinner in the UI.
_MAX_RATE_LIMIT_WAIT_SECONDS = 15.0
_RATE_LIMIT_LOCK = threading.Lock()
_RATE_LIMIT_NEXT_ALLOWED_AT: dict[str, float] = {}


class _SafeRedirectHandler(HTTPRedirectHandler):
    def __init__(self, *, resolve_dns: bool = True, validate_redirect: Callable[[str], None] | None = None):
        super().__init__()
        self.resolve_dns = resolve_dns
        self.validate_redirect = validate_redirect

    def redirect_request(self, request, fp, code, msg, headers, newurl):
        target = urljoin(request.full_url, newurl)
        pinned_addresses = _validate_url(target, resolve_dns=self.resolve_dns)
        if self.validate_redirect:
            self.validate_redirect(target)
        redirected = super().redirect_request(request, fp, code, msg, headers, target)
        if redirected is not None:
            setattr(redirected, _DNS_PIN_ATTRIBUTE, pinned_addresses)
            source = urlsplit(request.full_url)
            destination = urlsplit(target)
            source_origin = (source.scheme.lower(), source.hostname, source.port or 443)
            destination_origin = (destination.scheme.lower(), destination.hostname, destination.port or 443)
            if source_origin != destination_origin:
                for header in ("Authorization", "Cookie", "Proxy-Authorization"):
                    redirected.headers.pop(header, None)
        return redirected


class _PinnedHTTPSConnection(http.client.HTTPSConnection):
    def __init__(
        self,
        host: str,
        *,
        pinned_addresses: tuple[ipaddress.IPv4Address | ipaddress.IPv6Address, ...],
        **kwargs: Any,
    ) -> None:
        self.pinned_addresses = tuple(pinned_addresses)
        super().__init__(host, **kwargs)
        self._create_connection = self._connect_to_pinned_address

    def _connect_to_pinned_address(
        self,
        address: tuple[str, int],
        timeout: Any,
        source_address: tuple[str, int] | None,
    ) -> socket.socket:
        _, port = address
        last_error: OSError | None = None
        for pinned in self.pinned_addresses:
            family = socket.AF_INET6 if pinned.version == 6 else socket.AF_INET
            sockaddr = (str(pinned), port, 0, 0) if family == socket.AF_INET6 else (str(pinned), port)
            connection: socket.socket | None = None
            try:
                connection = socket.socket(family, socket.SOCK_STREAM)
                if timeout is not socket._GLOBAL_DEFAULT_TIMEOUT:
                    connection.settimeout(timeout)
                if source_address:
                    connection.bind(source_address)
                connection.connect(sockaddr)
                return connection
            except OSError as exc:
                last_error = exc
                if connection is not None:
                    try:
                        connection.close()
                    except OSError:
                        pass
        if last_error:
            raise last_error
        raise OSError("No vetted DNS addresses are available for this HTTPS request")


class _PinnedHTTPSHandler(HTTPSHandler):
    def https_open(self, request):
        pinned_addresses = getattr(request, _DNS_PIN_ATTRIBUTE, ())
        if not pinned_addresses:
            raise URLError("HTTPS request is missing validated DNS addresses")
        return self.do_open(
            lambda host, **kwargs: _PinnedHTTPSConnection(
                host,
                pinned_addresses=pinned_addresses,
                **kwargs,
            ),
            request,
            context=self._context,
        )


# ── SOCKS5 без внешних зависимостей ─────────────────────────────────────────
#
# Имена всегда резолвит прокси (режим socks5h): локальный DNS-pinning через
# туннель всё равно невозможен, зато приватность сохраняется.

_SOCKS5_TIMEOUT_SECONDS = 30.0


def _socks_read_exact(sock: socket.socket, size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining > 0:
        chunk = sock.recv(remaining)
        if not chunk:
            raise LauncherError("SOCKS5-прокси закрыл соединение")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def make_socks5_factory(proxy_url: str) -> Callable[[str, int, float | None], socket.socket]:
    parsed = urlsplit(proxy_url)
    proxy_host = parsed.hostname or ""
    proxy_port = parsed.port or 1080
    username = parsed.username or ""
    password = parsed.password or ""
    if not proxy_host:
        raise LauncherError("SOCKS5-прокси должен иметь вид socks5://host:port")

    def factory(host: str, port: int, timeout: float | None = None) -> socket.socket:
        deadline = timeout or _SOCKS5_TIMEOUT_SECONDS
        sock = socket.create_connection((proxy_host, proxy_port), timeout=deadline)
        try:
            auth_userpass = bool(username)
            sock.sendall(b"\x05\x02\x00\x02" if auth_userpass else b"\x05\x01\x00")
            greeting = _socks_read_exact(sock, 2)
            if greeting[0] != 5:
                raise LauncherError("Прокси не поддерживает SOCKS5")
            if greeting[1] == 0x02 and auth_userpass:
                user_bytes = username.encode("utf-8")
                password_bytes = password.encode("utf-8")
                sock.sendall(b"\x01" + bytes([len(user_bytes)]) + user_bytes + bytes([len(password_bytes)]) + password_bytes)
                auth_status = _socks_read_exact(sock, 2)
                if auth_status[1] != 0:
                    raise LauncherError("SOCKS5-прокси отклонил учётные данные")
            elif greeting[1] != 0x00:
                raise LauncherError("SOCKS5-прокси требует недоступный метод аутентификации")
            try:
                address = socket.inet_aton(host)
                request = b"\x05\x01\x00\x01" + address + struct.pack("!H", port)
            except OSError:
                name = host.encode("idna") if host.isascii() else host.encode("utf-8")
                if len(name) > 255:
                    raise LauncherError("Слишком длинное имя хоста для SOCKS5")
                request = b"\x05\x01\x00\x03" + bytes([len(name)]) + name + struct.pack("!H", port)
            sock.sendall(request)
            reply = _socks_read_exact(sock, 4)
            if reply[1] != 0:
                reasons = {1: "общая ошибка", 2: "соединение запрещено", 3: "сеть недоступна", 4: "хост недоступен", 5: "соединение отклонено", 6: "TTL истёк", 7: "команда не поддерживается", 8: "тип адреса не поддерживается"}
                raise LauncherError(f"SOCKS5-прокси отклонил соединение: {reasons.get(reply[1], reply[1])}")
            if reply[3] == 0x01:
                _socks_read_exact(sock, 6)
            elif reply[3] == 0x03:
                _socks_read_exact(sock, _socks_read_exact(sock, 1)[0] + 2)
            elif reply[3] == 0x04:
                _socks_read_exact(sock, 18)
            else:
                raise LauncherError("Некорректный ответ SOCKS5-прокси")
            return sock
        except Exception:
            sock.close()
            raise

    return factory


class _SocksHTTPConnection(http.client.HTTPConnection):
    def __init__(self, host, port=None, *, socket_factory=None, **kwargs) -> None:
        super().__init__(host, port, **kwargs)
        self._socks_factory = socket_factory

    def connect(self) -> None:
        self.sock = self._socks_factory(self.host, self.port, self.timeout)


class _SocksHTTPSConnection(http.client.HTTPSConnection):
    def __init__(self, host, port=None, *, socket_factory=None, **kwargs) -> None:
        super().__init__(host, port, **kwargs)
        self._socks_factory = socket_factory

    def connect(self) -> None:
        sock = self._socks_factory(self.host, self.port, self.timeout)
        server_hostname = self.host if isinstance(self.host, str) else None
        self.sock = self._context.wrap_socket(sock, server_hostname=server_hostname)


class _SocksHTTPHandler(HTTPHandler):
    def __init__(self, factory) -> None:
        super().__init__()
        self._factory = factory

    def http_open(self, request):
        return self.do_open(
            lambda host, **kwargs: _SocksHTTPConnection(host, socket_factory=self._factory, **kwargs),
            request,
        )


class _SocksHTTPSHandler(HTTPSHandler):
    def __init__(self, factory, context=None) -> None:
        super().__init__(context=context)
        self._factory = factory

    def https_open(self, request):
        return self.do_open(
            lambda host, **kwargs: _SocksHTTPSConnection(host, socket_factory=self._factory, **kwargs),
            request,
            context=self._context,
        )


class HttpClient:
    def __init__(
        self,
        timeout: int = 30,
        retries: int = 3,
        download_observer: DownloadObserver | None = None,
        request_observer: RequestObserver | None = None,
        proxy: str = "",
        cancellation: CancellationToken | None = None,
    ) -> None:
        self.timeout = max(1, timeout)
        self.retries = max(1, retries)
        self.user_agent = f"{APP_NAME}/{APP_VERSION}"
        self.download_observer = download_observer
        self.request_observer = request_observer
        self.cancellation = cancellation
        proxy = proxy.strip()
        self.proxy = proxy
        self.proxy_scheme = urlsplit(proxy).scheme.lower() if proxy else ""
        self.resolve_dns = not bool(proxy)
        self.opener = self._build_opener()

    def _build_opener(self, redirect_validator: Callable[[str], None] | None = None):
        # Do not inherit HTTP(S)_PROXY from the desktop environment silently.
        if self.proxy_scheme in {"socks5", "socks5h"}:
            # через SOCKS DNS резолвит прокси, поэтому пиннинг адресов не применяется
            factory = make_socks5_factory(self.proxy)
            return build_opener(
                ProxyHandler({}),
                _SocksHTTPHandler(factory),
                _SocksHTTPSHandler(factory),
                _SafeRedirectHandler(resolve_dns=False, validate_redirect=redirect_validator),
            )
        handlers: list[Any] = [
            ProxyHandler({"http": self.proxy, "https": self.proxy}) if self.proxy else ProxyHandler({}),
            _SafeRedirectHandler(resolve_dns=self.resolve_dns, validate_redirect=redirect_validator),
        ]
        if self.resolve_dns:
            handlers.append(_PinnedHTTPSHandler())
        return build_opener(*handlers)

    def _notify_request(self, event: str, url: str, attempt: int) -> None:
        if not self.request_observer:
            return
        try:
            self.request_observer(event, url, attempt)
        except Exception:
            return

    def _open(self, request: Request, *, redirect_validator: Callable[[str], None] | None = None):
        self.check_cancelled()
        if redirect_validator is None:
            return self.opener.open(request, timeout=self.timeout)
        opener = self._build_opener(redirect_validator)
        return opener.open(request, timeout=self.timeout)

    def check_cancelled(self) -> None:
        if self.cancellation:
            self.cancellation.check()

    def wait(self, seconds: float) -> None:
        if self.cancellation:
            self.cancellation.wait(seconds)
        else:
            time.sleep(max(0.0, seconds))

    def _rate_limit_interval(self, url: str) -> float:
        hostname = (urlsplit(url).hostname or "").lower()
        return _DEFAULT_RATE_LIMIT_INTERVALS.get(hostname, 0.0)

    def _rate_limit_before_request(self, url: str) -> None:
        interval = self._rate_limit_interval(url)
        if interval <= 0:
            return
        hostname = (urlsplit(url).hostname or "").lower()
        now = time.monotonic()
        with _RATE_LIMIT_LOCK:
            next_allowed = _RATE_LIMIT_NEXT_ALLOWED_AT.get(hostname, now)
            delay = min(max(0.0, next_allowed - now), _MAX_RATE_LIMIT_WAIT_SECONDS)
            reserved_from = max(now, next_allowed)
            _RATE_LIMIT_NEXT_ALLOWED_AT[hostname] = reserved_from + interval
        if delay > 0:
            self.wait(delay)

    def _rate_limit_after_429(self, url: str, retry_after: float) -> None:
        hostname = (urlsplit(url).hostname or "").lower()
        cooldown_until = time.monotonic() + max(0.0, retry_after)
        with _RATE_LIMIT_LOCK:
            _RATE_LIMIT_NEXT_ALLOWED_AT[hostname] = max(
                _RATE_LIMIT_NEXT_ALLOWED_AT.get(hostname, 0.0),
                cooldown_until,
            )

    def _notify_download(
        self,
        event: str,
        url: str,
        destination: Path,
        current: int = 0,
        total: int | None = None,
    ) -> None:
        if not self.download_observer:
            return
        try:
            self.download_observer(event, url, destination, current, total)
        except Exception:
            # Logging must never make a verified download fail.
            return

    def request(
        self,
        url: str,
        method: str = "GET",
        data: bytes | None = None,
        headers: dict[str, str] | None = None,
        max_bytes: int | None = None,
        redirect_validator: Callable[[str], None] | None = None,
    ) -> bytes:
        pinned_addresses = _validate_url(url, resolve_dns=self.resolve_dns)
        request_headers = {"User-Agent": self.user_agent, "Accept": "application/json"}
        request_headers.update(headers or {})
        last_error: Exception | None = None
        for attempt in range(self.retries):
            self.check_cancelled()
            if attempt:
                # Сервис мог сменить адреса между попытками — резолвим
                # DNS заново, а не долбим в тот же недоступный IP.
                pinned_addresses = _validate_url(url, resolve_dns=self.resolve_dns)
            request = Request(url, data=data, headers=request_headers, method=method)
            setattr(request, _DNS_PIN_ATTRIBUTE, pinned_addresses)
            self._rate_limit_before_request(url)
            self._notify_request("start", url, attempt + 1)
            try:
                with self._open(request, redirect_validator=redirect_validator) as response:
                    if max_bytes is not None:
                        content_length = response.headers.get("Content-Length")
                        try:
                            declared_length = int(content_length) if content_length else None
                        except ValueError:
                            declared_length = None
                        if declared_length is not None and declared_length > max_bytes:
                            raise LauncherError("Ответ сервиса слишком большой")
                        raw = response.read(max_bytes + 1)
                        if len(raw) > max_bytes:
                            raise LauncherError("Ответ сервиса слишком большой")
                        return raw
                    raw = response.read(MAX_RESPONSE_BYTES + 1)
                    if len(raw) > MAX_RESPONSE_BYTES:
                        raise LauncherError("Ответ сервиса слишком большой")
                    return raw
            except HTTPError as exc:
                if exc.code == 429:
                    retry_after = exc.headers.get("Retry-After", "")
                    try:
                        delay = min(60.0, max(1.0, float(retry_after)))
                    except (TypeError, ValueError):
                        delay = 2.0 + attempt
                    if attempt + 1 < self.retries:
                        self._rate_limit_after_429(url, delay)
                        self._notify_request("retry", url, attempt + 1)
                        self.wait(delay)
                        continue
                    raise LauncherError(f"HTTP 429: сервер временно ограничил запросы к {url}. Повторите позже.") from exc
                if exc.code >= 500 and attempt + 1 < self.retries:
                    self._notify_request("retry", url, attempt + 1)
                    self.wait(1.0 + attempt)
                    continue
                raise LauncherError(f"HTTP {exc.code} при запросе {url}") from exc
            except (URLError, TimeoutError, OSError) as exc:
                last_error = exc
                if attempt + 1 < self.retries:
                    self._notify_request("retry", url, attempt + 1)
                    self.wait(1.0 + attempt)
                    continue
                break
        raise LauncherError(f"Сетевая ошибка при запросе {url}") from last_error

    def request_detailed(
        self,
        url: str,
        method: str = "GET",
        data: bytes | None = None,
        headers: dict[str, str] | None = None,
        max_bytes: int | None = None,
        redirect_validator: Callable[[str], None] | None = None,
    ) -> dict[str, Any]:
        pinned_addresses = _validate_url(url, resolve_dns=self.resolve_dns)
        request_headers = {"User-Agent": self.user_agent, "Accept": "*/*"}
        request_headers.update(headers or {})
        request = Request(url, data=data, headers=request_headers, method=method)
        setattr(request, _DNS_PIN_ATTRIBUTE, pinned_addresses)
        last_error: Exception | None = None
        for attempt in range(self.retries):
            self.check_cancelled()
            self._rate_limit_before_request(url)
            self._notify_request("start", url, attempt + 1)
            try:
                with self._open(request, redirect_validator=redirect_validator) as response:
                    limit = max_bytes if max_bytes is not None else MAX_RESPONSE_BYTES
                    content_length = response.headers.get("Content-Length")
                    try:
                        declared_length = int(content_length) if content_length else None
                    except ValueError:
                        declared_length = None
                    if declared_length is not None and declared_length > limit:
                        raise LauncherError("Ответ сервиса слишком большой")
                    raw = response.read(limit + 1)
                    if len(raw) > limit:
                        raise LauncherError("Ответ сервиса слишком большой")
                    return {
                        "status": int(getattr(response, "status", 200) or 200),
                        "headers": dict(response.headers.items()),
                        "url": str(getattr(response, "url", url) or url),
                        "body": raw,
                    }
            except HTTPError as exc:
                if exc.code == 429:
                    retry_after = exc.headers.get("Retry-After", "")
                    try:
                        delay = min(60.0, max(1.0, float(retry_after)))
                    except (TypeError, ValueError):
                        delay = 2.0 + attempt
                    if attempt + 1 < self.retries:
                        self._rate_limit_after_429(url, delay)
                        self._notify_request("retry", url, attempt + 1)
                        self.wait(delay)
                        continue
                    raise LauncherError(f"HTTP 429: сервер временно ограничил запросы к {url}. Повторите позже.") from exc
                if exc.code >= 500 and attempt + 1 < self.retries:
                    self._notify_request("retry", url, attempt + 1)
                    self.wait(1.0 + attempt)
                    continue
                raise LauncherError(f"HTTP {exc.code} при запросе {url}") from exc
            except (URLError, TimeoutError, OSError) as exc:
                last_error = exc
                if attempt + 1 < self.retries:
                    self._notify_request("retry", url, attempt + 1)
                    self.wait(1.0 + attempt)
                    continue
                break
        raise LauncherError(f"Сетевая ошибка при запросе {url}") from last_error

    def get_json(self, url: str, headers: dict[str, str] | None = None) -> Any:
        try:
            return json.loads(self.request(url, headers=headers).decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise LauncherError(f"Сервер вернул некорректный JSON: {url}") from exc

    def post_form(self, url: str, values: dict[str, str]) -> Any:
        body = urlencode(values).encode("utf-8")
        raw = self.request(
            url,
            method="POST",
            data=body,
            headers={
                "Content-Type": "application/x-www-form-urlencoded",
                "Accept": "application/json",
            },
        )
        try:
            return json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise LauncherError(f"Сервер авторизации вернул некорректный ответ: {url}") from exc

    def post_form_soft(self, url: str, values: dict[str, str]) -> tuple[int, Any]:
        """Post a form and return the HTTP status as well as the parsed body.

        OAuth device-code polling intentionally uses HTTP 400 for pending states.
        """
        pinned_addresses = _validate_url(url, resolve_dns=self.resolve_dns)
        body = urlencode(values).encode("utf-8")
        request = Request(
            url,
            data=body,
            headers={
                "User-Agent": self.user_agent,
                "Content-Type": "application/x-www-form-urlencoded",
                "Accept": "application/json",
            },
            method="POST",
        )
        setattr(request, _DNS_PIN_ATTRIBUTE, pinned_addresses)
        last_error: Exception | None = None
        for attempt in range(self.retries):
            self.check_cancelled()
            self._rate_limit_before_request(url)
            self._notify_request("start", url, attempt + 1)
            try:
                with self._open(request) as response:
                    raw = response.read(MAX_RESPONSE_BYTES + 1)
                    if len(raw) > MAX_RESPONSE_BYTES:
                        raise LauncherError("Ответ OAuth слишком большой")
                    status = response.status
                break
            except HTTPError as exc:
                raw = exc.read(MAX_RESPONSE_BYTES + 1)
                if len(raw) > MAX_RESPONSE_BYTES:
                    raw = raw[:MAX_RESPONSE_BYTES]
                status = exc.code
                if status >= 500 and attempt + 1 < self.retries:
                    self.wait(1.0 + attempt)
                    continue
                break
            except (URLError, TimeoutError, OSError) as exc:
                last_error = exc
                if attempt + 1 < self.retries:
                    self.wait(1.0 + attempt)
                    continue
                raise LauncherError("Сетевая ошибка OAuth") from exc
        else:
            raise LauncherError("Сетевая ошибка OAuth") from last_error
        try:
            return status, json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise LauncherError(f"Сервер авторизации вернул некорректный ответ: {url}") from exc

    def post_json(self, url: str, value: dict[str, Any], headers: dict[str, str] | None = None) -> Any:
        body = json.dumps(value).encode("utf-8")
        raw = self.request(
            url,
            method="POST",
            data=body,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json",
                **(headers or {}),
            },
        )
        try:
            return json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise LauncherError(f"Сервер вернул некорректный JSON: {url}") from exc

    def download(
        self,
        url: str,
        destination: Path,
        *,
        sha1: str | None = None,
        sha256: str | None = None,
        sha512: str | None = None,
        size: int | None = None,
        progress: Progress | None = None,
        headers: dict[str, str] | None = None,
        redirect_validator: Callable[[str], None] | None = None,
    ) -> bool:
        pinned_addresses = _validate_url(url, resolve_dns=self.resolve_dns)
        self.check_cancelled()
        if size is not None and size > MAX_DOWNLOAD_BYTES:
            raise LauncherError("Файл загрузки слишком большой")
        _reject_symlink_path(destination)
        destination.parent.mkdir(parents=True, exist_ok=True)
        sidecar = destination.with_name(f".{destination.name}.trial-meta")
        _reject_symlink_path(sidecar)
        self._notify_download("start", url, destination, 0, size)
        if destination.exists():
            trusted = bool(sha1 or sha256 or sha512 or size is not None) and verify_file(destination, sha1=sha1, sha256=sha256, sha512=sha512, size=size)
            if sha256:
                trusted = trusted and file_hash(destination, "sha256").lower() == sha256.lower()
            if trusted:
                actual_size = destination.stat().st_size
                if progress:
                    progress(actual_size, actual_size)
                self._notify_download("cached", url, destination, actual_size, size or actual_size)
                return False

        temporary = destination.with_name(f".{destination.name}.part")
        _reject_symlink_path(temporary)
        last_error: Exception | None = None
        # Оборванные загрузки продолжаются с места обрыва (Range): часть
        # зеркал регулярно рвёт длинные соединения, и без докачки каждая
        # попытка начиналась бы с нуля и упиралась в тот же обрыв.
        integrity_restarts = 0
        for attempt in range(self.retries):
            self.check_cancelled()
            self._notify_request("download", url, attempt + 1)
            offset = _partial_download_offset(temporary)
            try:
                request_headers = {"User-Agent": self.user_agent}
                request_headers.update(headers or {})
                if attempt and offset:
                    # Зеркало могло сменить адреса между попытками —
                    # резолвим DNS заново, а не долбим в тот же IP.
                    pinned_addresses = _validate_url(url, resolve_dns=self.resolve_dns)
                if offset:
                    request_headers["Range"] = f"bytes={offset}-"
                request = Request(url, headers=request_headers)
                setattr(request, _DNS_PIN_ATTRIBUTE, pinned_addresses)
                with self._open(request, redirect_validator=redirect_validator) as response:
                    status = getattr(response, "status", None) or getattr(response, "code", None)
                    remaining = _content_length(response)
                    resumed = bool(offset) and status == 206
                    if offset and not resumed:
                        # Зеркало игнорирует Range — качаем файл целиком.
                        offset = 0
                    if resumed and remaining is not None:
                        total_size = offset + remaining
                    else:
                        total_size = remaining if remaining is not None else size
                    if total_size is not None and total_size > MAX_DOWNLOAD_BYTES:
                        raise LauncherError("Файл загрузки слишком большой")
                    written = offset
                    with temporary.open("ab" if resumed else "wb") as handle:
                        while True:
                            self.check_cancelled()
                            chunk = response.read(1024 * 1024)
                            if not chunk:
                                break
                            if written + len(chunk) > MAX_DOWNLOAD_BYTES:
                                raise LauncherError("Файл загрузки слишком большой")
                            handle.write(chunk)
                            written += len(chunk)
                            if progress:
                                progress(written, total_size)
                            self._notify_download("progress", url, destination, written, total_size)
                    if size is not None and written != size:
                        # Соединение закрылось раньше конца файла: частичный
                        # .part остаётся и следующая попытка докачает его.
                        raise _IncompleteDownloadError(f"Зеркало закрыло соединение на {written} из {size} байт")
                if not verify_file(temporary, sha1=sha1, sha256=sha256, sha512=sha512, size=size):
                    raise IntegrityError(f"Проверка хеша не пройдена: {url}")
                temporary.replace(destination)
                if sidecar.exists():
                    sidecar.unlink()
                self._notify_download("complete", url, destination, written, total_size)
                return True
            except HTTPError as exc:
                last_error = exc
                if exc.code == 429:
                    retry_after = (exc.headers or {}).get("Retry-After", "") if exc.headers else ""
                    try:
                        delay = min(60.0, max(1.0, float(retry_after)))
                    except (TypeError, ValueError):
                        delay = 2.0 + attempt
                    if attempt + 1 < self.retries:
                        self._rate_limit_after_429(url, delay)
                        self._notify_request("retry", url, attempt + 1)
                        self.wait(delay)
                        continue
                    raise LauncherError(f"HTTP 429: сервер временно ограничил загрузки {url}. Повторите позже.") from exc
                if exc.code == 416 and offset:
                    # Зеркало не подтвердило докачку (файл уже заменили или
                    # кеш отдаёт другой контент) — уходим на полную загрузку.
                    _discard_partial(temporary)
                    self._notify_request("retry", url, attempt + 1)
                    self.wait(0.5)
                    continue
                if exc.code >= 500 and attempt + 1 < self.retries:
                    self._notify_request("retry", url, attempt + 1)
                    self.wait(1.0 + attempt)
                    continue
                raise LauncherError(f"HTTP {exc.code} при загрузке {url}") from exc
            except IntegrityError as exc:
                last_error = exc
                # Повреждённая докачка (зеркало подмешало чужие байты)
                # заслуживает одну чистую попытку с нуля.
                _discard_partial(temporary)
                if integrity_restarts >= 1 or attempt + 1 >= self.retries:
                    break
                integrity_restarts += 1
                self._notify_request("retry", url, attempt + 1)
                continue
            except (URLError, TimeoutError, OSError) as exc:
                last_error = exc
                if attempt + 1 >= self.retries:
                    break
                self._notify_request("retry", url, attempt + 1)
                self.wait(1.0 + attempt)
            except LauncherError as exc:
                last_error = exc
                break
        if isinstance(last_error, IntegrityError):
            raise last_error
        if isinstance(last_error, LauncherError):
            raise last_error
        raise LauncherError(f"Не удалось скачать {url}: {last_error}") from last_error


class _IncompleteDownloadError(OSError):
    """Зеркало закрыло соединение до конца файла (обрыв посреди загрузки)."""


def _partial_download_offset(temporary: Path) -> int:
    """Размер недокачанного .part для докачки, 0 — качать с нуля."""
    if not temporary.exists():
        return 0
    if temporary.is_symlink():
        raise LauncherError(f"Небезопасный временный файл загрузки: {temporary}")
    if not temporary.is_file():
        raise LauncherError(f"Временный файл загрузки не является файлом: {temporary}")
    try:
        partial_size = temporary.stat().st_size
    except OSError:
        return 0
    if partial_size <= 0 or partial_size > MAX_DOWNLOAD_BYTES:
        _discard_partial(temporary)
        return 0
    return partial_size


def _discard_partial(temporary: Path) -> None:
    try:
        temporary.unlink(missing_ok=True)
    except OSError:
        pass


def _content_length(response: Any) -> int | None:
    value = response.headers.get("Content-Length")
    if value and str(value).isdigit():
        return int(value)
    return None


def file_hash(path: Path, algorithm: str) -> str:
    digest = hashlib.new(algorithm)
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_file(
    path: Path,
    *,
    sha1: str | None = None,
    sha256: str | None = None,
    sha512: str | None = None,
    size: int | None = None,
) -> bool:
    if not path.exists() or not path.is_file():
        return False
    if size is not None and path.stat().st_size != size:
        return False
    if sha1 and file_hash(path, "sha1").lower() != sha1.lower():
        return False
    if sha256 and file_hash(path, "sha256").lower() != sha256.lower():
        return False
    if sha512 and file_hash(path, "sha512").lower() != sha512.lower():
        return False
    return True


def _verify_sidecar(destination: Path, sidecar: Path) -> bool:
    if sidecar.is_symlink() or not sidecar.exists():
        return False
    try:
        metadata = json.loads(sidecar.read_text(encoding="utf-8"))
        return verify_file(destination, sha512=metadata.get("sha512"), size=metadata.get("size"))
    except (OSError, ValueError, TypeError):
        return False


def _validate_url(
    url: str,
    *,
    resolve_dns: bool = True,
) -> tuple[ipaddress.IPv4Address | ipaddress.IPv6Address, ...]:
    try:
        parsed = urlsplit(url)
        host = (parsed.hostname or "").lower().rstrip(".")
    except ValueError as exc:
        raise LauncherError(f"Некорректный сетевой адрес: {url}") from exc
    if parsed.scheme.lower() != "https" or not host or parsed.username or parsed.password:
        raise LauncherError(f"Разрешены только безопасные HTTPS-адреса: {url}")
    if host in {"localhost", "localhost.localdomain"} or host.endswith(".local"):
        raise LauncherError(f"Сетевой запрос к localhost запрещён: {host}")
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        if not resolve_dns:
            return ()
        try:
            port = parsed.port or 443
            results = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Не удалось проверить сетевой адрес: {host}") from exc
        if not results:
            raise LauncherError(f"Не удалось проверить сетевой адрес: DNS не вернул адреса для {host}")
        addresses = []
        seen: set[ipaddress.IPv4Address | ipaddress.IPv6Address] = set()
        try:
            for result in results:
                resolved = ipaddress.ip_address(str(result[4][0]).split("%", 1)[0])
                if resolved not in seen:
                    seen.add(resolved)
                    addresses.append(resolved)
        except (IndexError, TypeError, ValueError) as exc:
            raise LauncherError(f"Не удалось проверить сетевой адрес: некорректный ответ DNS для {host}") from exc
    else:
        addresses = [address]
    for address in addresses:
        mapped = getattr(address, "ipv4_mapped", None)
        checked = mapped or address
        # Python's address tables can mark a publicly routable IPv6 allocation
        # as reserved. Mojang's manifest currently resolves to such an address.
        # Keep the exception hostname-scoped; private, loopback and link-local
        # addresses remain blocked for every host.
        public_reserved = checked.version == 6 and checked.is_reserved and host in _PUBLIC_RESERVED_HOSTS
        non_global = not checked.is_global and not public_reserved
        if non_global or checked.is_private or checked.is_loopback or checked.is_link_local or checked.is_unspecified or checked.is_multicast:
            raise LauncherError(f"Сетевой запрос к непубличному адресу запрещён: {host} ({checked})")
    return tuple(addresses)


def _reject_symlink_path(path: Path) -> None:
    current = path
    while current != current.parent:
        if current.is_symlink():
            raise LauncherError(f"Небезопасный путь загрузки: {path}")
        current = current.parent
