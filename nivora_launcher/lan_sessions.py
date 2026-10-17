from __future__ import annotations

import base64
import binascii
import errno
import hashlib
import hmac
import ipaddress
import json
import select
import secrets
import socket
import struct
import threading
import time
import unicodedata
from dataclasses import dataclass, field
from typing import Any


MIN_PASSWORD_LENGTH = 8
MAX_PASSWORD_LENGTH = 256
MAX_ROOM_NAME_LENGTH = 64
MAX_NICKNAME_LENGTH = 32
MAX_MESSAGE_BYTES = 16 * 1024
MAX_PEERS = 64
PBKDF2_ITERATIONS = 200_000
_POLL_INTERVAL = 0.2
# Метка LAN-протокола: смена ломает видимость комнат между версиями лаунчера.
_PROTOCOL_LABEL = b"trial-launcher-lan-room-v1\0"
_LENGTH = struct.Struct("!I")
_LOCAL_TUNNEL_HOST = "127.0.0.1"


class LanSessionError(Exception):
    """Raised when a room operation cannot be completed."""


class AuthenticationError(LanSessionError):
    """Raised when room credentials do not match the active session."""


@dataclass
class _Peer:
    nickname: str
    source_ip: str


@dataclass
class _HostSession:
    room_name: str
    host_nickname: str
    salt: bytes
    verifier: bytes
    control_port: int
    game_port: int
    advertised_host: str
    listener: socket.socket
    stop_event: threading.Event = field(default_factory=threading.Event)
    peers: dict[socket.socket, _Peer] = field(default_factory=dict)
    pending: set[socket.socket] = field(default_factory=set)
    handler_threads: set[threading.Thread] = field(default_factory=set)
    accept_thread: threading.Thread | None = None


@dataclass
class _ClientSession:
    room_name: str
    nickname: str
    control_port: int
    game_port: int
    host_address: str
    host_ip: str
    salt: bytes
    verifier: bytes
    connection: socket.socket
    relay_listener: socket.socket | None = None
    relay_port: int = 0
    relay_stop_event: threading.Event = field(default_factory=threading.Event)
    relay_accept_thread: threading.Thread | None = None
    relay_threads: set[threading.Thread] = field(default_factory=set)
    lock: threading.Lock = field(default_factory=threading.Lock)


