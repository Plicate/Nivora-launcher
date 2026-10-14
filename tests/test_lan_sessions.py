from __future__ import annotations

import time
import ipaddress
import socket
import threading
import unittest

from nivora_launcher.lan_sessions import AuthenticationError, LanSessionError, LanSessionManager


class LanSessionTests(unittest.TestCase):
    def test_default_host_binds_all_interfaces_and_advertises_specific_lan_address(self) -> None:
        host = LanSessionManager()
        try:
            created = host.create("Scoped room", "room-secret", "Host", 0, 25569)
            bound_host = host._host.listener.getsockname()[0]

            # авто-режим слушает все интерфейсы: иначе на машинах с Wi-Fi + VPN
            # комнату не видно соседям, а адрес уходит в туннель
            self.assertTrue(ipaddress.ip_address(bound_host.split("%", 1)[0]).is_unspecified)
            advertised = ipaddress.ip_address(created["host_address"].split("%", 1)[0])
            self.assertFalse(advertised.is_unspecified)
            self.assertIsInstance(created.get("alternative_hosts"), list)
        finally:
            host.stop()

    def test_authentication_status_and_peer_leave(self) -> None:
        host = LanSessionManager(bind_host="127.0.0.1")
        peer = LanSessionManager(bind_host="127.0.0.1")
        try:
            created = host.create("Friday LAN", "correct horse", "Host", 0, 25565)
            joined = peer.join(
                "Friday LAN",
                "correct horse",
                "Guest",
                created["host_control_port"],
                25565,
                host_address="127.0.0.1",
            )

            self.assertTrue(joined["same_lan_only"])
            self.assertFalse(joined["coordination_only"])
            self.assertTrue(joined["game_traffic_tunnel"])
            self.assertEqual(joined["network_scope"], "private_network_tcp_tunnel")
            self.assertTrue(joined["host_reachable_minecraft_address"].startswith("127.0.0.1:"))
            self.assertNotEqual(joined["host_reachable_minecraft_address"], "127.0.0.1:25565")
            self.assertEqual(joined["local_role"], "peer")

            host_status = host.status(
                "Friday LAN", "correct horse", "Host", created["host_control_port"], 25565
            )
            self.assertEqual(host_status["player_count"], 2)
            self.assertEqual(host_status["peers"], [{"nickname": "Guest", "source_ip": "127.0.0.1"}])

            peer_status = peer.status(
                "Friday LAN", "correct horse", "Guest", created["host_control_port"], 25565
            )
            self.assertEqual(peer_status["players"], ["Host", "Guest"])
            self.assertEqual(peer_status["local_role"], "peer")

            left = peer.leave("Friday LAN", "correct horse", "Guest", created["host_control_port"], 25565)
            self.assertTrue(left["left"])
            self.assertTrue(
                _wait_for(
                    lambda: not host.status(
                        "Friday LAN", "correct horse", "Host", created["host_control_port"], 25565
                    )["peers"]
                )
            )
        finally:
            peer.stop()
            host.stop()

    def test_game_tcp_bytes_reach_the_host_through_the_local_tunnel(self) -> None:
        game_listener = socket.socket()
        game_listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        game_listener.bind(("127.0.0.1", 0))
        game_listener.listen(1)
        game_listener.settimeout(3)
        game_port = int(game_listener.getsockname()[1])
        received: list[bytes] = []

        def echo_game_data() -> None:
            try:
                connection, _address = game_listener.accept()
                with connection:
                    data = connection.recv(4096)
                    received.append(data)
                    connection.sendall(data)
            except OSError:
                return

        game_thread = threading.Thread(target=echo_game_data, daemon=True)
        host = LanSessionManager(bind_host="127.0.0.1")
        peer = LanSessionManager(bind_host="127.0.0.1")
        game_thread.start()
        try:
            created = host.create("TCP tunnel", "room-secret", "Host", 0, game_port)
            joined = peer.join(
                "TCP tunnel",
                "room-secret",
                "Guest",
                created["host_control_port"],
                game_port,
                host_address="127.0.0.1",
            )
            relay_port = int(joined["host_reachable_minecraft_address"].rsplit(":", 1)[1])
            payload = b"minecraft-tcp-probe"
            with socket.create_connection(("127.0.0.1", relay_port), timeout=3) as game_client:
                game_client.sendall(payload)
                response = bytearray()
                while len(response) < len(payload):
                    chunk = game_client.recv(len(payload) - len(response))
                    if not chunk:
                        break
                    response.extend(chunk)

            self.assertEqual(bytes(response), payload)
            self.assertEqual(received, [payload])
        finally:
            peer.stop()
            host.stop()
            game_listener.close()
            game_thread.join(timeout=1)

    def test_authentication_failure_does_not_add_a_peer(self) -> None:
        host = LanSessionManager(bind_host="127.0.0.1")
        peer = LanSessionManager(bind_host="127.0.0.1")
        try:
            created = host.create("Private room", "room-secret", "Host", 0, 25566)
            with self.assertRaises(AuthenticationError):
                peer.join(
                    "Private room",
                    "wrong-secret",
                    "Guest",
                    created["host_control_port"],
                    25566,
                    host_address="127.0.0.1",
                )

            status = host.status("Private room", "room-secret", "Host", created["host_control_port"], 25566)
            self.assertEqual(status["peers"], [])
            self.assertEqual(status["player_count"], 1)
        finally:
            peer.stop()
            host.stop()

    def test_connection_refused_explains_room_port_and_firewall_checks(self) -> None:
        probe = socket.socket()
        probe.bind(("127.0.0.1", 0))
        closed_port = int(probe.getsockname()[1])
        probe.close()
        manager = LanSessionManager(bind_host="127.0.0.1")
        try:
            with self.assertRaisesRegex(LanSessionError, "порт комнаты.*брандмауэром"):
                manager.join(
                    "Closed room",
                    "correct horse",
                    "Guest",
                    closed_port,
                    25565,
                    host_address="127.0.0.1",
                )
        finally:
            manager.stop()

    def test_host_leave_and_stop_close_active_membership(self) -> None:
        host = LanSessionManager(bind_host="127.0.0.1")
        peer = LanSessionManager(bind_host="127.0.0.1")
        try:
            created = host.create("Stop room", "room-secret", "Host", 0, 25567)
            peer.join("Stop room", "room-secret", "Guest", created["host_control_port"], 25567)

            left = host.leave("Stop room", "room-secret", "Host", created["host_control_port"], 25567)
            self.assertTrue(left["left"])
            with self.assertRaises(LanSessionError):
                host.status("Stop room", "room-secret", "Host", created["host_control_port"], 25567)
            with self.assertRaises(LanSessionError):
                peer.status("Stop room", "room-secret", "Guest", created["host_control_port"], 25567)

            restarted = host.create("Restarted", "room-secret", "Host", 0, 25568)
            self.assertEqual(restarted["room_name"], "Restarted")
            host.stop()
            host.stop()
        finally:
            peer.stop()
            host.stop()

    def test_bad_room_credentials_names_and_ports_are_rejected(self) -> None:
        manager = LanSessionManager(bind_host="127.0.0.1")
        invalid_create_args = (
            ("", "long-enough", "Host", 0, 25565),
            ("r" * 65, "long-enough", "Host", 0, 25565),
            ("room", "short", "Host", 0, 25565),
            ("room", "long-enough", "\nHost", 0, 25565),
            ("room", "long-enough", "Host", -1, 25565),
            ("room", "long-enough", "Host", 70000, 25565),
            ("room", "long-enough", "Host", True, 25565),
            ("room", "long-enough", "Host", 0, 0),
            ("room", "long-enough", "Host", 0, 65536),
        )
        try:
            for args in invalid_create_args:
                with self.subTest(args=args), self.assertRaises(ValueError):
                    manager.create(*args)

            with self.assertRaises(ValueError):
                manager.join("room", "short", "Guest", 12345, 25565)
            with self.assertRaises(ValueError):
                manager.join("room", "long-enough", "g" * 33, 12345, 25565)
            with self.assertRaises(ValueError):
                manager.join("room", "long-enough", "Guest", 0, 25565)
            with self.assertRaises(LanSessionError):
                manager.join("room", "long-enough", "Guest", 12345, 25565, host_address="8.8.8.8")
        finally:
            manager.stop()


def _wait_for(predicate, timeout: float = 2.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.02)
    return bool(predicate())


if __name__ == "__main__":
    unittest.main()
