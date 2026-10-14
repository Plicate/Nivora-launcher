import socket
import threading
import unittest

from nivora_launcher.net import HttpClient, make_socks5_factory
from nivora_launcher.errors import LauncherError


def _run_fake_proxy(server: socket.socket, script) -> None:
    """Исполняет scripted-диалог SOCKS5 от лица прокси."""
    conn, _ = server.accept()
    try:
        script(conn)
    except OSError:
        pass
    finally:
        conn.close()


class Socks5FactoryTests(unittest.TestCase):
    def setUp(self):
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen(1)
        self.port = self.server.getsockname()[1]

    def tearDown(self):
        self.server.close()

    def _factory(self, username: str = "", password: str = ""):
        credentials = f"{username}:{password}" if username else ""
        return make_socks5_factory(f"socks5://{credentials}@127.0.0.1:{self.port}")

    def test_handshake_without_auth_returns_tunnelled_socket(self):
        def script(conn):
            self.assertEqual(conn.recv(3), b"\x05\x01\x00")
            conn.sendall(b"\x05\x00")
            request = conn.recv(512)
            self.assertEqual(request[0:3], b"\x05\x01\x00")
            self.assertEqual(request[3], 0x03)  # имя резолвит прокси
            name_length = request[4]
            self.assertEqual(request[5:5 + name_length], b"example.com")
            self.assertEqual(int.from_bytes(request[5 + name_length:], "big"), 443)
            conn.sendall(b"\x05\x00\x00\x01" + socket.inet_aton("127.0.0.1") + (443).to_bytes(2, "big"))
            conn.recv(16)  # ждём данные клиента, соединение остаётся открыто

        threading.Thread(target=_run_fake_proxy, args=(self.server, script), daemon=True).start()
        sock = self._factory()("example.com", 443, 5.0)
        try:
            self.assertEqual(sock.getpeername()[1], self.port)
            sock.sendall(b"ping")
        finally:
            sock.close()

    def test_username_password_auth_flow(self):
        def script(conn):
            self.assertEqual(conn.recv(4), b"\x05\x02\x00\x02")
            conn.sendall(b"\x05\x02")
            header = conn.recv(2)
            user = conn.recv(header[1])
            password = conn.recv(conn.recv(1)[0])
            self.assertEqual(user, b"user")
            self.assertEqual(password, b"pass")
            conn.sendall(b"\x01\x00")
            conn.recv(512)
            conn.sendall(b"\x05\x00\x00\x01" + socket.inet_aton("127.0.0.1") + (80).to_bytes(2, "big"))
            conn.recv(16)

        threading.Thread(target=_run_fake_proxy, args=(self.server, script), daemon=True).start()
        sock = self._factory("user", "pass")("example.com", 80, 5.0)
        sock.close()

    def test_rejected_connection_closes_socket_and_raises(self):
        def script(conn):
            conn.recv(3)
            conn.sendall(b"\x05\x00")
            conn.recv(512)
            conn.sendall(b"\x05\x05\x00\x01" + socket.inet_aton("127.0.0.1") + (80).to_bytes(2, "big"))

        threading.Thread(target=_run_fake_proxy, args=(self.server, script), daemon=True).start()
        with self.assertRaises(LauncherError) as context:
            self._factory()("example.com", 80, 5.0)
        self.assertIn("отклонил", str(context.exception))

    def test_client_builds_socks_opener_and_rejects_garbage_url(self):
        client = HttpClient(proxy=f"socks5://127.0.0.1:{self.port}")
        handler_names = [type(handler).__name__ for handler in client.opener.handlers]
        self.assertIn("_SocksHTTPHandler", handler_names)
        self.assertIn("_SocksHTTPSHandler", handler_names)
        with self.assertRaises(LauncherError):
            make_socks5_factory("socks5://")


if __name__ == "__main__":
    unittest.main()