class LanSessionManager:
    """Coordinate room membership and an optional TCP Minecraft tunnel.

    Port 0 is accepted only by ``create`` to request an ephemeral coordination
    port; the bound port is returned in the created room's status. Peers join
    over the coordination channel, then receive a local loopback Minecraft
    address that the launcher relays to the host's game port.
    """

    def __init__(
        self,
        *,
        bind_host: str = "auto",
        timeout: float = 5.0,
        max_peers: int = 32,
    ) -> None:
        if not isinstance(bind_host, str) or not bind_host.strip() or _has_control_chars(bind_host):
            raise ValueError("bind_host must be 'auto' or a non-empty host name or IP address")
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 0.1 <= timeout <= 30:
            raise ValueError("timeout must be between 0.1 and 30 seconds")
        if isinstance(max_peers, bool) or not isinstance(max_peers, int) or not 1 <= max_peers <= MAX_PEERS:
            raise ValueError(f"max_peers must be between 1 and {MAX_PEERS}")

        self.bind_host = bind_host.strip()
        self.timeout = float(timeout)
        self.max_peers = max_peers
        self._lock = threading.RLock()
        self._host: _HostSession | None = None
        self._client: _ClientSession | None = None
        self._busy = False

    def create(
        self,
        room_name: str,
        password: str,
        player_nickname: str,
        host_control_port: int,
        minecraft_game_port: int,
    ) -> dict[str, Any]:
        room_name = _validate_room_name(room_name)
        player_nickname = _validate_nickname(player_nickname)
        password = _validate_password(password)
        host_control_port = _validate_port(host_control_port, "host_control_port", allow_zero=True)
        minecraft_game_port = _validate_port(minecraft_game_port, "minecraft_game_port")

        with self._lock:
            if self._host or self._client or self._busy:
                raise LanSessionError("This manager already has an active room operation")
            self._busy = True

        listener: socket.socket | None = None
        host: _HostSession | None = None
        try:
            listener, family = _open_listener(self.bind_host, host_control_port, self.max_peers)
            bound = listener.getsockname()
            actual_port = int(bound[1])
            advertised_host = _advertised_host(str(bound[0]), family)
            salt = secrets.token_bytes(16)
            verifier = _derive_verifier(password, salt)
            host = _HostSession(
                room_name=room_name,
                host_nickname=player_nickname,
                salt=salt,
                verifier=verifier,
                control_port=actual_port,
                game_port=minecraft_game_port,
                advertised_host=advertised_host,
                listener=listener,
            )
            host.accept_thread = threading.Thread(
                target=self._accept_loop,
                args=(host,),
                name="trial-lan-room-accept",
                daemon=True,
            )
            with self._lock:
                self._host = host
                host.accept_thread.start()
                self._busy = False
            return self._decorate_status(self._room_status(host), "host", player_nickname)
        except Exception:
            if host is not None:
                with self._lock:
                    host.stop_event.set()
                    sockets = set(host.pending) | set(host.peers)
                    if self._host is host:
                        self._host = None
                for connection in sockets:
                    _close_socket(connection)
            if listener is not None:
                _close_socket(listener)
            if host is not None and host.accept_thread is not None and host.accept_thread.is_alive():
                host.accept_thread.join(self.timeout)
            with self._lock:
                self._busy = False
            raise

    def join(
        self,
        room_name: str,
        password: str,
        player_nickname: str,
        host_control_port: int,
        minecraft_game_port: int,
        *,
        host_address: str = "127.0.0.1",
    ) -> dict[str, Any]:
        room_name = _validate_room_name(room_name)
        player_nickname = _validate_nickname(player_nickname)
        password = _validate_password(password)
        host_control_port = _validate_port(host_control_port, "host_control_port")
        minecraft_game_port = _validate_port(minecraft_game_port, "minecraft_game_port")
        host_address = _validate_host_address(host_address)
        family, socktype, protocol, sockaddr, host_ip = _resolve_lan_target(host_address, host_control_port)

        with self._lock:
            if self._host or self._client or self._busy:
                raise LanSessionError("This manager already has an active room operation")
            self._busy = True

        connection: socket.socket | None = None
        try:
            connection = socket.socket(family, socktype, protocol)
            connection.settimeout(self.timeout)
            connection.connect(sockaddr)
            _send_message(
                connection,
                {
                    "op": "join",
                    "room_name": room_name,
                    "player_nickname": player_nickname,
                    "host_control_port": host_control_port,
                    "minecraft_game_port": minecraft_game_port,
                },
            )
            challenge = _receive_message(connection)
            if challenge.get("type") == "error":
                raise AuthenticationError("Room authentication failed")
            if challenge.get("type") != "challenge":
                raise LanSessionError("The room service returned an invalid challenge")
            salt = _decode_field(challenge.get("salt"), expected_size=16)
            nonce = _decode_field(challenge.get("nonce"), expected_size=32)
            verifier = _derive_verifier(password, salt)
            proof = _make_proof(
                verifier,
                nonce,
                room_name,
                player_nickname,
                host_control_port,
                minecraft_game_port,
            )
            _send_message(connection, {"op": "authenticate", "proof": base64.b64encode(proof).decode("ascii")})
            joined = _receive_message(connection)
            if joined.get("type") == "error":
                raise AuthenticationError("Room authentication failed")
            if joined.get("type") != "joined" or not isinstance(joined.get("status"), dict):
                raise LanSessionError("The room service returned an invalid join response")

            connection.settimeout(None)
            session = _ClientSession(
                room_name=room_name,
                nickname=player_nickname,
                control_port=host_control_port,
                game_port=minecraft_game_port,
                host_address=host_address,
                host_ip=host_ip,
                salt=salt,
                verifier=verifier,
                connection=connection,
            )
            self._start_client_relay(session)
            with self._lock:
                self._client = session
                self._busy = False
            return self._decorate_status(joined["status"], "peer", player_nickname, session.relay_port)
        except socket.timeout as exc:
            if connection is not None:
                _close_socket(connection)
            raise LanSessionError(
                f"Нет ответа от {host_address}:{host_control_port}. Проверьте, что устройства в одной локальной сети, "
                "комната открыта и порт комнаты разрешён во входящих правилах брандмауэра."
            ) from exc
        except OSError as exc:
            if connection is not None:
                _close_socket(connection)
            endpoint = f"{host_address}:{host_control_port}"
            if exc.errno == errno.ECONNREFUSED:
                message = (
                    f"Хост отказал во входе на порт комнаты {endpoint}. Убедитесь, что комната всё ещё открыта, "
                    "адрес и порт комнаты указаны верно, а TCP-порт разрешён брандмауэром."
                )
            elif exc.errno in {errno.EHOSTUNREACH, errno.ENETUNREACH}:
                message = f"Хост {endpoint} недоступен. Проверьте LAN-адрес и подключение устройств к одной сети."
            else:
                message = f"Не удалось подключиться к комнате на {endpoint}: {exc.strerror or exc}"
            raise LanSessionError(message) from exc
        except EOFError as exc:
            if connection is not None:
                _close_socket(connection)
            raise LanSessionError(
                "Сервис комнаты завершил соединение. Проверьте, что комната всё ещё открыта и порт комнаты верен."
            ) from exc
        except Exception:
            if connection is not None:
                _close_socket(connection)
            raise
        finally:
            with self._lock:
                self._busy = False

    def status(
        self,
        room_name: str,
        password: str,
        player_nickname: str,
        host_control_port: int,
        minecraft_game_port: int,
        *,
        host_address: str | None = None,
    ) -> dict[str, Any]:
        credentials = _validate_credentials(
            room_name,
            password,
            player_nickname,
            host_control_port,
            minecraft_game_port,
        )
        with self._lock:
            host = self._host
            client = self._client

        if host is not None:
            _verify_host_credentials(host, credentials)
            return self._decorate_status(self._room_status(host), "host", host.host_nickname)
        if client is not None:
            _verify_client_credentials(client, credentials, host_address)
            return self._client_request(client, "status")
        raise LanSessionError("There is no active room session")

    def leave(
        self,
        room_name: str,
        password: str,
        player_nickname: str,
        host_control_port: int,
        minecraft_game_port: int,
        *,
        host_address: str | None = None,
    ) -> dict[str, Any]:
        credentials = _validate_credentials(
            room_name,
            password,
            player_nickname,
            host_control_port,
            minecraft_game_port,
        )
        with self._lock:
            host = self._host
            client = self._client

        if host is not None:
            _verify_host_credentials(host, credentials)
            self.stop()
        elif client is not None:
            _verify_client_credentials(client, credentials, host_address)
            try:
                self._client_request(client, "leave")
            except LanSessionError:
                pass
            finally:
                self._drop_client(client)
        else:
            raise LanSessionError("There is no active room session")
        return {"left": True, "room_name": credentials[0]}

    def stop(self) -> None:
        """Close any hosted or joined connection and wait briefly for workers."""
        with self._lock:
            host = self._host
            client = self._client
            self._host = None
            self._client = None
            if host is not None:
                host.stop_event.set()
                sockets = set(host.pending) | set(host.peers)
                threads = [host.accept_thread, *host.handler_threads]
            else:
                sockets = set()
                threads = []

        if host is not None:
            _close_socket(host.listener)
        for connection in sockets:
            _close_socket(connection)
        if client is not None:
            client.relay_stop_event.set()
            if client.relay_listener is not None:
                _close_socket(client.relay_listener)
            _close_socket(client.connection)

        deadline = time.monotonic() + 2.0
        current = threading.current_thread()
        for thread in [*threads, client.relay_accept_thread if client is not None else None, *(client.relay_threads if client is not None else set())]:
            if thread is not None and thread is not current:
                thread.join(max(0.0, deadline - time.monotonic()))

    close = stop

    def __enter__(self) -> LanSessionManager:
        return self

    def __exit__(self, _exception_type: Any, _exception: Any, _traceback: Any) -> None:
        self.stop()

    def _accept_loop(self, host: _HostSession) -> None:
        while not host.stop_event.is_set():
            try:
                connection, address = host.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break

            source_ip = _normalize_ip(str(address[0]))
            if not _is_lan_ip(source_ip):
                _close_socket(connection)
                continue
            with self._lock:
                if (
                    self._host is not host
                    or host.stop_event.is_set()
                    or len(host.peers) + len(host.pending) >= self.max_peers
                ):
                    accepted = False
                else:
                    host.pending.add(connection)
                    accepted = True
            if not accepted:
                _close_socket(connection)
                continue

            thread = threading.Thread(
                target=self._handle_connection,
                args=(host, connection, source_ip),
                name="trial-lan-room-peer",
                daemon=True,
            )
            with self._lock:
                host.handler_threads.add(thread)
            try:
                thread.start()
            except RuntimeError:
                with self._lock:
                    host.pending.discard(connection)
                    host.handler_threads.discard(thread)
                _close_socket(connection)

    def _handle_connection(self, host: _HostSession, connection: socket.socket, source_ip: str) -> None:
        try:
            connection.settimeout(self.timeout)
            request = _receive_message(connection)
            operation = request.get("op")
            if operation == "relay_open":
                self._open_host_relay(host, connection, source_ip, request)
                return
            peer = self._authenticate_peer(host, connection, source_ip, request)
            if peer is None:
                return
            connection.settimeout(None)
            self._serve_peer(host, connection, peer)
        except (OSError, EOFError, LanSessionError, ValueError, TypeError, UnicodeError):
            return
        finally:
            with self._lock:
                host.pending.discard(connection)
                host.peers.pop(connection, None)
                host.handler_threads.discard(threading.current_thread())
            _close_socket(connection)

    def _authenticate_peer(self, host: _HostSession, connection: socket.socket, source_ip: str, request: dict[str, Any]) -> _Peer | None:
        if request.get("op") != "join" or "password" in request:
            _send_rejection(connection)
            return None
        try:
            room_name = _validate_room_name(request.get("room_name"))
            nickname = _validate_nickname(request.get("player_nickname"))
            control_port = _validate_port(request.get("host_control_port"), "host_control_port")
            game_port = _validate_port(request.get("minecraft_game_port"), "minecraft_game_port")
        except (TypeError, ValueError):
            _send_rejection(connection)
            return None

        with self._lock:
            room_matches = (
                self._host is host
                and not host.stop_event.is_set()
                and room_name == host.room_name
                and control_port == host.control_port
                and game_port == host.game_port
            )
        if not room_matches:
            _send_rejection(connection)
            return None

        nonce = secrets.token_bytes(32)
        _send_message(
            connection,
            {
                "type": "challenge",
                "salt": base64.b64encode(host.salt).decode("ascii"),
                "nonce": base64.b64encode(nonce).decode("ascii"),
            },
        )
        authentication = _receive_message(connection)
        if authentication.get("op") != "authenticate" or "password" in authentication:
            _send_rejection(connection)
            return None
        try:
            proof = _decode_field(authentication.get("proof"), expected_size=32)
        except LanSessionError:
            _send_rejection(connection)
            return None
        expected = _make_proof(host.verifier, nonce, room_name, nickname, control_port, game_port)
        if not hmac.compare_digest(proof, expected):
            _send_rejection(connection)
            return None

        peer = _Peer(nickname=nickname, source_ip=source_ip)
        with self._lock:
            duplicate = nickname.casefold() == host.host_nickname.casefold() or any(
                existing.nickname.casefold() == nickname.casefold() for existing in host.peers.values()
            )
            if self._host is not host or host.stop_event.is_set() or duplicate:
                peer = None
            else:
                host.pending.discard(connection)
                host.peers[connection] = peer
        if peer is None:
            _send_rejection(connection)
            return None
        _send_message(connection, {"type": "joined", "status": self._room_status(host)})
        return peer

    def _open_host_relay(
        self,
        host: _HostSession,
        connection: socket.socket,
        source_ip: str,
        request: dict[str, Any],
    ) -> None:
        room_name = _validate_room_name(request.get("room_name"))
        nickname = _validate_nickname(request.get("player_nickname"))
        control_port = _validate_port(request.get("host_control_port"), "host_control_port")
        game_port = _validate_port(request.get("minecraft_game_port"), "minecraft_game_port")

        with self._lock:
            peer_known = any(
                peer.nickname.casefold() == nickname.casefold() and peer.source_ip == source_ip
                for peer in host.peers.values()
            )
            room_matches = (
                self._host is host
                and not host.stop_event.is_set()
                and room_name == host.room_name
                and control_port == host.control_port
                and game_port == host.game_port
                and peer_known
            )
        if not room_matches:
            _send_rejection(connection)
            return

        nonce = secrets.token_bytes(32)
        _send_message(
            connection,
            {
                "type": "challenge",
                "salt": base64.b64encode(host.salt).decode("ascii"),
                "nonce": base64.b64encode(nonce).decode("ascii"),
            },
        )
        authentication = _receive_message(connection)
        if authentication.get("op") != "authenticate" or "password" in authentication:
            _send_rejection(connection)
            return
        proof = _decode_field(authentication.get("proof"), expected_size=32)
        expected = _make_proof(host.verifier, nonce, room_name, nickname, control_port, game_port)
        if not hmac.compare_digest(proof, expected):
            _send_rejection(connection)
            return

        upstream = self._connect_host_game(host)
        try:
            _send_message(connection, {"type": "relay_ready"})
            connection.settimeout(None)
            self._pipe_bidirectional(connection, upstream, host.stop_event)
        finally:
            _close_socket(upstream)

    def _connect_host_game(self, host: _HostSession) -> socket.socket:
        attempts = [(_LOCAL_TUNNEL_HOST, host.game_port)]
        if host.advertised_host not in {_LOCAL_TUNNEL_HOST, "0.0.0.0"}:
            attempts.append((host.advertised_host, host.game_port))
        last_error: OSError | None = None
        for target_host, target_port in attempts:
            try:
                family, socktype, protocol, sockaddr, _ = _resolve_lan_target(target_host, target_port)
                upstream = socket.socket(family, socktype, protocol)
                upstream.settimeout(self.timeout)
                upstream.connect(sockaddr)
                upstream.settimeout(None)
                return upstream
            except OSError as exc:
                last_error = exc
        raise LanSessionError(
            f"Не удалось подключиться к игре хоста на порту {host.game_port}. Убедитесь, что мир открыт по LAN и порт актуален."
        ) from last_error

    def _start_client_relay(self, session: _ClientSession) -> None:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind((_LOCAL_TUNNEL_HOST, 0))
        listener.listen(4)
        listener.settimeout(_POLL_INTERVAL)
        session.relay_listener = listener
        session.relay_port = int(listener.getsockname()[1])
        session.relay_accept_thread = threading.Thread(
            target=self._client_relay_accept_loop,
            args=(session,),
            name="trial-lan-room-relay-accept",
            daemon=True,
        )
        session.relay_accept_thread.start()

    def _client_relay_accept_loop(self, session: _ClientSession) -> None:
        listener = session.relay_listener
        if listener is None:
            return
        while not session.relay_stop_event.is_set():
            try:
                local_connection, _address = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            thread = threading.Thread(
                target=self._handle_client_relay,
                args=(session, local_connection),
                name="trial-lan-room-relay-peer",
                daemon=True,
            )
            session.relay_threads.add(thread)
            try:
                thread.start()
            except RuntimeError:
                session.relay_threads.discard(thread)
                _close_socket(local_connection)

    def _handle_client_relay(self, session: _ClientSession, local_connection: socket.socket) -> None:
        remote_connection: socket.socket | None = None
        try:
            remote_connection = self._connect_remote_relay(session)
            self._pipe_bidirectional(local_connection, remote_connection, session.relay_stop_event)
        except (OSError, EOFError, LanSessionError, ValueError, TypeError, UnicodeError):
            pass
        finally:
            session.relay_threads.discard(threading.current_thread())
            _close_socket(local_connection)
            if remote_connection is not None:
                _close_socket(remote_connection)

    def _connect_remote_relay(self, session: _ClientSession) -> socket.socket:
        family, socktype, protocol, sockaddr, _ = _resolve_lan_target(session.host_address, session.control_port)
        connection = socket.socket(family, socktype, protocol)
        connection.settimeout(self.timeout)
        connection.connect(sockaddr)
        _send_message(
            connection,
            {
                "op": "relay_open",
                "room_name": session.room_name,
                "player_nickname": session.nickname,
                "host_control_port": session.control_port,
                "minecraft_game_port": session.game_port,
            },
        )
        challenge = _receive_message(connection)
        if challenge.get("type") == "error":
            raise AuthenticationError("Room authentication failed")
        if challenge.get("type") != "challenge":
            raise LanSessionError("The room service returned an invalid relay challenge")
        nonce = _decode_field(challenge.get("nonce"), expected_size=32)
        proof = _make_proof(
            session.verifier,
            nonce,
            session.room_name,
            session.nickname,
            session.control_port,
            session.game_port,
        )
        _send_message(connection, {"op": "authenticate", "proof": base64.b64encode(proof).decode("ascii")})
        ready = _receive_message(connection)
        if ready.get("type") != "relay_ready":
            raise LanSessionError("The room service could not open the Minecraft tunnel")
        connection.settimeout(None)
        return connection

    def _pipe_bidirectional(
        self,
        left: socket.socket,
        right: socket.socket,
        stop_event: threading.Event,
    ) -> None:
        sockets = [left, right]
        while not stop_event.is_set():
            readable, _, _ = select.select(sockets, [], [], _POLL_INTERVAL)
            if not readable:
                continue
            for current in readable:
                peer = right if current is left else left
                chunk = current.recv(64 * 1024)
                if not chunk:
                    return
                peer.sendall(chunk)

    def _serve_peer(self, host: _HostSession, connection: socket.socket, peer: _Peer) -> None:
        while not host.stop_event.is_set():
            readable, _, _ = select.select([connection], [], [], _POLL_INTERVAL)
            if not readable:
                continue
            connection.settimeout(self.timeout)
            message = _receive_message(connection)
            connection.settimeout(None)
            if not self._valid_member_request(host, peer, message):
                _send_message(connection, {"type": "error", "error": "invalid_request"})
                return
            if message["op"] == "leave":
                _send_message(connection, {"type": "left", "room_name": host.room_name})
                return
            _send_message(connection, {"type": "status", "status": self._room_status(host)})

    def _valid_member_request(self, host: _HostSession, peer: _Peer, message: dict[str, Any]) -> bool:
        try:
            return (
                message.get("op") in {"status", "leave"}
                and "password" not in message
                and _validate_room_name(message.get("room_name")) == host.room_name
                and _validate_nickname(message.get("player_nickname")) == peer.nickname
                and _validate_port(message.get("host_control_port"), "host_control_port") == host.control_port
                and _validate_port(message.get("minecraft_game_port"), "minecraft_game_port") == host.game_port
            )
        except (TypeError, ValueError):
            return False

    def _client_request(self, client: _ClientSession, operation: str) -> dict[str, Any]:
        message = {
            "op": operation,
            "room_name": client.room_name,
            "player_nickname": client.nickname,
            "host_control_port": client.control_port,
            "minecraft_game_port": client.game_port,
        }
        try:
            with client.lock:
                client.connection.settimeout(self.timeout)
                _send_message(client.connection, message)
                response = _receive_message(client.connection)
                client.connection.settimeout(None)
            expected_type = "status" if operation == "status" else "left"
            if response.get("type") != expected_type:
                raise LanSessionError("The room service rejected the request")
            if operation == "status":
                status = response.get("status")
                if not isinstance(status, dict):
                    raise LanSessionError("The room service returned an invalid status")
                return self._decorate_status(status, "peer", client.nickname, client.relay_port)
            return {"left": True, "room_name": client.room_name}
        except (OSError, EOFError, socket.timeout) as exc:
            self._drop_client(client)
            raise LanSessionError("The room connection is no longer active") from exc
        except LanSessionError:
            self._drop_client(client)
            raise

    def _drop_client(self, client: _ClientSession) -> None:
        with self._lock:
            if self._client is client:
                self._client = None
        client.relay_stop_event.set()
        if client.relay_listener is not None:
            _close_socket(client.relay_listener)
        _close_socket(client.connection)
        deadline = time.monotonic() + 1.0
        current = threading.current_thread()
        for thread in [client.relay_accept_thread, *client.relay_threads]:
            if thread is not None and thread is not current:
                thread.join(max(0.0, deadline - time.monotonic()))

    def _room_status(self, host: _HostSession) -> dict[str, Any]:
        with self._lock:
            if self._host is not host or host.stop_event.is_set():
                raise LanSessionError("The room is no longer active")
            peers = sorted(host.peers.values(), key=lambda item: item.nickname.casefold())
            peer_status = [
                {"nickname": peer.nickname, "source_ip": peer.source_ip}
                for peer in peers
            ]
        members = [
            {"nickname": host.host_nickname, "source_ip": host.advertised_host, "role": "host"},
            *({**peer, "role": "peer"} for peer in peer_status),
        ]
        address = _format_endpoint(host.advertised_host, host.game_port)
        return {
            "room_name": host.room_name,
            "host_nickname": host.host_nickname,
            "host_address": host.advertised_host,
            "host_control_port": host.control_port,
            "minecraft_game_port": host.game_port,
            "host_reachable_minecraft_address": address,
            "minecraft_address": address,
            "alternative_hosts": _lan_ipv4_candidates(host.advertised_host),
            "same_lan_only": True,
            "network_scope": "same_lan_only",
            "coordination_only": True,
            "game_traffic_tunnel": False,
            "members": members,
            "peers": peer_status,
            "players": [member["nickname"] for member in members],
            "player_count": len(members),
        }

    @staticmethod
    def _decorate_status(status: dict[str, Any], role: str, nickname: str, relay_port: int | None = None) -> dict[str, Any]:
        result = dict(status)
        result["local_role"] = role
        result["local_nickname"] = nickname
        if relay_port:
            tunnel_address = _format_endpoint(_LOCAL_TUNNEL_HOST, relay_port)
            result["host_reachable_minecraft_address"] = tunnel_address
            result["minecraft_address"] = tunnel_address
            result["same_lan_only"] = True
            result["network_scope"] = "private_network_tcp_tunnel"
            result["coordination_only"] = False
            result["game_traffic_tunnel"] = True
        return result


