from __future__ import annotations

import os
import platform
import socket
import struct
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.server_traffic import ServerTrafficTracker, TrafficUnavailable, parse_diag_message, socket_inodes, tcp_counters


def message(inode=123, port=25565, rx=4096, tx=2048, cookie=1, info_size=232):
    raw = bytearray(72)
    raw[0], raw[1] = socket.AF_INET, 1
    struct.pack_into("!H", raw, 4, port)
    struct.pack_into("=Q", raw, 44, cookie)
    struct.pack_into("=I", raw, 68, inode)
    info = bytearray(info_size)
    if info_size >= 208:
        struct.pack_into("=Q", info, 128, rx)
        struct.pack_into("=Q", info, 200, tx)
    return bytes(raw) + struct.pack("=HH", len(info) + 4, 2) + info


class ServerTrafficTests(unittest.TestCase):
    def test_diag_only_counts_owned_server_port_sockets(self):
        value = parse_diag_message(message(), {123}, 25565)
        self.assertEqual(value[1:], (4096, 2048))
        self.assertIsNone(parse_diag_message(message(inode=456), {123}, 25565))
        self.assertIsNone(parse_diag_message(message(port=54321), {123}, 25565))
        self.assertIsNone(parse_diag_message(b"short", {123}, 25565))

    def test_inode_reuse_does_not_reuse_connection_counters(self):
        first = parse_diag_message(message(cookie=1), {123}, 25565)
        second = parse_diag_message(message(cookie=2), {123}, 25565)
        self.assertNotEqual(first[0], second[0])

    def test_unsupported_tcp_info_is_unavailable_not_fake_zero(self):
        with self.assertRaises(TrafficUnavailable):
            parse_diag_message(message(info_size=104), {123}, 25565)

    def test_ownership_includes_descendant_java_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for pid, children, inode in [(10, "11", 100), (11, "", 101)]:
                (root / str(pid) / "fd").mkdir(parents=True)
                (root / str(pid) / "task" / str(pid)).mkdir(parents=True)
                (root / str(pid) / "task" / str(pid) / "children").write_text(children)
                (root / str(pid) / "fd" / "4").symlink_to(f"socket:[{inode}]")
            self.assertEqual(socket_inodes(10, root), {100, 101})
            with self.assertRaises(TrafficUnavailable):
                socket_inodes(99, root)

    def test_rates_are_deltas_and_totals_survive_closed_connections(self):
        tracker = ServerTrafficTracker(10, 25565)
        with patch("nivora_launcher.server_traffic.time.monotonic", return_value=100) as clock, patch("nivora_launcher.server_traffic.tcp_counters", return_value={"a": (1000, 500)}) as counters:
            first = tracker.sample()
            self.assertIsNone(first["network_rx_bps"])
            self.assertEqual(first["network_rx_bytes"], 1000)
            clock.return_value = 102
            counters.return_value = {"a": (3000, 1500)}
            value = tracker.sample()
            self.assertEqual(value["network_rx_bps"], 1000)
            self.assertEqual(value["network_tx_bps"], 500)
            clock.return_value = 104
            counters.return_value = {"b": (600, 300)}
            value = tracker.sample()
            self.assertEqual(value["network_rx_bytes"], 3600)
            self.assertEqual(value["network_tx_bytes"], 1800)
            self.assertEqual(value["network_rx_bps"], 300)
            self.assertEqual(tracker.sample(running=False)["network_rx_bps"], 0)
            self.assertEqual(tracker.sample(running=False)["network_rx_bytes"], 3600)

    def test_failures_are_visible_and_do_not_substitute_host_traffic(self):
        tracker = ServerTrafficTracker(10, 25565)
        with patch("nivora_launcher.server_traffic.tcp_counters", side_effect=TrafficUnavailable("Permission denied")):
            result = tracker.sample()
        self.assertFalse(result["network_available"])
        self.assertIsNone(result["network_rx_bps"])
        self.assertIn("Permission denied", result["network_error"])

    @unittest.skipUnless(platform.system() == "Linux", "INET_DIAG is Linux-only")
    def test_real_loopback_payload_is_measured_by_kernel(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
            with socket.create_connection(("127.0.0.1", port), timeout=2) as client:
                connection, _ = listener.accept()
                with connection:
                    connection.settimeout(2)
                    client.sendall(b"a" * 4096)
                    data = b""
                    while len(data) < 4096: data += connection.recv(4096 - len(data))
                    connection.sendall(b"b" * 2048)
                    data = b""
                    while len(data) < 2048: data += client.recv(2048 - len(data))
                    try:
                        counters = tcp_counters(os.getpid(), port)
                    except TrafficUnavailable as exc:
                        self.skipTest(f"Kernel socket metrics unavailable: {exc}")
                    self.assertTrue(counters)
                    self.assertEqual(sum(value[0] for value in counters.values()), 4096)
                    self.assertEqual(sum(value[1] for value in counters.values()), 2048)


if __name__ == "__main__":
    unittest.main()