def _validate_credentials(
    room_name: str,
    password: str,
    nickname: str,
    control_port: int,
    game_port: int,
) -> tuple[str, str, str, int, int]:
    return (
        _validate_room_name(room_name),
        _validate_password(password),
        _validate_nickname(nickname),
        _validate_port(control_port, "host_control_port"),
        _validate_port(game_port, "minecraft_game_port"),
    )


def _verify_host_credentials(host: _HostSession, credentials: tuple[str, str, str, int, int]) -> None:
    room_name, password, nickname, control_port, game_port = credentials
    if (
        room_name != host.room_name
        or nickname != host.host_nickname
        or control_port != host.control_port
        or game_port != host.game_port
    ):
        raise LanSessionError("The supplied room details do not match the active host session")
    _verify_password(password, host.salt, host.verifier)


def _verify_client_credentials(
    client: _ClientSession,
    credentials: tuple[str, str, str, int, int],
    host_address: str | None,
) -> None:
    room_name, password, nickname, control_port, game_port = credentials
    if (
        room_name != client.room_name
        or nickname != client.nickname
        or control_port != client.control_port
        or game_port != client.game_port
        or (host_address is not None and _validate_host_address(host_address).casefold() != client.host_address.casefold())
    ):
        raise LanSessionError("The supplied room details do not match the active peer session")
    _verify_password(password, client.salt, client.verifier)


def _verify_password(password: str, salt: bytes, verifier: bytes) -> None:
    if not hmac.compare_digest(_derive_verifier(password, salt), verifier):
        raise AuthenticationError("Room authentication failed")


def _validate_room_name(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("room_name must be text")
    if _has_control_chars(value):
        raise ValueError("room_name must not contain control characters")
    name = value.strip()
    if not name or len(name) > MAX_ROOM_NAME_LENGTH:
        raise ValueError(f"room_name must be 1 to {MAX_ROOM_NAME_LENGTH} characters")
    return name


def _validate_nickname(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("player_nickname must be text")
    if _has_control_chars(value):
        raise ValueError("player_nickname must not contain control characters")
    nickname = value.strip()
    if not nickname or len(nickname) > MAX_NICKNAME_LENGTH:
        raise ValueError(f"player_nickname must be 1 to {MAX_NICKNAME_LENGTH} characters")
    return nickname


def _validate_password(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("password must be text")
    if len(value) < MIN_PASSWORD_LENGTH:
        raise ValueError(f"password must be at least {MIN_PASSWORD_LENGTH} characters")
    try:
        encoded = value.encode("utf-8")
    except UnicodeEncodeError as exc:
        raise ValueError("password must be valid UTF-8 text") from exc
    if len(value) > MAX_PASSWORD_LENGTH or len(encoded) > MAX_PASSWORD_LENGTH * 4:
        raise ValueError(f"password must be no more than {MAX_PASSWORD_LENGTH} characters")
    return value


def _validate_port(value: Any, name: str, *, allow_zero: bool = False) -> int:
    minimum = 0 if allow_zero else 1
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= 65535:
        allowed = "0 to 65535" if allow_zero else "1 to 65535"
        raise ValueError(f"{name} must be an integer from {allowed}")
    return value


def _validate_host_address(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("host_address must be text")
    address = value.strip()
    if not address or len(address) > 253 or _has_control_chars(address):
        raise ValueError("host_address must be a valid local host name or IP address")
    return address


def _has_control_chars(value: str) -> bool:
    return any(unicodedata.category(character) in {"Cc", "Cs"} for character in value)


def _derive_verifier(password: str, salt: bytes) -> bytes:
    return hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, PBKDF2_ITERATIONS, dklen=32)


def _make_proof(
    verifier: bytes,
    nonce: bytes,
    room_name: str,
    nickname: str,
    control_port: int,
    game_port: int,
) -> bytes:
    message = b"\0".join(
        (
            _PROTOCOL_LABEL.rstrip(b"\0"),
            nonce,
            room_name.encode("utf-8"),
            nickname.encode("utf-8"),
            str(control_port).encode("ascii"),
            str(game_port).encode("ascii"),
        )
    )
    return hmac.new(verifier, message, hashlib.sha256).digest()


def _open_listener(bind_host: str, port: int, backlog: int) -> tuple[socket.socket, int]:
    if bind_host.strip().lower() == "auto":
        # слушаем все интерфейсы: выбор одного адреса ломал комнаты на машинах
        # с несколькими сетями (Wi-Fi + Ethernet, VPN-туннель) — адрес для
        # участников подбирается отдельно
        bind_host = "0.0.0.0"
    try:
        addresses = socket.getaddrinfo(bind_host, port, type=socket.SOCK_STREAM, flags=socket.AI_PASSIVE)
    except OSError as exc:
        raise LanSessionError("The coordination service bind address is invalid") from exc

    last_error: OSError | None = None
    for family, socktype, protocol, _canonical, sockaddr in addresses:
        listener = socket.socket(family, socktype, protocol)
        try:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            if family == socket.AF_INET6:
                listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
            listener.bind(sockaddr)
            listener.listen(backlog)
            listener.settimeout(_POLL_INTERVAL)
            return listener, family
        except OSError as exc:
            last_error = exc
            _close_socket(listener)
    if last_error is not None:
        raise LanSessionError("Could not bind the local coordination service") from last_error
    raise LanSessionError("The coordination service bind address has no usable IP address")


def _advertised_host(bound_host: str, family: int) -> str:
    try:
        address = ipaddress.ip_address(bound_host.split("%", 1)[0])
    except ValueError:
        address = None
    if address is not None and not address.is_unspecified:
        return bound_host

    probe_targets = {
        socket.AF_INET: ("192.0.2.1", 9),
        socket.AF_INET6: ("2001:db8::1", 9, 0, 0),
    }
    target = probe_targets.get(family)
    if target is not None:
        probe = socket.socket(family, socket.SOCK_DGRAM)
        try:
            probe.connect(target)
            candidate = str(probe.getsockname()[0])
            if _is_lan_ip(candidate) and not ipaddress.ip_address(candidate.split("%", 1)[0]).is_loopback:
                return candidate
        except OSError:
            pass
        finally:
            _close_socket(probe)

    try:
        for result in socket.getaddrinfo(socket.gethostname(), None, family=family, type=socket.SOCK_STREAM):
            candidate = str(result[4][0])
            if _is_lan_ip(candidate) and not ipaddress.ip_address(candidate.split("%", 1)[0]).is_loopback:
                return candidate
    except OSError:
        pass
    return "::1" if family == socket.AF_INET6 else "127.0.0.1"


def _lan_ipv4_candidates(preferred: str) -> list[str]:
    """Все LAN IPv4 машины, кроме выбранного. Домашние подсети (192.168/172.16-31)
    идут раньше 10.x — последняя часто корпоративный VPN, не доходящий до друзей."""
    candidates: set[str] = set()

    def collect(address: str) -> None:
        try:
            value = ipaddress.ip_address(address.split("%", 1)[0])
        except ValueError:
            return
        if value.version != 4 or not _is_lan_ip(str(value)) or value.is_loopback or value.is_link_local:
            return
        candidates.add(str(value))

    try:
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            probe.connect(("192.0.2.1", 9))
            collect(str(probe.getsockname()[0]))
        except OSError:
            pass
        finally:
            _close_socket(probe)
    except OSError:
        pass
    try:
        for result in socket.getaddrinfo(socket.gethostname(), None, family=socket.AF_INET, type=socket.SOCK_STREAM):
            collect(str(result[4][0]))
    except OSError:
        pass

    def sort_key(address: str) -> tuple[int, str]:
        try:
            parsed = ipaddress.ip_address(address)
        except ValueError:
            return (9, address)
        if parsed in ipaddress.ip_network("192.168.0.0/16"):
            return (0, address)
        if parsed in ipaddress.ip_network("172.16.0.0/12"):
            return (1, address)
        return (2, address)

    return sorted((address for address in candidates if address != preferred), key=sort_key)[:4]


def _resolve_lan_target(host_address: str, port: int) -> tuple[int, int, int, Any, str]:
    candidate = host_address[1:-1] if host_address.startswith("[") and host_address.endswith("]") else host_address
    try:
        literal = ipaddress.ip_address(candidate.split("%", 1)[0])
    except ValueError:
        if any(not (character.isalnum() or character in ".-_") for character in candidate):
            raise ValueError("host_address must be a local host name or IP address")
    else:
        if not _is_lan_ip(str(literal)):
            raise LanSessionError("Rooms can only be joined through a local-network address")

    try:
        addresses = socket.getaddrinfo(candidate, port, type=socket.SOCK_STREAM)
    except OSError as exc:
        raise LanSessionError("Could not resolve the room host") from exc
    for family, socktype, protocol, _canonical, sockaddr in addresses:
        ip = _normalize_ip(str(sockaddr[0]))
        if _is_lan_ip(ip):
            return family, socktype, protocol, sockaddr, ip
    raise LanSessionError("Rooms can only be joined through a local-network address")


def _normalize_ip(value: str) -> str:
    host = value.split("%", 1)[0]
    try:
        return str(ipaddress.ip_address(host))
    except ValueError:
        return value


def _is_lan_ip(value: str) -> bool:
    try:
        address = ipaddress.ip_address(value.split("%", 1)[0])
    except ValueError:
        return False
    mapped = getattr(address, "ipv4_mapped", None)
    if mapped is not None:
        address = mapped
    if address.is_unspecified or address.is_multicast or address.is_reserved:
        return False
    return address.is_private or address.is_loopback or address.is_link_local


def _format_endpoint(host: str, port: int) -> str:
    return f"[{host}]:{port}" if ":" in host else f"{host}:{port}"


def _send_message(connection: socket.socket, message: dict[str, Any]) -> None:
    payload = json.dumps(message, ensure_ascii=True, separators=(",", ":")).encode("utf-8")
    if not payload or len(payload) > MAX_MESSAGE_BYTES:
        raise LanSessionError("Room message exceeds the size limit")
    connection.sendall(_LENGTH.pack(len(payload)) + payload)


def _receive_message(connection: socket.socket) -> dict[str, Any]:
    length = _LENGTH.unpack(_receive_exact(connection, _LENGTH.size))[0]
    if not 1 <= length <= MAX_MESSAGE_BYTES:
        raise LanSessionError("Room message has an invalid size")
    try:
        message = json.loads(_receive_exact(connection, length).decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as exc:
        raise LanSessionError("Room message is not valid JSON") from exc
    if not isinstance(message, dict):
        raise LanSessionError("Room message must be a JSON object")
    return message


def _receive_exact(connection: socket.socket, size: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        if not chunk:
            raise EOFError("The room connection closed")
        data.extend(chunk)
    return bytes(data)


def _decode_field(value: Any, *, expected_size: int) -> bytes:
    if not isinstance(value, str):
        raise LanSessionError("The room service returned an invalid challenge")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error) as exc:
        raise LanSessionError("The room service returned an invalid challenge") from exc
    if len(decoded) != expected_size:
        raise LanSessionError("The room service returned an invalid challenge")
    return decoded


def _send_rejection(connection: socket.socket) -> None:
    try:
        _send_message(connection, {"type": "error", "error": "join_rejected"})
    except (OSError, LanSessionError):
        pass


def _close_socket(connection: socket.socket) -> None:
    try:
        connection.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass
    try:
        connection.close()
    except OSError:
        pass
